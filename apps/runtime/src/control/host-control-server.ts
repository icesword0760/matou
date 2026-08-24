import { createHash, randomBytes } from 'node:crypto'
import { chmod, mkdir, rm } from 'node:fs/promises'
import { createServer, type Server, type Socket } from 'node:net'
import { dirname, resolve } from 'node:path'

const CONTROL_VERSION = 1
const DEFAULT_MAX_FRAME_BYTES = 1024 * 1024

export type HostControlScope =
  | 'host.list'
  | 'terminal.read-current'
  | 'terminal.read-history'
  | 'terminal.read-commands'
  | 'terminal.send-text'
  | 'terminal.send-key'
  | 'task.status.write'
  | 'task.progress.write'
  | 'task.log.append'

export interface HostTarget {
  ref: string
  workspaceId: string
  taskId: string
  sessionId: string
  mountId?: string
  title: string
}

export interface HostControlBackend {
  listTargets(): HostTarget[] | Promise<HostTarget[]>
  readCurrent(sessionId: string, limits: { maxLines: number; maxBytes: number }): Promise<unknown>
  readHistory(sessionId: string, limits: { maxLines: number; maxBytes: number }): Promise<unknown>
  readCommands(sessionId: string, limits: { limit: number }): Promise<unknown>
  sendText(sessionId: string, text: string): Promise<void>
  sendKey(sessionId: string, key: AllowedControlKey): Promise<void>
  writeTaskStatus(taskId: string, key: string, value: string | null): Promise<void>
  writeTaskProgress(taskId: string, progress: number, label?: string): Promise<void>
  appendTaskLog(taskId: string, level: TaskLogLevel, source: string, message: string): Promise<void>
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
}

export class CapabilityTokenService {
  readonly #runtimeGeneration: string
  readonly #records = new Map<string, CapabilityRecord>()

  constructor(runtimeGeneration: string) {
    this.#runtimeGeneration = runtimeGeneration
  }

  issue(runId: string, scopes: HostControlScope[], expiresAt: number): string {
    const token = randomBytes(32).toString('base64url')
    const tokenHash = hashToken(token)
    this.#records.set(tokenHash, {
      tokenHash,
      runId,
      scopes: new Set(scopes),
      expiresAt,
      runtimeGeneration: this.#runtimeGeneration
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
  }
}

type AllowedControlKey =
  | 'Enter'
  | 'Tab'
  | 'Escape'
  | 'ArrowUp'
  | 'ArrowDown'
  | 'ArrowLeft'
  | 'ArrowRight'
  | 'CtrlC'
  | 'CtrlD'
  | 'CtrlL'
  | 'CtrlZ'

type TaskLogLevel = 'debug' | 'info' | 'warn' | 'error'

interface ControlRequest {
  version: number
  requestId: string
  token: string
  method: HostControlScope
  params: unknown
  deadlineAt: number
}

type ControlErrorCode =
  | 'INVALID_REQUEST'
  | 'TARGET_NOT_FOUND'
  | 'RUNTIME_NOT_READY'
  | 'AMBIGUOUS_TARGET'
  | 'TIMEOUT'
  | 'CAPABILITY_DENIED'
  | 'CONFLICT'
  | 'UNSUPPORTED'
  | 'INTERNAL_ERROR'

class ControlFault extends Error {
  readonly code: ControlErrorCode
  constructor(code: ControlErrorCode, message: string) {
    super(message)
    this.code = code
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
          this.#write(socket, errorResponse('unknown', 'INVALID_REQUEST', 'control frame exceeds size limit'))
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
      this.#write(socket, errorResponse('unknown', 'INVALID_REQUEST', 'control frame is not valid JSON'))
      return
    }
    let requestId = 'unknown'
    try {
      const request = parseRequest(raw)
      requestId = request.requestId
      if (Date.now() > request.deadlineAt) throw new ControlFault('TIMEOUT', 'request deadline elapsed')
      if (!this.#tokens.validate(request.token, request.method)) {
        throw new ControlFault('CAPABILITY_DENIED', 'capability token is missing, expired, or out of scope')
      }
      const result = await this.#dispatch(request.method, request.params)
      if (Date.now() > request.deadlineAt) throw new ControlFault('TIMEOUT', 'request deadline elapsed')
      this.#write(socket, { version: CONTROL_VERSION, requestId, ok: true, result })
    } catch (error) {
      const fault = error instanceof ControlFault
        ? error
        : new ControlFault('INTERNAL_ERROR', errorMessage(error))
      this.#write(socket, errorResponse(requestId, fault.code, fault.message))
    }
  }

  async #dispatch(method: HostControlScope, rawParams: unknown): Promise<unknown> {
    const params = record(rawParams)
    const targets = await this.#backend.listTargets()
    const projectionRevision = targetRevision(targets)
    if (method === 'host.list') return { projectionRevision, targets }

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
      if (progress < 0 || progress > 100) throw new ControlFault('INVALID_REQUEST', 'progress must be between 0 and 100')
      const label = optionalText(params.label, 'label', 1024)
      await this.#backend.writeTaskProgress(
        text(params.taskId, 'taskId', 160),
        progress,
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

    const sessionId = resolveTarget(params.target, targets, projectionRevision)
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
      await this.#backend.sendText(sessionId, text(params.text, 'text', 64 * 1024))
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

  #write(socket: Socket, value: unknown): void {
    if (socket.destroyed) return
    const body = Buffer.from(JSON.stringify(value))
    const prefix = Buffer.alloc(4)
    prefix.writeUInt32BE(body.byteLength)
    socket.write(Buffer.concat([prefix, body]))
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

function resolveTarget(raw: unknown, targets: HostTarget[], revision: string): string {
  const target = record(raw)
  if (typeof target.sessionId === 'string') {
    if (!targets.some(({ sessionId }) => sessionId === target.sessionId)) {
      throw new ControlFault('TARGET_NOT_FOUND', `Session ${target.sessionId} is not available`)
    }
    return target.sessionId
  }
  const ref = text(target.ref, 'target.ref', 160)
  if (target.projectionRevision !== revision) {
    throw new ControlFault('CONFLICT', 'target ordinal projection is stale; list targets again')
  }
  const matches = targets.filter((target) => target.ref === ref)
  if (matches.length === 0) throw new ControlFault('TARGET_NOT_FOUND', `target ${ref} does not exist`)
  if (matches.length > 1) throw new ControlFault('AMBIGUOUS_TARGET', `target ${ref} is ambiguous`)
  return matches[0]!.sessionId
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
function errorResponse(requestId: string, code: ControlErrorCode, message: string): unknown {
  return { version: CONTROL_VERSION, requestId, ok: false, error: { code, message } }
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
function isAllowedKey(value: unknown): value is AllowedControlKey {
  return typeof value === 'string' && [
    'Enter', 'Tab', 'Escape', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight',
    'CtrlC', 'CtrlD', 'CtrlL', 'CtrlZ'
  ].includes(value)
}
function isControlScope(value: unknown): value is HostControlScope {
  return typeof value === 'string' && [
    'host.list', 'terminal.read-current', 'terminal.read-history', 'terminal.read-commands',
    'terminal.send-text', 'terminal.send-key', 'task.status.write',
    'task.progress.write', 'task.log.append'
  ].includes(value)
}
function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
