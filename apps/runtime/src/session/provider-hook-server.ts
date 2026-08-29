import { randomBytes, randomUUID } from 'node:crypto'
import { chmod, mkdir, rename, rm, writeFile } from 'node:fs/promises'
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'
import { join } from 'node:path'

import type { SessionRepository } from '../domain/session-repository'
import { toProviderNotificationEvent, type ProviderNotificationEvent } from './provider-notification-event'

const MAX_HOOK_BYTES = 1024 * 1024

interface HookRegistrationRecord {
  runId: string
  sessionId: string
  permissionMode?: string
  acceptStatuslineIdentity: boolean
  provisionalStatuslineIdentity: boolean
  acceptIdentity: boolean
  settingsPath: string
  statusScriptPath: string
  retirementTimer?: ReturnType<typeof setTimeout>
}


export interface ProviderHookNotification {
  runId: string
  sessionId: string
  provider: 'claude-code'
  event: ProviderNotificationEvent
}

export interface ProviderHookServerOptions {
  onNotification?: (notification: ProviderHookNotification) => void
  onHudPayload?: (event: {
    runId: string
    sessionId: string
    provider: 'claude-code'
    payload: Record<string, unknown>
  }) => void
  onIdentityRecorded?: (event: {
    runId: string
    sessionId: string
    provider: 'claude-code'
    providerSessionId: string
    eventName: string
  }) => void
}

export interface ProviderHookRegistration {
  settingsPath: string
  hookUrl: string
  retire(graceMs?: number): void
  dispose(): Promise<void>
}

const DEFAULT_RETIREMENT_GRACE_MS = 2_000

export class ProviderHookServer {
  readonly #dataRoot: string
  readonly #sessions: SessionRepository
  readonly #registrations = new Map<string, HookRegistrationRecord>()
  readonly #onNotification: (notification: ProviderHookNotification) => void
  readonly #onHudPayload: NonNullable<ProviderHookServerOptions['onHudPayload']>
  readonly #onIdentityRecorded: NonNullable<ProviderHookServerOptions['onIdentityRecorded']>
  #server: Server | undefined
  #port: number | undefined

  constructor(dataRoot: string, sessions: SessionRepository, options: ProviderHookServerOptions = {}) {
    this.#dataRoot = dataRoot
    this.#sessions = sessions
    this.#onNotification = options.onNotification ?? (() => {})
    this.#onHudPayload = options.onHudPayload ?? (() => {})
    this.#onIdentityRecorded = options.onIdentityRecorded ?? (() => {})
  }

  async start(): Promise<void> {
    if (this.#server) return
    const server = createServer((request, response) => {
      void this.#handle(request, response)
    })
    this.#server = server
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject)
      server.listen(0, '127.0.0.1', () => {
        server.off('error', reject)
        resolve()
      })
    })
    this.#port = (server.address() as AddressInfo).port
  }

  async stop(): Promise<void> {
    const server = this.#server
    this.#server = undefined
    this.#port = undefined
    const records = [...this.#registrations.values()]
    this.#registrations.clear()
    for (const record of records) clearTimeout(record.retirementTimer)
    await Promise.all(records.flatMap(({ settingsPath, statusScriptPath }) => [
      rm(settingsPath, { force: true }), rm(statusScriptPath, { force: true })
    ]))
    if (!server) return
    server.closeAllConnections()
    await new Promise<void>((resolve) => server.close(() => resolve()))
  }

  async registerClaudeSession(input: {
    runId: string
    sessionId: string
    permissionMode?: string
    acceptStatuslineIdentity?: boolean
    provisionalStatuslineIdentity?: boolean
  }): Promise<ProviderHookRegistration> {
    if (this.#port === undefined) throw new Error('Provider hook server is not started')
    const token = randomBytes(32).toString('base64url')
    const hookUrl = `http://127.0.0.1:${this.#port}/hooks/${token}`
    const directory = join(this.#dataRoot, 'provider-hooks')
    await mkdir(directory, { recursive: true, mode: 0o700 })
    await chmod(directory, 0o700)
    const settingsPath = join(directory, `${token}.settings.json`)
    const statusScriptPath = join(directory, `${token}.statusline.sh`)
    const temporaryPath = `${settingsPath}.tmp`
    await writeFile(statusScriptPath, statusLineScript(hookUrl), { encoding: 'utf8', mode: 0o700 })
    await chmod(statusScriptPath, 0o700)
    await writeFile(temporaryPath, JSON.stringify(claudeHookSettings(hookUrl, statusScriptPath), null, 2), {
      encoding: 'utf8', mode: 0o600
    })
    await rename(temporaryPath, settingsPath)
    await chmod(settingsPath, 0o600)
    const record: HookRegistrationRecord = {
      runId: input.runId,
      sessionId: input.sessionId,
      ...(input.permissionMode === undefined ? {} : { permissionMode: input.permissionMode }),
      acceptStatuslineIdentity: input.acceptStatuslineIdentity === true,
      provisionalStatuslineIdentity: input.provisionalStatuslineIdentity === true,
      acceptIdentity: true,
      settingsPath,
      statusScriptPath
    }
    this.#registrations.set(token, record)
    let disposed = false
    const dispose = async () => {
      if (disposed) return
      disposed = true
      clearTimeout(record.retirementTimer)
      this.#registrations.delete(token)
      await Promise.all([
        rm(settingsPath, { force: true }), rm(statusScriptPath, { force: true })
      ])
    }
    return {
      settingsPath,
      hookUrl,
      retire: (graceMs = DEFAULT_RETIREMENT_GRACE_MS) => {
        if (disposed || record.retirementTimer) return
        // Keep the endpoint alive briefly for the final HUD/notification hooks, while
        // preventing a process that has already returned to Shell from resurrecting
        // its Claude conversation identity.
        record.acceptIdentity = false
        record.retirementTimer = setTimeout(() => { void dispose() }, Math.max(0, graceMs))
        record.retirementTimer.unref?.()
      },
      dispose
    }
  }

  async #handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    if (request.method !== 'POST') {
      sendJson(response, 405, { error: 'Method not allowed' })
      return
    }
    const token = tokenFromUrl(request.url)
    const registration = token === undefined ? undefined : this.#registrations.get(token)
    if (!registration) {
      sendJson(response, 404, { error: 'Unknown provider run' })
      return
    }
    try {
      const payload = await readJsonBody(request)
      this.#onHudPayload({
        runId: registration.runId,
        sessionId: registration.sessionId,
        provider: 'claude-code',
        payload
      })
      const providerSessionId = providerSessionIdentity(payload)
      const eventName = nonEmptyText(payload.hook_event_name) ?? 'unknown'
      const confirmsConversation = eventName !== 'SessionEnd' && (
        eventName !== 'unknown' || registration.acceptStatuslineIdentity
      )
      if (providerSessionId && confirmsConversation && registration.acceptIdentity) {
        const cwd = nonEmptyText(payload.cwd)
        const hookPermissionMode = nonEmptyText(payload.permission_mode) ??
          nonEmptyText(payload.permissionMode)
        const currentBinding = this.#sessions.getResumeBinding(
          registration.sessionId,
          'claude-code'
        )
        const isKnownIdentity = currentBinding?.providerSessionId === providerSessionId
        // The registration mode is only a launch-time fallback for a newly observed
        // conversation. Reusing it for an already known identity could undo a newer
        // mode selected after this provider run started.
        const permissionMode = hookPermissionMode ??
          (isKnownIdentity ? undefined : registration.permissionMode)
        const now = Date.now()
        this.#sessions.recordResumableProviderIdentity({
          commandId: `provider-hook-${registration.runId}-${randomUUID()}`,
          commandType: 'provider-hook.identity',
          requestHash: `${registration.sessionId}:${providerSessionId}:${eventName}:${now}`
        }, {
          id: `provider-binding-${randomUUID()}`,
          sessionId: registration.sessionId,
          provider: 'claude-code',
          providerSessionId,
          metadata: {
            ...(permissionMode === undefined ? {} : { permissionMode }),
            ...(cwd === undefined ? {} : { cwd }),
            lastHookEvent: eventName
          },
          provisional: eventName === 'unknown' && registration.provisionalStatuslineIdentity,
          now
        })
        this.#onIdentityRecorded({
          runId: registration.runId,
          sessionId: registration.sessionId,
          provider: 'claude-code',
          providerSessionId,
          eventName
        })
      }
      const notificationEvent = toProviderNotificationEvent(payload)
      if (notificationEvent) {
        this.#onNotification({
          runId: registration.runId,
          sessionId: registration.sessionId,
          provider: 'claude-code',
          event: notificationEvent
        })
      }
      sendJson(response, 200, {})
    } catch (error) {
      sendJson(response, 409, { error: errorMessage(error) })
    }
  }
}

function claudeHookSettings(hookUrl: string, statusScriptPath: string): unknown {
  const hook = (timeout: number) => ({ type: 'http', url: hookUrl, timeout })
  return {
    hooks: {
      // Claude Code currently does not dispatch SessionStart to HTTP hooks. Keep
      // the entry for forward compatibility; the first supported later event
      // (usually UserPromptSubmit) records the identity through the same run URL.
      SessionStart: [{ hooks: [hook(5)] }],
      SessionEnd: [{ hooks: [hook(3)] }],
      UserPromptSubmit: [{ hooks: [hook(5)] }],
      PreToolUse: [{
        matcher: 'Bash|Write|Edit|Read|Glob|Grep|TodoWrite|TaskCreate|TaskUpdate|Agent|Skill', hooks: [hook(10)]
      }],
      PostToolUse: [{ matcher: 'Bash|Write|Edit|Read|Glob|Grep|TodoWrite|TaskCreate|TaskUpdate|Agent|Skill', hooks: [hook(10)] }],
      PostToolUseFailure: [{ hooks: [hook(5)] }],
      Stop: [{ hooks: [hook(5)] }],
      Notification: [{ hooks: [hook(5)] }]
    },
    statusLine: { type: 'command', command: statusScriptPath, padding: 0 }
  }
}

function statusLineScript(hookUrl: string): string {
  return `#!/bin/sh\n/usr/bin/curl --silent --show-error --max-time 2 --request POST --header 'content-type: application/json' --data-binary @- '${hookUrl}' >/dev/null 2>&1 || true\n`
}

function tokenFromUrl(url: string | undefined): string | undefined {
  if (!url) return undefined
  const match = /^\/hooks\/([A-Za-z0-9_-]+)$/.exec(url)
  return match?.[1]
}

async function readJsonBody(request: IncomingMessage): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = []
  let size = 0
  for await (const chunk of request) {
    const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)
    size += bytes.byteLength
    if (size > MAX_HOOK_BYTES) throw new Error('Provider hook payload exceeds size limit')
    chunks.push(bytes)
  }
  const parsed = JSON.parse(Buffer.concat(chunks).toString('utf8')) as unknown
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new Error('Provider hook payload must be an object')
  }
  return parsed as Record<string, unknown>
}

export function providerSessionIdentity(payload: Record<string, unknown>): string | undefined {
  const direct = nonEmptyText(payload.session_id) ?? nonEmptyText(payload.sessionId)
  if (direct) return direct
  const transcriptPath = nonEmptyText(payload.transcript_path) ??
    nonEmptyText(payload.transcriptPath) ??
    nonEmptyText(payload.agent_transcript_path)
  return transcriptPath?.match(
    /([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl/i
  )?.[1]
}

function nonEmptyText(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined
  const trimmed = value.trim()
  return trimmed || undefined
}

function sendJson(response: ServerResponse, status: number, body: unknown): void {
  response.writeHead(status, { 'content-type': 'application/json' })
  response.end(JSON.stringify(body))
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
