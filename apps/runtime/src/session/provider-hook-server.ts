import { randomBytes, randomUUID } from 'node:crypto'
import { watch, type FSWatcher } from 'node:fs'
import { chmod, mkdir, open, rename, rm, stat, writeFile } from 'node:fs/promises'
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'
import { join } from 'node:path'

import type { SessionRepository } from '../domain/session-repository'
import { latestClaudeAutoTitle } from './claude-session-catalog'
import { toProviderNotificationEvent, type ProviderNotificationEvent } from './provider-notification-event'

const MAX_HOOK_BYTES = 1024 * 1024

interface HookRegistrationRecord {
  runId: string
  sessionId: string
  permissionMode?: string
  acceptStatuslineIdentity: boolean
  inheritedConversation: boolean
  acceptIdentity: boolean
  settingsPath: string
  statusScriptPath: string
  retirementTimer?: ReturnType<typeof setTimeout>
  observedTitle?: string
  titleWatch?: {
    providerSessionId: string
    transcriptPath: string
    watcher: FSWatcher
    timeout: ReturnType<typeof setTimeout>
    debounce?: ReturnType<typeof setTimeout>
  }
}


export interface ProviderHookNotification {
  runId: string
  sessionId: string
  provider: 'claude-code'
  event: ProviderNotificationEvent
}

export interface AgentTeamObservation {
  runId: string
  leadSessionId: string
  teammateId: string
  teamId: string
  name: string
  workStatus: 'running' | 'idle' | 'needs-input' | 'error'
  latestLines: string[]
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
  onTitleObserved?: (event: {
    runId: string
    sessionId: string
    provider: 'claude-code'
    providerSessionId: string
    title: string
  }) => void | Promise<void>
  onTeamObservations?: (observations: AgentTeamObservation[]) => void | Promise<void>
}

export interface ProviderHookRegistration {
  settingsPath: string
  hookUrl: string
  retire(graceMs?: number): void
  dispose(): Promise<void>
}

const DEFAULT_RETIREMENT_GRACE_MS = 2_000
const TITLE_WATCH_WINDOW_MS = 10_000
const TITLE_WATCH_DEBOUNCE_MS = 25

export class ProviderHookServer {
  readonly #dataRoot: string
  readonly #sessions: SessionRepository
  readonly #registrations = new Map<string, HookRegistrationRecord>()
  readonly #onNotification: (notification: ProviderHookNotification) => void
  readonly #onHudPayload: NonNullable<ProviderHookServerOptions['onHudPayload']>
  readonly #onIdentityRecorded: NonNullable<ProviderHookServerOptions['onIdentityRecorded']>
  readonly #onTitleObserved: NonNullable<ProviderHookServerOptions['onTitleObserved']>
  readonly #onTeamObservations: NonNullable<ProviderHookServerOptions['onTeamObservations']>
  #server: Server | undefined
  #port: number | undefined

  constructor(dataRoot: string, sessions: SessionRepository, options: ProviderHookServerOptions = {}) {
    this.#dataRoot = dataRoot
    this.#sessions = sessions
    this.#onNotification = options.onNotification ?? (() => {})
    this.#onHudPayload = options.onHudPayload ?? (() => {})
    this.#onIdentityRecorded = options.onIdentityRecorded ?? (() => {})
    this.#onTitleObserved = options.onTitleObserved ?? (() => {})
    this.#onTeamObservations = options.onTeamObservations ?? (() => {})
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
    for (const record of records) {
      clearTimeout(record.retirementTimer)
      this.#stopTitleWatch(record)
    }
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
    inheritedConversation?: boolean
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
      inheritedConversation: input.inheritedConversation === true,
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
      this.#stopTitleWatch(record)
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
      const transcriptPath = providerTranscriptPath(payload)
      if (transcriptPath && nonEmptyText(payload.hook_event_name)) {
        const observations = await readAgentTeamObservations(transcriptPath, {
          runId: registration.runId,
          leadSessionId: registration.sessionId
        })
        if (observations.length > 0) await this.#onTeamObservations(observations)
      }
      const providerSessionId = providerSessionIdentity(payload)
      const eventName = nonEmptyText(payload.hook_event_name) ?? 'unknown'
      if (transcriptPath && providerSessionId && eventName !== 'unknown') {
        const titleObserved = await this.#observeTitle(
          registration, providerSessionId, transcriptPath
        ).catch(() => false)
        if (eventName === 'Stop' && !titleObserved) {
          await this.#startTitleWatch(registration, providerSessionId, transcriptPath)
        }
      }
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
            lastHookEvent: eventName,
            ...(registration.inheritedConversation
              ? { inheritedConversation: true, canFork: true }
              : {})
          },
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

  async #observeTitle(
    registration: HookRegistrationRecord,
    providerSessionId: string,
    transcriptPath: string
  ): Promise<boolean> {
    const transcript = await readTranscriptTail(transcriptPath).catch(() => '')
    const title = latestClaudeAutoTitle(transcript, providerSessionId)
    if (!title) return false
    if (registration.observedTitle === title) {
      this.#stopTitleWatch(registration)
      return true
    }
    await this.#onTitleObserved({
      runId: registration.runId,
      sessionId: registration.sessionId,
      provider: 'claude-code',
      providerSessionId,
      title
    })
    registration.observedTitle = title
    this.#stopTitleWatch(registration)
    return true
  }

  async #startTitleWatch(
    registration: HookRegistrationRecord,
    providerSessionId: string,
    transcriptPath: string
  ): Promise<void> {
    const current = registration.titleWatch
    if (current?.providerSessionId === providerSessionId && current.transcriptPath === transcriptPath) return
    this.#stopTitleWatch(registration)

    let watcher: FSWatcher
    try {
      watcher = watch(transcriptPath, { persistent: false }, () => {
        const active = registration.titleWatch
        if (!active || active.debounce) return
        active.debounce = setTimeout(() => {
          const latest = registration.titleWatch
          if (!latest) return
          delete latest.debounce
          void this.#observeTitle(registration, providerSessionId, transcriptPath).catch(() => {})
        }, TITLE_WATCH_DEBOUNCE_MS)
        active.debounce.unref?.()
      })
    } catch {
      return
    }
    watcher.on('error', () => this.#stopTitleWatch(registration))
    const timeout = setTimeout(() => this.#stopTitleWatch(registration), TITLE_WATCH_WINDOW_MS)
    timeout.unref?.()
    registration.titleWatch = { providerSessionId, transcriptPath, watcher, timeout }

    // Close the gap between the first read and installing the file watcher.
    await this.#observeTitle(registration, providerSessionId, transcriptPath).catch(() => false)
  }

  #stopTitleWatch(registration: HookRegistrationRecord): void {
    const active = registration.titleWatch
    if (!active) return
    delete registration.titleWatch
    clearTimeout(active.timeout)
    clearTimeout(active.debounce)
    active.watcher.close()
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
      PermissionRequest: [{ hooks: [hook(10)] }],
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

export function providerTranscriptPath(payload: Record<string, unknown>): string | undefined {
  return nonEmptyText(payload.transcript_path) ??
    nonEmptyText(payload.transcriptPath) ??
    nonEmptyText(payload.agent_transcript_path)
}

const MAX_TRANSCRIPT_TAIL_BYTES = 4 * 1024 * 1024

async function readAgentTeamObservations(
  transcriptPath: string,
  owner: { runId: string; leadSessionId: string }
): Promise<AgentTeamObservation[]> {
  const transcript = await readTranscriptTail(transcriptPath).catch(() => '')
  if (!transcript) return []
  const members = new Map<string, {
    teammateId: string
    teamId: string
    name: string
    workStatus: AgentTeamObservation['workStatus']
    latestLines: string[]
  }>()
  const addLine = (member: { latestLines: string[] }, value: unknown) => {
    const text = nonEmptyText(value)
    if (!text || member.latestLines.at(-1) === text) return
    member.latestLines.push(text)
    member.latestLines = member.latestLines.slice(-4)
  }
  for (const line of transcript.split('\n')) {
    if (!line.trim()) continue
    let row: Record<string, unknown>
    try {
      const parsed = JSON.parse(line) as unknown
      if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) continue
      row = parsed as Record<string, unknown>
    } catch {
      continue
    }
    const toolUseResult = record(row.toolUseResult)
    if (nonEmptyText(toolUseResult?.status) === 'teammate_spawned') {
      const teammateId = nonEmptyText(toolUseResult?.teammate_id) ??
        nonEmptyText(toolUseResult?.agent_id)
      const name = nonEmptyText(toolUseResult?.name) ?? teammateId?.split('@')[0]
      if (teammateId && name) {
        const member = members.get(name) ?? {
          teammateId,
          teamId: nonEmptyText(toolUseResult?.team_name) ?? teammateId.split('@')[1] ?? '',
          name,
          workStatus: 'running' as const,
          latestLines: []
        }
        member.teammateId = teammateId
        member.teamId = nonEmptyText(toolUseResult?.team_name) ?? member.teamId
        member.workStatus = 'running'
        addLine(member, toolUseResult?.prompt)
        members.set(name, member)
      }
    }
    const listing = nonEmptyText(toolUseResult?.listing)
    if (listing) {
      for (const listingLine of listing.split('\n')) {
        const match = /^\s*(.+?)\s+\[[^\]]+\]\s+·\s+(running|idle)\b/i.exec(listingLine)
        if (!match) continue
        const name = match[1]!.trim()
        const member = members.get(name)
        if (member) member.workStatus = match[2]!.toLowerCase() === 'idle' ? 'idle' : 'running'
      }
    }
    const messageCandidates = [nonEmptyText(row.content), messageText(row.message)]
      .filter((value): value is string => Boolean(value))
    for (const content of messageCandidates) {
      for (const match of content.matchAll(
        /<(?:agent-message\s+from|teammate-message\s+teammate_id)="([^"]+)"[^>]*>([\s\S]*?)<\/(?:agent-message|teammate-message)>/g
      )) {
        const name = match[1]!.split('@')[0]!
        const member = members.get(name)
        if (!member) continue
        const body = match[2]!.trim()
        const idle = parseIdleNotification(body)
        if (idle) {
          member.workStatus = 'idle'
          addLine(member, idle)
        } else {
          addLine(member, body)
        }
      }
    }
  }
  return [...members.values()].map((member) => ({
    ...owner,
    ...member
  }))
}

async function readTranscriptTail(path: string): Promise<string> {
  const info = await stat(path)
  const start = Math.max(0, info.size - MAX_TRANSCRIPT_TAIL_BYTES)
  const length = info.size - start
  const handle = await open(path, 'r')
  try {
    const buffer = Buffer.alloc(length)
    await handle.read(buffer, 0, length, start)
    const value = buffer.toString('utf8')
    return start === 0 ? value : value.slice(Math.max(0, value.indexOf('\n') + 1))
  } finally {
    await handle.close()
  }
}

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined
}

function messageText(value: unknown): string | undefined {
  const message = record(value)
  const content = message?.content
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return undefined
  return content.flatMap((part) => {
    const entry = record(part)
    return nonEmptyText(entry?.text) ?? nonEmptyText(entry?.content) ?? []
  }).join('\n') || undefined
}

function parseIdleNotification(value: string): string | undefined {
  try {
    const parsed = JSON.parse(value) as unknown
    const event = record(parsed)
    if (event?.type !== 'idle_notification') return undefined
    return nonEmptyText(event.result) ?? '队友已完成当前任务'
  } catch {
    return undefined
  }
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
