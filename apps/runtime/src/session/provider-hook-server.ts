import { randomBytes, randomUUID } from 'node:crypto'
import { chmod, mkdir, rename, rm, writeFile } from 'node:fs/promises'
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'
import { join } from 'node:path'

import type { SessionRepository } from '../domain/session-repository'

const MAX_HOOK_BYTES = 1024 * 1024

interface HookRegistrationRecord {
  runId: string
  sessionId: string
  permissionMode?: string
  settingsPath: string
}

export interface ProviderHookRegistration {
  settingsPath: string
  hookUrl: string
  dispose(): Promise<void>
}

export class ProviderHookServer {
  readonly #dataRoot: string
  readonly #sessions: SessionRepository
  readonly #registrations = new Map<string, HookRegistrationRecord>()
  #server: Server | undefined
  #port: number | undefined

  constructor(dataRoot: string, sessions: SessionRepository) {
    this.#dataRoot = dataRoot
    this.#sessions = sessions
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
    await Promise.all(records.map(({ settingsPath }) => rm(settingsPath, { force: true })))
    if (!server) return
    server.closeAllConnections()
    await new Promise<void>((resolve) => server.close(() => resolve()))
  }

  async registerClaudeSession(input: {
    runId: string
    sessionId: string
    permissionMode?: string
  }): Promise<ProviderHookRegistration> {
    if (this.#port === undefined) throw new Error('Provider hook server is not started')
    const token = randomBytes(32).toString('base64url')
    const hookUrl = `http://127.0.0.1:${this.#port}/hooks/${token}`
    const directory = join(this.#dataRoot, 'provider-hooks')
    await mkdir(directory, { recursive: true, mode: 0o700 })
    await chmod(directory, 0o700)
    const settingsPath = join(directory, `${token}.settings.json`)
    const temporaryPath = `${settingsPath}.tmp`
    await writeFile(temporaryPath, JSON.stringify(claudeHookSettings(hookUrl), null, 2), {
      encoding: 'utf8', mode: 0o600
    })
    await rename(temporaryPath, settingsPath)
    await chmod(settingsPath, 0o600)
    this.#registrations.set(token, {
      runId: input.runId,
      sessionId: input.sessionId,
      ...(input.permissionMode === undefined ? {} : { permissionMode: input.permissionMode }),
      settingsPath
    })
    let disposed = false
    return {
      settingsPath,
      hookUrl,
      dispose: async () => {
        if (disposed) return
        disposed = true
        this.#registrations.delete(token)
        await rm(settingsPath, { force: true })
      }
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
      const providerSessionId = providerSessionIdentity(payload)
      if (providerSessionId) {
        const eventName = nonEmptyText(payload.hook_event_name) ?? 'unknown'
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
          now
        })
      }
      sendJson(response, 200, {})
    } catch (error) {
      sendJson(response, 409, { error: errorMessage(error) })
    }
  }
}

function claudeHookSettings(hookUrl: string): unknown {
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
        matcher: 'Bash|Write|Edit|Read|Glob|Grep', hooks: [hook(10)]
      }],
      PostToolUse: [{ matcher: 'Bash|Write|Edit', hooks: [hook(10)] }],
      Stop: [{ hooks: [hook(5)] }],
      Notification: [{ hooks: [hook(5)] }]
    }
  }
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
