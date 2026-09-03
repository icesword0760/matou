import { createHash, randomBytes } from 'node:crypto'
import { chmod, mkdir, rm } from 'node:fs/promises'
import { createServer, type Server, type Socket } from 'node:net'
import { dirname, resolve } from 'node:path'

import {
  HostControlTargetNotFoundError,
  HostControlTargetNotReadyError,
  type AllowedControlKey,
  type HostCallerIdentity,
  type HostControlErrorDetails,
  type HostControlScope,
  type HostListScope,
  type HostTarget,
  type HostTargetSelector
} from './host-control-types'
import {
  isHostControlCommittedResult,
  runHostControlPostResponseEffects
} from './host-control-post-response'
import type {
  HostActionErrorCode,
  HostActionMethod,
  HostActionResult
} from './host-action-types'

export type {
  AllowedControlKey,
  HostCallerIdentity,
  HostControlErrorDetails,
  HostControlScope,
  HostListScope,
  HostTarget,
  HostTargetSelector
} from './host-control-types'

const CONTROL_VERSION = 1
const DEFAULT_MAX_FRAME_BYTES = 1024 * 1024

export interface HostControlBackend {
  identify(caller: HostCallerIdentity): unknown | Promise<unknown>
  listTargets(caller?: HostCallerIdentity, scope?: HostListScope): HostTarget[] | Promise<HostTarget[]>
  resolveTarget(
    caller: HostCallerIdentity,
    selector: HostTargetSelector,
    targets: HostTarget[],
    projectionRevision: string
  ): string | Promise<string>
  readCurrent(sessionId: string, limits: { maxLines: number; maxBytes: number }): Promise<unknown>
  readHistory(sessionId: string, limits: { maxLines: number; maxBytes: number }): Promise<unknown>
  readCommands(sessionId: string, limits: { limit: number }): Promise<unknown>
  sendText(sessionId: string, text: string, submit: boolean): Promise<void>
  sendKey(sessionId: string, key: AllowedControlKey): Promise<void>
  writeTaskStatus(taskId: string, key: string, value: string | null): Promise<void>
  writeTaskProgress(taskId: string, progress: number, label?: string): Promise<void>
  appendTaskLog(taskId: string, level: TaskLogLevel, source: string, message: string): Promise<void>
  moveTaskToWindow(input: {
    migrationId: string
    taskId: string
    sourceWindowId: string
    targetWindowId: string
  }): Promise<unknown>
  executeHostAction(
    method: HostActionMethod,
    caller: HostCallerIdentity,
    params: unknown
  ): Promise<HostActionResult>
}

export function controlEndpointForPlatform(
  dataRoot: string,
  platform: NodeJS.Platform = process.platform
): string {
  if (platform === 'win32') {
    const profileKey = createHash('sha256').update(dataRoot).digest('hex').slice(0, 24)
    return `\\\\.\\pipe\\matou-${profileKey}`
  }
  return resolve(dataRoot, 'control', 'runtime.sock')
}

interface CapabilityRecord {
  tokenHash: string
  runId: string
  scopes: Set<HostControlScope>
  expiresAt: number
  runtimeGeneration: string
  caller: HostCallerIdentity
}

export class CapabilityTokenService {
  readonly #runtimeGeneration: string
  readonly #records = new Map<string, CapabilityRecord>()
  readonly #onRunRevoked: ((runId: string) => void) | undefined

  constructor(
    runtimeGeneration: string,
    options: { onRunRevoked?: (runId: string) => void } = {}
  ) {
    this.#runtimeGeneration = runtimeGeneration
    this.#onRunRevoked = options.onRunRevoked
  }

  issue(
    callerOrRunId: HostCallerIdentity | string,
    scopes: readonly HostControlScope[],
    expiresAt: number
  ): string {
    const caller = typeof callerOrRunId === 'string'
      ? { runId: callerOrRunId, sessionId: callerOrRunId }
      : callerOrRunId
    const token = randomBytes(32).toString('base64url')
    const tokenHash = hashToken(token)
    this.#records.set(tokenHash, {
      tokenHash,
      runId: caller.runId,
      scopes: new Set(scopes),
      expiresAt,
      runtimeGeneration: this.#runtimeGeneration,
      caller: { ...caller }
    })
    return token
  }

  validate(token: string, scope: HostControlScope, now = Date.now()): CapabilityRecord | undefined {
    const record = this.#records.get(hashToken(token))
    if (
      !record ||
      record.runtimeGeneration !== this.#runtimeGeneration ||
      record.expiresAt < now ||
      !record.scopes.has(scope)
    ) {
      return undefined
    }
    return record
  }

  revokeRun(runId: string): void {
    for (const [hash, record] of this.#records) {
      if (record.runId === runId) this.#records.delete(hash)
    }
    this.#onRunRevoked?.(runId)
  }
}

type TaskLogLevel = 'debug' | 'info' | 'warn' | 'error'

interface ControlRequest {
  version: number
  requestId: string
  token: string
  method: HostControlScope
  params: unknown
  deadlineAt: number
}

export type ControlErrorCode =
  | 'INVALID_REQUEST'
  | 'TARGET_NOT_FOUND'
  | 'TARGET_NOT_READY'
  | 'RUNTIME_NOT_READY'
  | 'AMBIGUOUS_TARGET'
  | 'TIMEOUT'
  | 'CAPABILITY_DENIED'
  | 'CONFLICT'
  | 'UNSUPPORTED'
  | 'INTERNAL_ERROR'
  | HostActionErrorCode

class ControlFault extends Error {
  readonly code: ControlErrorCode
  readonly details: HostControlErrorDetails | undefined
  constructor(
    code: ControlErrorCode,
    message: string,
    details?: HostControlErrorDetails
  ) {
    super(message)
    this.code = code
    this.details = details
  }
}

export class HostControlServer {
  readonly #socketPath: string
  readonly #tokens: CapabilityTokenService
  readonly #backend: HostControlBackend
  readonly #maxFrameBytes: number
  readonly #sockets = new Set<Socket>()
  #server: Server | undefined

  constructor(options: {
    socketPath: string
    tokenService: CapabilityTokenService
    backend: HostControlBackend
    maxFrameBytes?: number
  }) {
    this.#socketPath = options.socketPath
    this.#tokens = options.tokenService
    this.#backend = options.backend
    this.#maxFrameBytes = options.maxFrameBytes ?? DEFAULT_MAX_FRAME_BYTES
  }

  async start(): Promise<void> {
    if (this.#server) return
    if (!isWindowsNamedPipe(this.#socketPath)) {
      const directory = dirname(this.#socketPath)
      await mkdir(directory, { recursive: true, mode: 0o700 })
      await chmod(directory, 0o700)
      await rm(this.#socketPath, { force: true })
    }
    const server = createServer((socket) => this.#accept(socket))
    this.#server = server
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject)
      server.listen(this.#socketPath, () => {
        server.off('error', reject)
        resolve()
      })
    })
    if (!isWindowsNamedPipe(this.#socketPath)) await chmod(this.#socketPath, 0o600)
  }

  async stop(): Promise<void> {
    const server = this.#server
    if (!server) return
    this.#server = undefined
    for (const socket of this.#sockets) socket.destroy()
    await new Promise<void>((resolve) => server.close(() => resolve()))
    if (!isWindowsNamedPipe(this.#socketPath)) await rm(this.#socketPath, { force: true })
  }

  #accept(socket: Socket): void {
    this.#sockets.add(socket)
    socket.once('close', () => this.#sockets.delete(socket))
    let buffered = Buffer.alloc(0)
    let chain = Promise.resolve()
    socket.on('data', (chunk) => {
      buffered = Buffer.concat([buffered, Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)])
      while (buffered.byteLength >= 4) {
        const length = buffered.readUInt32BE(0)
        if (length > this.#maxFrameBytes) {
          void this.#write(socket, errorResponse('unknown', 'INVALID_REQUEST', 'control frame exceeds size limit'))
          socket.end()
          return
        }
        if (buffered.byteLength < 4 + length) return
        const body = buffered.subarray(4, 4 + length)
        buffered = buffered.subarray(4 + length)
        chain = chain.then(() => this.#process(socket, body)).catch(() => undefined)
      }
    })
  }

  async #process(socket: Socket, body: Buffer): Promise<void> {
    let raw: unknown
    try {
      raw = JSON.parse(body.toString('utf8')) as unknown
    } catch {
      await this.#write(socket, errorResponse('unknown', 'INVALID_REQUEST', 'control frame is not valid JSON'))
      return
    }
    let requestId = 'unknown'
    let result: unknown
    try {
      const request = parseRequest(raw)
      requestId = request.requestId
      if (Date.now() > request.deadlineAt) throw new ControlFault('TIMEOUT', 'request deadline elapsed')
      const capability = this.#tokens.validate(request.token, request.method)
      if (!capability) {
        throw new ControlFault('CAPABILITY_DENIED', 'capability token is missing, expired, or out of scope')
      }
      result = await this.#dispatch(request.method, request.params, capability.caller)
      if (
        Date.now() > request.deadlineAt &&
        !isHostControlCommittedResult(result)
      ) throw new ControlFault('TIMEOUT', 'request deadline elapsed')
      await this.#write(socket, { version: CONTROL_VERSION, requestId, ok: true, result })
    } catch (error) {
      const fault = error instanceof ControlFault
        ? error
        : isCodedControlError(error)
          ? new ControlFault(error.code, error.message, controlErrorDetails(error))
        : error instanceof HostControlTargetNotFoundError
          ? new ControlFault('TARGET_NOT_FOUND', error.message)
        : error instanceof HostControlTargetNotReadyError
          ? new ControlFault('TARGET_NOT_READY', error.message)
        : new ControlFault('INTERNAL_ERROR', errorMessage(error))
      await this.#write(socket, errorResponse(requestId, fault.code, fault.message, fault.details))
    } finally {
      await runHostControlPostResponseEffects(result).catch((error) => {
        console.error(`[host-control.post-response] ${errorMessage(error)}`)
      })
    }
  }

  async #dispatch(
    method: HostControlScope,
    rawParams: unknown,
    caller: HostCallerIdentity
  ): Promise<unknown> {
    const params = record(rawParams)
    if (method === 'host.identify') return this.#backend.identify(caller)
    if (isHostActionMethod(method)) {
      return this.#backend.executeHostAction(method, caller, rawParams)
    }

    const listScope = method === 'host.list'
      ? enumerationWithDefault(params.scope, ['current-level', 'all'] as const, 'scope', 'current-level')
      : targetListScope(params.target)
    const targets = await this.#backend.listTargets(caller, listScope)
    const projectionRevision = targetRevision(targets)
    if (method === 'host.list') return { projectionRevision, scope: listScope, targets }

    if (method === 'task.status.write') {
      const value = params.value === null ? null : text(params.value, 'value', 4096)
      await this.#backend.writeTaskStatus(
        text(params.taskId, 'taskId', 160),
        text(params.key, 'key', 160),
        value
      )
      return { written: true }
    }
    if (method === 'task.progress.write') {
      const progress = finiteNumber(params.progress, 'progress')
      const clampedProgress = Math.min(100, Math.max(0, progress))
      const label = optionalText(params.label, 'label', 1024)
      await this.#backend.writeTaskProgress(
        text(params.taskId, 'taskId', 160),
        clampedProgress,
        ...(label === undefined ? [] : [label])
      )
      return { written: true }
    }
    if (method === 'task.log.append') {
      const level = enumeration(params.level, ['debug', 'info', 'warn', 'error'] as const, 'level')
      await this.#backend.appendTaskLog(
        text(params.taskId, 'taskId', 160),
        level,
        text(params.source, 'source', 160),
        text(params.message, 'message', 16 * 1024)
      )
      return { appended: true }
    }
    if (method === 'task.move-to-window') {
      try {
        return await this.#backend.moveTaskToWindow({
          migrationId: text(params.migrationId, 'migrationId', 160),
          taskId: text(params.taskId, 'taskId', 160),
          sourceWindowId: text(params.sourceWindowId, 'sourceWindowId', 160),
          targetWindowId: text(params.targetWindowId, 'targetWindowId', 160)
        })
      } catch (error) {
        throw new ControlFault('CONFLICT', errorMessage(error))
      }
    }

    const selector = parseTargetSelector(params.target)
    if (
      (selector.kind === 'ref' || selector.kind === 'sibling') &&
      selector.projectionRevision !== projectionRevision
    ) {
      throw new ControlFault('STALE_PROJECTION', 'target ordinal projection is stale; list targets again')
    }
    const sessionId = await this.#backend.resolveTarget(caller, selector, targets, projectionRevision)
    if (method === 'terminal.read-current' || method === 'terminal.read-history') {
      const limits = {
        maxLines: boundedInteger(params.maxLines, 'maxLines', 1, 5000),
        maxBytes: boundedInteger(params.maxBytes, 'maxBytes', 1, 1024 * 1024)
      }
      return method === 'terminal.read-current'
        ? this.#backend.readCurrent(sessionId, limits)
        : this.#backend.readHistory(sessionId, limits)
    }
    if (method === 'terminal.read-commands') {
      return this.#backend.readCommands(sessionId, {
        limit: boundedInteger(params.limit, 'limit', 1, 1000)
      })
    }
    if (method === 'terminal.send-text') {
      await this.#backend.sendText(
        sessionId,
        text(params.text, 'text', 64 * 1024),
        booleanWithDefault(params.submit, 'submit', false)
      )
      return { sent: true }
    }
    if (method === 'terminal.send-key') {
      const key = params.key
      if (!isAllowedKey(key)) throw new ControlFault('UNSUPPORTED', 'control key is not allowlisted')
      await this.#backend.sendKey(sessionId, key)
      return { sent: true }
    }
    throw new ControlFault('UNSUPPORTED', `unsupported control method ${method}`)
  }

  #write(socket: Socket, value: unknown): Promise<void> {
    if (socket.destroyed) return Promise.resolve()
    const body = Buffer.from(JSON.stringify(value))
    const prefix = Buffer.alloc(4)
    prefix.writeUInt32BE(body.byteLength)
    return new Promise<void>((resolveWrite) => {
      socket.write(Buffer.concat([prefix, body]), () => resolveWrite())
    })
  }
}

function isWindowsNamedPipe(endpoint: string): boolean {
  return endpoint.startsWith('\\\\.\\pipe\\')
}

function parseRequest(value: unknown): ControlRequest {
  const input = record(value)
  if (input.version !== CONTROL_VERSION) throw new ControlFault('INVALID_REQUEST', 'unsupported control protocol version')
  const method = input.method
  if (!isControlScope(method)) throw new ControlFault('UNSUPPORTED', 'unsupported control method')
  return {
    version: CONTROL_VERSION,
    requestId: text(input.requestId, 'requestId', 160),
    token: text(input.token, 'token', 512),
    method,
    params: input.params,
    deadlineAt: boundedInteger(input.deadlineAt, 'deadlineAt', 1, Number.MAX_SAFE_INTEGER)
  }
}

export function resolveTargetFromProjection(
  selector: HostTargetSelector,
  targets: HostTarget[]
): string {
  if (selector.kind === 'session') {
    if (!targets.some(({ sessionId }) => sessionId === selector.sessionId)) {
      throw new ControlFault('TARGET_NOT_FOUND', `Session ${selector.sessionId} is not available`)
    }
    return selector.sessionId
  }
  if (selector.kind !== 'ref') {
    throw new ControlFault('UNSUPPORTED', `target selector ${selector.kind} needs topology resolution`)
  }
  const matches = targets.filter((target) => target.ref === selector.ref)
  if (matches.length === 0) throw new ControlFault('TARGET_NOT_FOUND', `target ${selector.ref} does not exist`)
  if (matches.length > 1) throw new ControlFault('AMBIGUOUS_TARGET', `target ${selector.ref} is ambiguous`)
  return matches[0]!.sessionId
}

function parseTargetSelector(raw: unknown): HostTargetSelector {
  const target = record(raw)
  if (typeof target.sessionId === 'string') {
    return { kind: 'session', sessionId: text(target.sessionId, 'target.sessionId', 160) }
  }
  if (target.kind === undefined && typeof target.ref === 'string') {
    return {
      kind: 'ref',
      ref: text(target.ref, 'target.ref', 160),
      projectionRevision: text(target.projectionRevision, 'target.projectionRevision', 160)
    }
  }
  const kind = enumeration(target.kind, ['self', 'relative', 'relation', 'sibling', 'ref', 'session'] as const, 'target.kind')
  if (kind === 'self') return { kind }
  if (kind === 'relative') {
    return { kind, direction: enumeration(target.direction, ['left', 'right'] as const, 'target.direction') }
  }
  if (kind === 'relation') {
    const relation = enumeration(target.relation, ['parent', 'child'] as const, 'target.relation')
    const ordinal = target.ordinal === undefined
      ? undefined
      : boundedInteger(target.ordinal, 'target.ordinal', 1, 10_000)
    return { kind, relation, ...(ordinal === undefined ? {} : { ordinal }) }
  }
  if (kind === 'sibling') {
    return {
      kind,
      ordinal: boundedInteger(target.ordinal, 'target.ordinal', 1, 10_000),
      projectionRevision: text(target.projectionRevision, 'target.projectionRevision', 160)
    }
  }
  if (kind === 'ref') {
    return {
      kind,
      ref: text(target.ref, 'target.ref', 160),
      projectionRevision: text(target.projectionRevision, 'target.projectionRevision', 160)
    }
  }
  return { kind, sessionId: text(target.sessionId, 'target.sessionId', 160) }
}

function targetListScope(raw: unknown): HostListScope {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return 'current-level'
  const target = raw as Record<string, unknown>
  return target.kind === 'session' || target.kind === 'ref' || typeof target.sessionId === 'string'
    ? 'all'
    : 'current-level'
}

function targetRevision(targets: HostTarget[]): string {
  return createHash('sha256')
    .update(JSON.stringify(targets.map(({ ref, workspaceId, taskId, sessionId, mountId }) => ({
      ref, workspaceId, taskId, sessionId, mountId
    }))))
    .digest('hex')
}

function hashToken(token: string): string {
  return createHash('sha256').update(token).digest('hex')
}
function errorResponse(
  requestId: string,
  code: ControlErrorCode,
  message: string,
  details?: HostControlErrorDetails
): unknown {
  return {
    version: CONTROL_VERSION,
    requestId,
    ok: false,
    error: { code, message, ...(details === undefined ? {} : { details }) }
  }
}
function record(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new ControlFault('INVALID_REQUEST', 'params must be an object')
  }
  return value as Record<string, unknown>
}
function text(value: unknown, label: string, maximum: number): string {
  if (typeof value !== 'string' || !value.trim() || Buffer.byteLength(value) > maximum) {
    throw new ControlFault('INVALID_REQUEST', `${label} must be a non-empty bounded string`)
  }
  return value
}
function optionalText(value: unknown, label: string, maximum: number): string | undefined {
  return value === undefined ? undefined : text(value, label, maximum)
}
function finiteNumber(value: unknown, label: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    throw new ControlFault('INVALID_REQUEST', `${label} must be a finite number`)
  }
  return value
}
function boundedInteger(value: unknown, label: string, minimum: number, maximum: number): number {
  if (!Number.isSafeInteger(value) || (value as number) < minimum || (value as number) > maximum) {
    throw new ControlFault('INVALID_REQUEST', `${label} must be between ${minimum} and ${maximum}`)
  }
  return value as number
}
function enumeration<const T extends readonly string[]>(value: unknown, values: T, label: string): T[number] {
  if (typeof value !== 'string' || !values.includes(value)) {
    throw new ControlFault('INVALID_REQUEST', `${label} is unsupported`)
  }
  return value as T[number]
}
function enumerationWithDefault<const T extends readonly string[]>(
  value: unknown,
  values: T,
  label: string,
  fallback: T[number]
): T[number] {
  return value === undefined ? fallback : enumeration(value, values, label)
}
function booleanWithDefault(value: unknown, label: string, fallback: boolean): boolean {
  if (value === undefined) return fallback
  if (typeof value !== 'boolean') throw new ControlFault('INVALID_REQUEST', `${label} must be a boolean`)
  return value
}
function isAllowedKey(value: unknown): value is AllowedControlKey {
  return typeof value === 'string' && [
    'Enter', 'Tab', 'Escape', 'Backspace', 'Delete',
    'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight',
    'Home', 'End', 'PageUp', 'PageDown',
    'CtrlC', 'CtrlD', 'CtrlL', 'CtrlU', 'CtrlZ'
  ].includes(value)
}
function isControlScope(value: unknown): value is HostControlScope {
  return typeof value === 'string' && [
    'host.identify', 'host.list', 'terminal.read-current', 'terminal.read-history', 'terminal.read-commands',
    'terminal.send-text', 'terminal.send-key', 'task.status.write',
    'task.progress.write', 'task.log.append', 'task.move-to-window',
    ...HOST_ACTION_METHODS
  ].includes(value)
}
const HOST_ACTION_METHODS = [
  'structure.create.workspace', 'structure.create.task', 'structure.create.canvas',
  'structure.create.session', 'structure.fork.child', 'structure.fork.sibling',
  'structure.fork.children', 'structure.remove.preview', 'structure.remove.commit',
  'structure.canvas-close.preview', 'structure.canvas-close.commit',
  'navigation.focus.session', 'navigation.switch.workspace',
  'navigation.switch.task', 'navigation.switch.canvas'
] as const satisfies readonly HostActionMethod[]
function isHostActionMethod(value: HostControlScope): value is HostActionMethod {
  return (HOST_ACTION_METHODS as readonly string[]).includes(value)
}
function isCodedControlError(error: unknown): error is Error & { code: ControlErrorCode } {
  return error instanceof Error &&
    typeof (error as Error & { code?: unknown }).code === 'string' &&
    CONTROL_ERROR_CODES.includes((error as Error & { code: string }).code as ControlErrorCode)
}
function controlErrorDetails(error: Error & { code: ControlErrorCode }): HostControlErrorDetails | undefined {
  if (error.code !== 'AMBIGUOUS_TARGET') return undefined
  const rawCandidates = (error as Error & { candidates?: unknown }).candidates
  if (!Array.isArray(rawCandidates)) return undefined
  const candidates = rawCandidates.slice(0, 5).flatMap((candidate) => {
    if (typeof candidate !== 'object' || candidate === null || Array.isArray(candidate)) return []
    const humanPath = (candidate as { displayPath?: unknown }).displayPath
    if (
      typeof humanPath !== 'string' ||
      !humanPath.trim() ||
      Buffer.byteLength(humanPath, 'utf8') > 4_096
    ) return []
    return [{ humanPath }]
  })
  return candidates.length > 0 ? { candidates } : undefined
}
const CONTROL_ERROR_CODES = [
  'INVALID_REQUEST', 'TARGET_NOT_FOUND', 'TARGET_NOT_READY', 'RUNTIME_NOT_READY',
  'AMBIGUOUS_TARGET', 'TIMEOUT', 'CAPABILITY_DENIED', 'CONFLICT', 'UNSUPPORTED',
  'INTERNAL_ERROR', 'STALE_PROJECTION', 'CONFIRMATION_REQUIRED',
  'CONFIRMATION_EXPIRED', 'CONFIRMATION_STALE', 'PATH_CONFLICT', 'BRANCH_CONFLICT',
  'WORKTREE_CONFLICT', 'PARTIAL_SUCCESS', 'NAVIGATION_TIMEOUT', 'STORAGE_READ_ONLY'
] as const satisfies readonly ControlErrorCode[]
function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
