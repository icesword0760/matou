import { randomUUID } from 'node:crypto'
import { execFile } from 'node:child_process'
import { readlink, stat } from 'node:fs/promises'
import os from 'node:os'
import { basename } from 'node:path'
import { promisify } from 'node:util'

import {
  PROTOCOL_VERSION,
  parseRendererMessage,
  type RendererMessage,
  type RuntimeMessage
} from '@matou/contracts'

import { DomainEventStore } from './events/domain-event-store'
import { JournalCorruptionError, SegmentJournal, readSessionFrames } from './journal/segment-journal'
import type { DecodedJournalFrame } from './journal/segment-journal'
import { CheckpointManager } from './checkpoints/checkpoint-manager'
import type { CapabilityTokenService } from './control/host-control-server'
import type { RuntimeControlBackend } from './control/runtime-control-backend'
import { RpcFault, RuntimeRpcRouter } from './rpc/runtime-rpc-router'
import { PtySession } from './session/pty-session'
import { RuntimeSessionRegistry } from './session/runtime-session-registry'
import { TerminalCwdTracker } from './session/terminal-cwd-tracker'
import { ProviderResumeMonitor } from './session/provider-resume-monitor'
import { SessionHudRegistry, type HudPermissionMode } from './session/session-hud-registry'
import { SessionForkIntentRepository } from './session/session-fork-intent-repository'
import type {
  ProviderHookRegistration,
  ProviderHookServer
} from './session/provider-hook-server'
import { SessionRepository } from './domain/session-repository'
import { DomainTransactionManager } from './storage/domain-transaction'
import type { RuntimeDatabase } from './storage/database'
import {
  WorkspacePathInvalidError,
  WorkspacePathService
} from './hierarchy/workspace-path-service'
import { HierarchyApplicationService } from './hierarchy/hierarchy-application-service'
import { SessionInteractionService } from './session-canvas/session-interaction-service'
import { ProviderModeService } from './session-canvas/provider-mode-service'

export interface PortMessageEvent {
  data: unknown
}

export interface RuntimePort {
  on(event: 'message', listener: (event: PortMessageEvent) => void): unknown
  on(event: 'close', listener: () => void): unknown
  postMessage(message: RuntimeMessage): void
  start(): void
  close(): void
}

interface ReplayState {
  sessionId: string
  frames: DecodedJournalFrame[]
  cursor: number
  pendingBytes: Map<number, number>
  unackedBytes: number
  liveSequence: number
  activeSession?: PtySession
  finishing?: boolean
}

interface PendingShellFallback {
  session: PtySession
  message: Extract<RendererMessage, { type: 'terminal.spawn' }>
}
type TerminalSpawnMessage = Extract<RendererMessage, { type: 'terminal.spawn' }>

export interface RuntimeServerOptions {
  providerResumeTimeoutMs?: number
  hudRegistry?: SessionHudRegistry
}

const REPLAY_HIGH_WATERMARK_BYTES = 1024 * 1024
const REPLAY_LOW_WATERMARK_BYTES = 512 * 1024
const DEFAULT_PROVIDER_RESUME_TIMEOUT_MS = 10_000
const execFileAsync = promisify(execFile)

export class RuntimeServer {
  readonly #runtimeId = randomUUID()
  readonly #port: RuntimePort
  readonly #sessions: RuntimeSessionRegistry
  readonly #attachedSessionIds = new Set<string>()
  readonly #endedSessionIds = new Set<string>()
  readonly #completedReplayThrough = new Map<string, number>()
  readonly #sendToPort = (message: RuntimeMessage) => this.#port.postMessage(message)
  readonly #dataRoot: string
  readonly #database: RuntimeDatabase
  readonly #router: RuntimeRpcRouter
  readonly #eventStore: DomainEventStore
  readonly #sessionRepository: SessionRepository
  readonly #forkIntents: SessionForkIntentRepository
  readonly #workspacePaths: WorkspacePathService
  readonly #hierarchy: HierarchyApplicationService
  readonly #sessionInteractions: SessionInteractionService
  readonly #providerModes: ProviderModeService
  readonly #cancelledRequests = new Set<string>()
  readonly #subscriptions = new Map<string, { afterSequence: number; batchSize: number }>()
  readonly #replays = new Map<string, ReplayState>()
  readonly #cwdTimers = new Map<string, ReturnType<typeof setTimeout>>()
  readonly #providerResumeTimers = new Map<string, ReturnType<typeof setTimeout>>()
  readonly #providerLaunchRunIds = new Map<string, string>()
  readonly #confirmedProviderRunIds = new Set<string>()
  readonly #spawnDescriptors = new Map<string, TerminalSpawnMessage>()
  readonly #permissionOverrides = new Map<string, HudPermissionMode>()
  readonly #shellInputBuffers = new Map<string, string>()
  readonly #summaryBuffers = new Map<string, string>()
  readonly #summaryTimers = new Map<string, ReturnType<typeof setTimeout>>()
  readonly #skipResumeSessionIds = new Set<string>()
  readonly #pendingShellFallbacks = new Map<string, PendingShellFallback>()
  readonly #spawnQueues = new Map<string, Promise<void>>()
  readonly #providerHooks: ProviderHookServer | undefined
  readonly #providerResumeTimeoutMs: number
  readonly #hud: SessionHudRegistry
  readonly #control:
    | { backend: RuntimeControlBackend; tokens: CapabilityTokenService; endpoint: string }
    | undefined
  #handshakeComplete = false
  #closed = false

  constructor(
    port: RuntimePort,
    dataRoot: string,
    database: RuntimeDatabase,
    router = new RuntimeRpcRouter(database),
    control?: { backend: RuntimeControlBackend; tokens: CapabilityTokenService; endpoint: string },
    sessions = new RuntimeSessionRegistry(),
    providerHooks?: ProviderHookServer,
    workspacePaths = new WorkspacePathService(
      database,
      new DomainTransactionManager(database)
    ),
    options: RuntimeServerOptions = {}
  ) {
    this.#port = port
    this.#dataRoot = dataRoot
    this.#database = database
    this.#router = router
    this.#eventStore = new DomainEventStore(database)
    const transactions = new DomainTransactionManager(database)
    this.#hierarchy = new HierarchyApplicationService(database, transactions)
    this.#sessionInteractions = new SessionInteractionService(database, transactions)
    this.#providerModes = new ProviderModeService(database, transactions)
    this.#sessionRepository = new SessionRepository(database, new DomainTransactionManager(database))
    this.#forkIntents = new SessionForkIntentRepository(database)
    this.#control = control
    this.#sessions = sessions
    this.#providerHooks = providerHooks
    this.#hud = options.hudRegistry ?? new SessionHudRegistry()
    this.#providerResumeTimeoutMs = positiveTimeout(
      options.providerResumeTimeoutMs,
      DEFAULT_PROVIDER_RESUME_TIMEOUT_MS
    )
    this.#workspacePaths = workspacePaths
    for (const session of [...this.#sessions.values()]) {
      const authority = this.#database.get<{ archived_at: number | null }>(
        'SELECT archived_at FROM sessions WHERE id = ?', session.sessionId
      )
      if (authority?.archived_at !== null && authority?.archived_at !== undefined) {
        session.dispose()
        this.#sessions.delete(session.sessionId, session)
      }
    }
    this.#workspacePaths.startPolling()
    port.on('message', (event) => {
      void this.#receive(event.data).catch((error) => {
        this.#sendError('INVALID_MESSAGE', errorMessage(error))
      })
    })
    port.on('close', () => {
      this.#closed = true
      this.#detachAll()
    })
    port.start()
  }

  flushSemanticEvents(): void {
    if (this.#closed) return
    for (const consumerId of this.#subscriptions.keys()) this.#pumpSubscription(consumerId)
  }

  providerIdentityRecorded(sessionId: string, runId: string): void {
    const restoring = this.#database.get<{ id: string }>(
      `SELECT id FROM provider_bindings
       WHERE session_id = ? AND provider = 'claude-code' AND restore_state = 'restoring'
       ORDER BY updated_at DESC, id DESC LIMIT 1`,
      sessionId
    )
    if (restoring) {
      const now = Date.now()
      try {
        this.#providerModes.markClaudeActive({
          commandId: `provider-restore-succeeded-${sessionId}-${runId}`,
          commandType: 'session.restore-succeeded',
          requestHash: `${sessionId}:${restoring.id}:${runId}`
        }, { sessionId, bindingId: restoring.id, now })
        this.flushSemanticEvents()
      } catch (error) {
        console.error(`[session.restore-succeeded] ${errorMessage(error)}`)
      }
    }
    if (this.#providerLaunchRunIds.get(sessionId) !== runId) return
    if (this.#providerResumeTimers.has(sessionId)) {
      this.#clearProviderResumeTimer(sessionId)
      this.#providerLaunchRunIds.delete(sessionId)
      return
    }
    this.#confirmedProviderRunIds.add(runId)
  }

  close(): void {
    if (this.#closed) return
    for (const timer of this.#summaryTimers.values()) clearTimeout(timer)
    this.#summaryTimers.clear()
    for (const sessionId of this.#summaryBuffers.keys()) this.#flushSessionSummary(sessionId)
    this.#closed = true
    for (const timer of this.#cwdTimers.values()) clearTimeout(timer)
    this.#cwdTimers.clear()
    for (const timer of this.#providerResumeTimers.values()) clearTimeout(timer)
    this.#providerResumeTimers.clear()
    this.#providerLaunchRunIds.clear()
    this.#confirmedProviderRunIds.clear()
    this.#detachAll()
    this.#port.close()
  }

  async #receive(rawMessage: unknown): Promise<void> {
    if (this.#closed) return
    let message: RendererMessage
    try {
      message = parseRendererMessage(rawMessage)
    } catch (error) {
      const isVersionMismatch =
        typeof rawMessage === 'object' &&
        rawMessage !== null &&
        'protocolVersion' in rawMessage &&
        rawMessage.protocolVersion !== PROTOCOL_VERSION
      this.#sendError(isVersionMismatch ? 'VERSION_MISMATCH' : 'INVALID_MESSAGE', errorMessage(error))
      return
    }

    if (!this.#handshakeComplete) {
      if (message.type !== 'protocol.hello') {
        this.#sendError('INVALID_MESSAGE', 'protocol.hello must be the first client message')
        return
      }
      this.#handshakeComplete = true
      this.#port.postMessage({
        type: 'protocol.ready',
        protocolVersion: PROTOCOL_VERSION,
        runtimeId: this.#runtimeId,
        capabilities: [
          'terminal-v1',
          'semantic-events-v1',
          'replay-v1',
          'domain-rpc-v1',
          'projection-v1',
          'hud-v1'
        ]
      })
      return
    }

    switch (message.type) {
      case 'protocol.hello':
        this.#sendError('INVALID_MESSAGE', 'protocol handshake is already complete')
        break
      case 'terminal.spawn':
        await this.#spawnSerialized(message)
        break
      case 'terminal.user-interaction': {
        const session = this.#session(message.sessionId)
        if (!session) break
        const now = Date.now()
        this.#sessionInteractions.record({
          commandId: `terminal-interaction-${message.sessionId}-${randomUUID()}`,
          commandType: 'session.user-interaction',
          requestHash: `${message.sessionId}:${message.interactionKind}:${now}`
        }, {
          sessionId: message.sessionId,
          interactionKind: message.interactionKind,
          now
        })
        this.flushSemanticEvents()
        break
      }
      case 'terminal.input':
        try {
          await this.#workspacePaths.assertSessionInputAllowed(message.sessionId)
          if (this.#closed) break
          const session = this.#session(message.sessionId)
          const promoted = session?.profile === 'shell'
            ? await this.#maybePromoteShellAgent(session, message.data)
            : false
          if (!promoted) session?.write(message.data)
          if (session?.profile === 'shell' && /[\r\n]/.test(message.data)) {
            this.#scheduleCwdCapture(session)
          }
        } catch (error) {
          if (error instanceof WorkspacePathInvalidError) {
            this.#sendError(error.code, error.message)
            break
          }
          throw error
        }
        break
      case 'terminal.resize':
        if (this.#attachedSessionIds.has(message.sessionId)) {
          this.#sessions.get(message.sessionId)?.resize(message.cols, message.rows)
        }
        break
      case 'terminal.ack':
        this.#acknowledge(message.sessionId, message.throughSequence)
        break
      case 'terminal.dispose':
        await this.#disposeSession(message.sessionId)
        break
      case 'terminal.replay-request':
        await this.#replay(message)
        break
      case 'rpc.cancel':
        this.#cancelledRequests.add(message.requestId)
        break
      case 'rpc.request':
        await this.#handleRpc(message)
        break
      case 'events.subscribe':
        this.#subscriptions.set(message.consumerId, {
          afterSequence: Math.max(message.afterSequence, this.#eventStore.cursor(message.consumerId)),
          batchSize: message.batchSize
        })
        this.#pumpSubscription(message.consumerId)
        break
    }
  }

  #scheduleCwdCapture(session: PtySession): void {
    const pending = this.#cwdTimers.get(session.sessionId)
    if (pending) clearTimeout(pending)
    this.#cwdTimers.set(session.sessionId, setTimeout(() => {
      this.#cwdTimers.delete(session.sessionId)
      void this.#captureCwd(session)
    }, 1_200))
  }

  async #captureCwd(session: PtySession): Promise<void> {
    if (this.#closed || this.#sessions.get(session.sessionId) !== session) return
    const cwd = await processWorkingDirectory(session.pid).catch(() => undefined)
    if (!cwd || this.#closed || this.#sessions.get(session.sessionId) !== session) return
    await this.#persistCwd(session.sessionId, cwd)
    if (this.#closed) return
    await this.refreshSessionHud(session.sessionId)
  }

  async #persistCwd(sessionId: string, cwd: string): Promise<void> {
    try {
      if (this.#closed) return
      const directory = await stat(cwd).catch(() => undefined)
      if (!directory?.isDirectory() || this.#closed) return
      const hud = this.#hud.snapshot(sessionId)
      if (hud && hud.cwd !== cwd) {
        this.#hud.updateEnvironment(sessionId, {
          cwd,
          ...(hud.shell ? { shell: hud.shell } : {}),
          ...(hud.gitBranch ? { gitBranch: hud.gitBranch, gitDirty: hud.gitDirty } : {})
        })
        await this.refreshSessionHud(sessionId)
      }
      const authority = this.#sessionRepository.getSession(sessionId)
      if (!authority || authority.cwd === cwd) return
      const now = Date.now()
      this.#sessionRepository.updateCwd({
        commandId: `runtime-cwd-${sessionId}-${now}-${randomUUID()}`,
        commandType: 'session.cwd-update',
        requestHash: `cwd:${sessionId}:${cwd}:${now}`
      }, sessionId, cwd, now)
    } catch (error) {
      if (!this.#closed) console.error(`[session.cwd-update] ${errorMessage(error)}`)
    }
  }

  #recordSessionSummary(sessionId: string, data: string): void {
    if (this.#closed) return
    const current = this.#summaryBuffers.get(sessionId) ?? ''
    this.#summaryBuffers.set(sessionId, (current + data).slice(-32_768))
    if (this.#summaryTimers.has(sessionId)) return
    this.#summaryTimers.set(sessionId, setTimeout(() => {
      this.#summaryTimers.delete(sessionId)
      this.#flushSessionSummary(sessionId)
    }, 180))
  }

  #flushSessionSummary(sessionId: string): void {
    const raw = this.#summaryBuffers.get(sessionId)
    if (raw === undefined) return
    try {
      this.#database.run(
        `INSERT INTO session_graph_summaries (session_id, latest_lines_json, updated_at)
         VALUES (?, ?, ?)
         ON CONFLICT(session_id) DO UPDATE SET
           latest_lines_json = excluded.latest_lines_json,
           updated_at = excluded.updated_at`,
        sessionId, JSON.stringify(terminalSummaryLines(raw)), Date.now()
      )
    } catch (error) {
      if (!this.#closed) console.error(`[session.graph-summary] ${errorMessage(error)}`)
    }
  }

  async #handleRpc(message: Extract<RendererMessage, { type: 'rpc.request' }>): Promise<void> {
    if (this.#cancelledRequests.delete(message.requestId)) {
      this.#sendRpcError(message.requestId, 'CANCELLED', 'request was cancelled', false)
      return
    }
    if (Date.now() > message.deadlineAt) {
      this.#sendRpcError(message.requestId, 'TIMEOUT', 'request deadline elapsed', true)
      return
    }
    try {
      const beforeHud = message.method === 'session.set-permission-mode'
        ? this.#hud.snapshot(textFromRpcInput(message.payload, 'sessionId') ?? '')
        : undefined
      const permissionSessionId = message.method === 'session.set-permission-mode'
        ? textFromRpcInput(message.payload, 'sessionId') : undefined
      const permissionSession = permissionSessionId === undefined
        ? undefined : this.#sessions.get(permissionSessionId)
      const ephemeralPermission = permissionSessionId !== undefined && permissionSession !== undefined &&
        permissionSession.profile !== 'shell' &&
        this.#sessionRepository.getResumeBinding(permissionSessionId, 'claude-code') === undefined
      let result = ephemeralPermission
        ? {
            sessionId: permissionSessionId,
            permissionMode: textFromRpcInput(message.payload, 'permissionMode'),
            persisted: false
          }
        : await this.#router.handle(message.method, message.payload)
      await this.#applyHudRpc(message.method, message.payload, beforeHud?.permissionMode)
      if (message.method === 'projection.snapshot') result = withSessionHuds(result, this.#hud.snapshots())
      for (const sessionId of disposedSessionIds(result)) {
        await this.#disposeSession(sessionId)
      }
      if (this.#cancelledRequests.delete(message.requestId)) {
        this.#sendRpcError(message.requestId, 'CANCELLED', 'request was cancelled', false)
        return
      }
      if (Date.now() > message.deadlineAt) {
        this.#sendRpcError(message.requestId, 'TIMEOUT', 'request deadline elapsed', true)
        return
      }
      this.#port.postMessage({
        type: 'rpc.response',
        protocolVersion: PROTOCOL_VERSION,
        requestId: message.requestId,
        runtimeGeneration: this.#database.runtimeGeneration,
        result
      })
      for (const consumerId of this.#subscriptions.keys()) this.#pumpSubscription(consumerId)
    } catch (error) {
      if (error instanceof RpcFault) {
        this.#sendRpcError(message.requestId, error.code, error.message, error.retryable)
      } else {
        this.#sendRpcError(message.requestId, 'INTERNAL_ERROR', errorMessage(error), false)
      }
    }
  }

  async #replay(message: Extract<RendererMessage, { type: 'terminal.replay-request' }>): Promise<void> {
    if (
      !this.#attachedSessionIds.has(message.sessionId) &&
      !this.#database.get('SELECT id FROM sessions WHERE id = ?', message.sessionId)
    ) {
      this.#sendError('SESSION_FORBIDDEN', `session ${message.sessionId} is outside this Renderer capability`)
      return
    }
    let detachedSession: PtySession | undefined
    try {
      this.#completedReplayThrough.delete(message.sessionId)
      const activeSession = this.#attachedSessionIds.has(message.sessionId)
        ? this.#sessions.get(message.sessionId)
        : undefined
      activeSession?.detach(this.#sendToPort)
      detachedSession = activeSession
      const frames = activeSession
        ? await activeSession.readFrames()
        : await readSessionFrames(this.#dataRoot, message.sessionId)
      const availableFromSequence = frames.at(0)?.sequence ?? 0
      const liveSequence = frames.at(-1)?.sequence ?? 0
      const checkpointTerminalWatermark = message.fromSequence === 0
        ? liveSequence
        : Math.min(message.fromSequence, liveSequence)
      const checkpointDomainWatermark = highestDomainCursorAtOrBefore(
        frames,
        checkpointTerminalWatermark
      )
      const checkpoint = await new CheckpointManager(this.#dataRoot, this.#database).loadLatest(
        message.sessionId,
        {
          terminalSequence: checkpointTerminalWatermark,
          domainEventSequence: checkpointDomainWatermark
        }
      )
      if (
        message.fromSequence > 0 &&
        availableFromSequence > 0 &&
        message.fromSequence < availableFromSequence
      ) {
        this.#port.postMessage({
          type: 'terminal.gap',
          protocolVersion: PROTOCOL_VERSION,
          sessionId: message.sessionId,
          requestedFromSequence: message.fromSequence,
          availableFromSequence,
          reason: 'retention'
        })
      }
      this.#port.postMessage({
        type: 'terminal.replay-start',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: message.sessionId,
        ...(checkpoint === undefined ? {} : {
          checkpointSequence: checkpoint.terminalSequence,
          checkpoint: {
            terminalSequence: checkpoint.terminalSequence,
            domainEventSequence: checkpoint.domainEventSequence,
            screenEpoch: checkpoint.screenEpoch,
            snapshot: checkpoint.snapshot
          }
        }),
        availableFromSequence,
        liveSequence
      })
      const requestedFrom = message.fromSequence === 0
        ? availableFromSequence
        : Math.max(message.fromSequence, availableFromSequence)
      const effectiveFrom = checkpoint === undefined
        ? requestedFrom
        : Math.max(requestedFrom, checkpoint.terminalSequence + 1)
      this.#replays.set(message.sessionId, {
        sessionId: message.sessionId,
        frames: frames.filter(({ sequence }) => sequence >= effectiveFrom),
        cursor: 0,
        pendingBytes: new Map(),
        unackedBytes: 0,
        liveSequence,
        ...(activeSession === undefined ? {} : { activeSession })
      })
      this.#pumpReplay(message.sessionId)
    } catch (error) {
      detachedSession?.attach(this.#sendToPort)
      if (error instanceof JournalCorruptionError) {
        this.#port.postMessage({
          type: 'terminal.gap',
          protocolVersion: PROTOCOL_VERSION,
          sessionId: message.sessionId,
          requestedFromSequence: message.fromSequence,
          availableFromSequence: 0,
          reason: 'corruption'
        })
        return
      }
      if (isMissingFile(error)) {
        this.#sendError('SESSION_FORBIDDEN', `session ${message.sessionId} has no replayable journal`)
        return
      }
      this.#sendError('INTERNAL_ERROR', errorMessage(error))
    }
  }

  #acknowledge(sessionId: string, throughSequence: number): void {
    const wasAttached = this.#attachedSessionIds.has(sessionId)
    const session = wasAttached ? this.#sessions.get(sessionId) : undefined
    const replay = this.#replays.get(sessionId)
    if (!session && !replay) {
      if (wasAttached || this.#endedSessionIds.has(sessionId)) return
      this.#sendError('SESSION_FORBIDDEN', `session ${sessionId} is not attached to this connection`)
      return
    }
    if (
      !replay &&
      throughSequence <= (this.#completedReplayThrough.get(sessionId) ?? -1)
    ) return
    if (session && !replay) {
      try {
        session.acknowledge(throughSequence)
      } catch (error) {
        if (
          error instanceof RangeError &&
          /exceeds latest sent sequence/.test(error.message) &&
          throughSequence <= session.lastSequence
        ) return
        throw error
      }
    }
    if (!replay) return
    for (const [sequence, bytes] of replay.pendingBytes) {
      if (sequence > throughSequence) break
      replay.pendingBytes.delete(sequence)
      replay.unackedBytes -= bytes
    }
    if (replay.unackedBytes <= REPLAY_LOW_WATERMARK_BYTES) this.#pumpReplay(sessionId)
  }

  #pumpReplay(sessionId: string): void {
    const replay = this.#replays.get(sessionId)
    if (!replay) return
    while (replay.cursor < replay.frames.length) {
      if (replay.unackedBytes > REPLAY_HIGH_WATERMARK_BYTES) return
      const frame = replay.frames[replay.cursor++]!
      if (frame.kind === 'output') {
        this.#port.postMessage({
          type: 'terminal.data', protocolVersion: PROTOCOL_VERSION,
          sessionId, sequence: frame.sequence, data: frame.data
        })
        replay.pendingBytes.set(frame.sequence, frame.data.byteLength)
        replay.unackedBytes += frame.data.byteLength
      } else if (frame.kind === 'exit') {
        this.#port.postMessage({
          type: 'terminal.exited', protocolVersion: PROTOCOL_VERSION,
          sessionId, sequence: frame.sequence, exitCode: frame.exitCode,
          ...(frame.signal === undefined ? {} : { signal: frame.signal })
        })
      }
    }
    if (!replay.finishing) {
      replay.finishing = true
      void this.#finishReplay(replay)
    }
  }

  async #finishReplay(replay: ReplayState): Promise<void> {
    if (replay.activeSession) {
      const missed = (await replay.activeSession.readFrames()).filter(
        ({ sequence }) => sequence > replay.liveSequence
      )
      if (missed.length > 0) {
        replay.frames = missed
        replay.cursor = 0
        replay.liveSequence = missed.at(-1)!.sequence
        replay.finishing = false
        this.#pumpReplay(replay.sessionId)
        return
      }
      replay.activeSession.attach(this.#sendToPort)
    }
    this.#port.postMessage({
      type: 'terminal.replay-complete', protocolVersion: PROTOCOL_VERSION,
      sessionId: replay.sessionId, throughSequence: replay.liveSequence
    })
    this.#completedReplayThrough.set(replay.sessionId, replay.liveSequence)
    this.#replays.delete(replay.sessionId)
  }

  #pumpSubscription(consumerId: string): void {
    const subscription = this.#subscriptions.get(consumerId)
    if (!subscription) return
    const events = this.#eventStore.readAfter(subscription.afterSequence, subscription.batchSize)
    if (events.length === 0) return
    const throughSequence = events.at(-1)!.sequence
    subscription.afterSequence = throughSequence
    this.#port.postMessage({
      type: 'events.batch',
      protocolVersion: PROTOCOL_VERSION,
      consumerId,
      runtimeGeneration: this.#database.runtimeGeneration,
      events,
      throughSequence
    })
  }

  #sendRpcError(
    requestId: string,
    code: Extract<RuntimeMessage, { type: 'rpc.error' }>['code'],
    message: string,
    retryable: boolean
  ): void {
    this.#port.postMessage({
      type: 'rpc.error',
      protocolVersion: PROTOCOL_VERSION,
      requestId,
      runtimeGeneration: this.#database.runtimeGeneration,
      code,
      message,
      retryable
    })
  }

  async #spawn(message: Extract<RendererMessage, { type: 'terminal.spawn' }>): Promise<void> {
    if (this.#sessions.has(message.sessionId)) {
      const existing = this.#sessions.get(message.sessionId)!
      if (
        existing.executionContextId !== message.executionContextId ||
        existing.profile !== message.profile
      ) {
        this.#sendError('SESSION_FORBIDDEN', 'live Session identity does not match the attach request')
        return
      }
      existing.attach(this.#sendToPort)
      this.#endedSessionIds.delete(message.sessionId)
      this.#completedReplayThrough.delete(message.sessionId)
      this.#attachedSessionIds.add(message.sessionId)
      this.#port.postMessage({
        type: 'terminal.spawned', protocolVersion: PROTOCOL_VERSION,
        sessionId: message.sessionId, pid: existing.pid, reattached: true,
        replayFromSequence: existing.replayFromSequence
      })
      if (!this.#hud.snapshot(message.sessionId)) {
        this.#hud.spawn({
          sessionId: message.sessionId, profile: existing.profile,
          ...(shellName(existing.profile) ? { shell: shellName(existing.profile)! } : {}),
          startedAt: Date.now()
        })
      }
      this.publishSessionHud(message.sessionId)
      void this.refreshSessionHud(message.sessionId)
      return
    }
    const persistentAuthority = this.#database.get<{
      execution_context_id: string
      cwd: string
    }>(
      `SELECT execution_context_id, cwd FROM sessions
       WHERE id = ? AND archived_at IS NULL`,
      message.sessionId
    )
    if (
      persistentAuthority !== undefined &&
      persistentAuthority.execution_context_id !== message.executionContextId
    ) {
      this.#sendError(
        'SESSION_FORBIDDEN',
        'persisted Session execution context does not match the attach request'
      )
      return
    }
    const contextCwd = message.executionContextId === 'local-default'
      ? undefined
      : this.#database.get<{ cwd: string }>(
          'SELECT cwd FROM execution_contexts WHERE id = ? AND archived_at IS NULL',
          message.executionContextId
        )?.cwd
    const cwd = await firstUsableDirectory([
      persistentAuthority?.cwd,
      contextCwd,
      process.env.HOME,
      os.homedir()
    ])
    if (!cwd) {
      this.#sendError('SESSION_FORBIDDEN', 'execution context is not registered')
      return
    }
    if (persistentAuthority && persistentAuthority.cwd !== cwd) {
      await this.#persistCwd(message.sessionId, cwd)
    }

    if (message.executionContextId !== 'local-default') {
      try {
        await this.#workspacePaths.validateExecutionContextBeforeExecution(
          message.executionContextId
        )
      } catch (error) {
        if (error instanceof WorkspacePathInvalidError) {
          this.#sendError(error.code, error.message)
          return
        }
        throw error
      }
    }

    const forkDecision = message.profile === 'claude-code'
      ? this.#forkIntents.claimForLaunch(message.sessionId, Date.now())
      : undefined
    if (forkDecision?.kind === 'failed') {
      await this.#presentForkFailure(message, forkDecision.error)
      return
    }
    const forkLaunch = forkDecision?.kind === 'launch' ? forkDecision : undefined
    const skipResume = this.#skipResumeSessionIds.delete(message.sessionId)
    const resumeBinding = message.profile === 'shell' || skipResume || forkLaunch
      ? undefined
      : this.#sessionRepository.getResumeBinding(message.sessionId, message.profile)
    const providerSessionId = forkLaunch?.sourceProviderSessionId ?? resumeBinding?.providerSessionId
    let providerProcessStarted = false
    let hookRegistration: ProviderHookRegistration | undefined
    try {
      const runId = persistentAuthority ? randomUUID() : undefined
      const cwdTracker = new TerminalCwdTracker()
      const permissionMode = this.#permissionOverrides.get(message.sessionId) ??
        permissionModeFromMetadata(resumeBinding?.metadata)
      if (!this.#hud.snapshot(message.sessionId)) {
        this.#hud.spawn({
          sessionId: message.sessionId,
          profile: message.profile,
          ...(shellName(message.profile) ? { shell: shellName(message.profile)! } : {}),
          cwd,
          startedAt: Date.now(),
          resumable: Boolean(resumeBinding),
          ...(permissionMode === undefined ? {} : { permissionMode })
        })
      }
      const resumeMonitor = providerSessionId === undefined ? undefined : new ProviderResumeMonitor()
      let activeSession: PtySession | undefined
      let pendingResumeFailure: string | undefined
      let controlEnvironment: Record<string, string> | undefined
      if (message.profile !== 'shell' && this.#control) {
        const token = this.#control.tokens.issue(
          runId ?? message.sessionId,
          [
            'host.list', 'terminal.read-current', 'terminal.read-history',
            'terminal.read-commands', 'terminal.send-text', 'terminal.send-key',
            'task.status.write', 'task.progress.write', 'task.log.append',
            'task.move-to-window'
          ],
          Date.now() + 24 * 60 * 60 * 1000
        )
        controlEnvironment = {
          MATOU_CONTROL_ENDPOINT: this.#control.endpoint,
          MATOU_CONTROL_TOKEN: token,
          MATOU_CONTROL_PROTOCOL: '1'
        }
      }
      if (message.profile === 'claude-code' && runId && this.#providerHooks) {
        hookRegistration = await this.#providerHooks.registerClaudeSession({
          runId,
          sessionId: message.sessionId,
          acceptStatuslineIdentity: providerSessionId !== undefined,
          ...(permissionMode === undefined ? {} : { permissionMode })
        })
        if (providerSessionId !== undefined) {
          this.#providerLaunchRunIds.set(message.sessionId, runId)
        }
      }
      const session = await PtySession.create({
        sessionId: message.sessionId,
        executionContextId: message.executionContextId,
        cols: message.cols,
        rows: message.rows,
        cwd,
        dataRoot: this.#dataRoot,
        profile: message.profile,
        ...(providerSessionId === undefined ? {} : { providerSessionId }),
        ...(forkLaunch === undefined ? {} : { forkSession: true }),
        ...(permissionMode === undefined ? {} : { permissionMode }),
        ...(hookRegistration === undefined ? {} : {
          settingsPath: hookRegistration.settingsPath
        }),
        ...(runId === undefined ? {} : { runId }),
        ...(controlEnvironment === undefined ? {} : { env: controlEnvironment }),
        send: this.#sendToPort,
        onOutput: (data) => {
          this.#recordSessionSummary(message.sessionId, data)
          const reportedCwd = cwdTracker.ingest(data)
          if (reportedCwd) void this.#persistCwd(message.sessionId, reportedCwd)
          const resumeFailure = resumeMonitor?.ingest(data)
          if (resumeFailure) {
            pendingResumeFailure = resumeFailure
            if (activeSession && forkLaunch) {
              hookRegistration?.retire()
              this.#beginForkFailure(message, activeSession, resumeFailure)
            } else if (activeSession && resumeBinding) {
              hookRegistration?.retire()
              this.#beginResumeFallback(message, activeSession, resumeBinding.id, resumeFailure)
            }
          }
        },
        onExit: (exited, exitCode, signal) => {
          this.#flushSessionSummary(message.sessionId)
          this.#clearProviderResumeTimer(message.sessionId)
          this.#forgetProviderLaunch(message.sessionId, exited.runId)
          hookRegistration?.retire()
          const resumeExitFallback = Boolean(
            resumeBinding &&
            resumeMonitor?.isMonitoring &&
            this.#sessions.get(message.sessionId) === exited &&
            !this.#pendingShellFallbacks.has(message.sessionId) &&
            this.#markResumeFailed(
              message.sessionId,
              resumeBinding.id,
              `provider resume exited with code ${exitCode}`
            )
          )
          const wasCurrent = this.#sessions.delete(message.sessionId, exited)
          const forkState = forkLaunch ? this.#forkIntents.state(message.sessionId) : undefined
          const forkIncomplete = Boolean(forkLaunch && forkState && forkState !== 'succeeded')
          let forkFailure = Boolean(forkIncomplete && (!wasCurrent || forkState === 'failed'))
          if (wasCurrent && forkIncomplete && forkState !== 'failed') {
            const reason = `Fork 会话进程已退出，代码：${exitCode}`
            this.#forkIntents.fail(message.sessionId, reason, Date.now())
            forkFailure = true
            void this.#appendForkExitFailure(message.sessionId, reason, exited.lastSequence + 1)
          }
          const naturalAgentFallback = wasCurrent && exited.profile !== 'shell' &&
            !resumeExitFallback && (!forkLaunch || forkState === 'succeeded')
          if (wasCurrent) {
            if (!resumeExitFallback && !naturalAgentFallback) {
              this.#attachedSessionIds.delete(message.sessionId)
              this.#endedSessionIds.add(message.sessionId)
            }
            this.#control?.backend.unregister(message.sessionId, exited)
            this.#control?.tokens.revokeRun(exited.runId ?? message.sessionId)
            this.#hud.exit(message.sessionId, {
              fallbackToShell: exited.profile !== 'shell' && !forkLaunch
            })
            this.publishSessionHud(message.sessionId)
            if (!resumeExitFallback && !naturalAgentFallback) {
              this.#spawnDescriptors.delete(message.sessionId)
            }
            this.#shellInputBuffers.delete(message.sessionId)
          }
          if (exited.runId) {
            try {
              this.#sessionRepository.finishRun(
                {
                  commandId: `runtime-exit-${exited.runId}`,
                  commandType: 'session.run-exit',
                  requestHash: `exit:${exited.runId}:${exited.pid}`
                },
                exited.runId,
                { exitCode, ...(signal === undefined ? {} : { signal }), now: Date.now() }
              )
            } catch (error) {
              console.error(`[session.run-exit] ${errorMessage(error)}`)
            }
          }
          if (resumeExitFallback) {
            void this.#spawnShellFallback(message)
            return false
          }
          if (naturalAgentFallback) {
            const now = Date.now()
            try {
              this.#providerModes.markUserExited({
                commandId: `runtime-agent-return-shell-${message.sessionId}-${now}-${randomUUID()}`,
                commandType: 'session.agent-return-shell',
                requestHash: `agent-return-shell:${message.sessionId}:${now}`
              }, { sessionId: message.sessionId, now })
              void this.#spawnSerialized({ ...message, profile: 'shell' })
            } catch (error) {
              try {
                this.#sessionRepository.returnAgentToShell({
                  commandId: `runtime-agent-return-shell-legacy-${message.sessionId}-${now}-${randomUUID()}`,
                  commandType: 'session.agent-return-shell',
                  requestHash: `agent-return-shell-legacy:${message.sessionId}:${now}`
                }, message.sessionId, now)
                void this.#spawnSerialized({ ...message, profile: 'shell' })
              } catch (fallbackError) {
                console.error(`[session.agent-return-shell] ${errorMessage(fallbackError)}`)
              }
            }
            return false
          }
          const fallback = this.#pendingShellFallbacks.get(message.sessionId)
          if (fallback?.session === exited) {
            this.#pendingShellFallbacks.delete(message.sessionId)
            void this.#spawnShellFallback(fallback.message)
          }
          if (forkFailure) return false
        }
      })
      providerProcessStarted = message.profile !== 'shell'
      if (runId) {
        try {
          this.#sessionRepository.startRun(
            {
              commandId: `runtime-start-${runId}`,
              commandType: 'session.run-start',
              requestHash: `start:${runId}:${message.sessionId}:${message.profile}`
            },
            {
              id: runId,
              sessionId: message.sessionId,
              runtimeGeneration: this.#database.runtimeGeneration,
              profile: message.profile,
              pid: session.pid,
              cols: message.cols,
              rows: message.rows,
              now: Date.now()
            }
          )
        } catch (error) {
          this.#control?.tokens.revokeRun(runId)
          session.dispose()
          throw error
        }
      }
      this.#sessions.set(session)
      this.#permissionOverrides.delete(message.sessionId)
      this.#spawnDescriptors.set(message.sessionId, message)
      activeSession = session
      this.#endedSessionIds.delete(message.sessionId)
      this.#completedReplayThrough.delete(message.sessionId)
      this.#attachedSessionIds.add(message.sessionId)
      this.#control?.backend.register(message.sessionId, session)
      this.#port.postMessage({
        type: 'terminal.spawned',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: message.sessionId,
        pid: session.pid
      })
      this.publishSessionHud(message.sessionId)
      void this.refreshSessionHud(message.sessionId)
      if (pendingResumeFailure && forkLaunch) {
        this.#beginForkFailure(message, session, pendingResumeFailure)
      } else if (pendingResumeFailure && resumeBinding) {
        this.#beginResumeFallback(message, session, resumeBinding.id, pendingResumeFailure)
      } else if (resumeMonitor && forkLaunch) {
        this.#scheduleForkResumeTimeout(message, session, resumeMonitor)
      } else if (resumeMonitor && resumeBinding) {
        this.#scheduleProviderResumeTimeout(message, session, resumeBinding.id, resumeMonitor)
      }
    } catch (error) {
      await hookRegistration?.dispose()
      if (forkLaunch && !providerProcessStarted) {
        const reason = `Fork 会话进程启动失败：${errorMessage(error)}`
        this.#forkIntents.fail(message.sessionId, reason, Date.now())
        await this.#presentForkFailure(message, reason)
        return
      }
      if (resumeBinding && !providerProcessStarted) {
        if (this.#markResumeFailed(
          message.sessionId,
          resumeBinding.id,
          `provider process could not start: ${errorMessage(error)}`
        )) {
          await this.#spawnShellFallbackWithinCurrentSpawn(message)
          return
        }
      }
      this.#sendError('INTERNAL_ERROR', errorMessage(error))
    }
  }

  #beginForkFailure(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    session: PtySession,
    reason: string
  ): void {
    if (this.#sessions.get(message.sessionId) !== session) return
    this.#clearProviderResumeTimer(message.sessionId)
    this.#forkIntents.fail(message.sessionId, reason, Date.now())
    session.display('\r\n\u001b[33m[Fork 未完成，请检查上方原因后重试]\u001b[0m\r\n')
    this.#sessions.delete(message.sessionId, session)
    this.#control?.backend.unregister(message.sessionId, session)
    this.#control?.tokens.revokeRun(session.runId ?? message.sessionId)
    session.dispose({ notifyExit: false })
  }

  #scheduleForkResumeTimeout(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    session: PtySession,
    monitor: ProviderResumeMonitor
  ): void {
    if (this.#consumeProviderIdentityConfirmation(message.sessionId, session.runId)) return
    if (!monitor.isMonitoring || this.#forkIntents.state(message.sessionId) === 'succeeded') return
    this.#clearProviderResumeTimer(message.sessionId)
    this.#providerResumeTimers.set(message.sessionId, setTimeout(() => {
      this.#providerResumeTimers.delete(message.sessionId)
      const reason = monitor.timeout()
      if (!reason || this.#sessions.get(message.sessionId) !== session) return
      this.#beginForkFailure(message, session, reason)
    }, this.#providerResumeTimeoutMs))
  }

  async #presentForkFailure(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    reason: string
  ): Promise<void> {
    this.#forkIntents.fail(message.sessionId, reason, Date.now())
    const banner = '[Fork 未完成，请检查上方原因后重试]'
    const frames = await readSessionFrames(this.#dataRoot, message.sessionId).catch(() => [])
    const alreadyPresented = frames.some((frame) =>
      frame.kind === 'output' && new TextDecoder().decode(frame.data).includes(banner)
    )
    if (!alreadyPresented) {
      const journal = await SegmentJournal.open(this.#dataRoot, message.sessionId)
      const sequence = journal.lastSequence + 1
      await journal.appendOutput(sequence, new TextEncoder().encode(
        `\r\n\u001b[31m${reason}\u001b[0m\r\n` +
        `\u001b[33m${banner}\u001b[0m\r\n`
      ))
      await journal.close()
    }
    this.#attachedSessionIds.add(message.sessionId)
    this.#port.postMessage({
      type: 'terminal.spawned', protocolVersion: PROTOCOL_VERSION,
      sessionId: message.sessionId, pid: 0, reattached: true, replayFromSequence: 0
    })
    this.#hud.exit(message.sessionId, { fallbackToShell: false })
    this.publishSessionHud(message.sessionId)
  }

  async #appendForkExitFailure(sessionId: string, reason: string, sequence: number): Promise<void> {
    try {
      const banner = '[Fork 未完成，请检查上方原因后重试]'
      const journal = await SegmentJournal.open(this.#dataRoot, sessionId)
      const nextSequence = Math.max(sequence, journal.lastSequence + 1)
      const data = new TextEncoder().encode(
        `\r\n\u001b[31m${reason}\u001b[0m\r\n` +
        `\u001b[33m${banner}\u001b[0m\r\n`
      )
      await journal.appendOutput(nextSequence, data)
      await journal.close()
      if (this.#closed) return
      this.#port.postMessage({
        type: 'terminal.data', protocolVersion: PROTOCOL_VERSION,
        sessionId, sequence: nextSequence, data
      })
    } catch (error) {
      if (!this.#closed) console.error(`[session.fork-failed] ${errorMessage(error)}`)
    }
  }

  #beginResumeFallback(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    session: PtySession,
    bindingId: string,
    reason: string
  ): void {
    if (this.#sessions.get(message.sessionId) !== session) return
    this.#clearProviderResumeTimer(message.sessionId)
    if (!this.#markResumeFailed(message.sessionId, bindingId, reason)) return
    this.#sessions.delete(message.sessionId, session)
    this.#control?.backend.unregister(message.sessionId, session)
    this.#control?.tokens.revokeRun(session.runId ?? message.sessionId)
    this.#pendingShellFallbacks.set(message.sessionId, { session, message })
    session.dispose({ notifyExit: false })
  }

  #scheduleProviderResumeTimeout(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    session: PtySession,
    bindingId: string,
    monitor: ProviderResumeMonitor
  ): void {
    if (this.#consumeProviderIdentityConfirmation(message.sessionId, session.runId)) return
    if (!monitor.isMonitoring) return
    this.#clearProviderResumeTimer(message.sessionId)
    this.#providerResumeTimers.set(message.sessionId, setTimeout(() => {
      this.#providerResumeTimers.delete(message.sessionId)
      const reason = monitor.timeout()
      if (!reason || this.#sessions.get(message.sessionId) !== session) return
      this.#beginResumeFallback(message, session, bindingId, reason)
    }, this.#providerResumeTimeoutMs))
  }

  #clearProviderResumeTimer(sessionId: string): void {
    const timer = this.#providerResumeTimers.get(sessionId)
    if (!timer) return
    clearTimeout(timer)
    this.#providerResumeTimers.delete(sessionId)
  }

  #consumeProviderIdentityConfirmation(sessionId: string, runId: string | undefined): boolean {
    if (!runId || !this.#confirmedProviderRunIds.delete(runId)) return false
    if (this.#providerLaunchRunIds.get(sessionId) === runId) {
      this.#providerLaunchRunIds.delete(sessionId)
    }
    return true
  }

  #forgetProviderLaunch(sessionId: string, runId: string | undefined): void {
    if (!runId) return
    this.#confirmedProviderRunIds.delete(runId)
    if (this.#providerLaunchRunIds.get(sessionId) === runId) {
      this.#providerLaunchRunIds.delete(sessionId)
    }
  }

  #markResumeFailed(sessionId: string, bindingId: string, reason: string): boolean {
    const now = Date.now()
    try {
      this.#providerModes.markRestoreFailed(
        {
          commandId: `runtime-resume-failed-${sessionId}-${now}-${randomUUID()}`,
          commandType: 'session.resume-failed',
          requestHash: `resume-failed:${sessionId}:${bindingId}:${now}`
        },
        { sessionId, bindingId, reason, now }
      )
      return true
    } catch (graphError) {
      try {
        this.#sessionRepository.failResumeToShell(
          {
            commandId: `runtime-resume-failed-legacy-${sessionId}-${now}-${randomUUID()}`,
            commandType: 'session.resume-failed',
            requestHash: `resume-failed-legacy:${sessionId}:${bindingId}:${now}`
          },
          sessionId,
          bindingId,
          reason,
          now
        )
        return true
      } catch (error) {
        console.error(`[session.resume-failed] ${errorMessage(error)}`)
        return false
      }
    }
  }

  async #spawnShellFallback(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>
  ): Promise<void> {
    await this.#spawnSerialized({ ...message, profile: 'shell' })
    this.#displayResumeFallback(message.sessionId)
  }

  async #spawnShellFallbackWithinCurrentSpawn(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>
  ): Promise<void> {
    await this.#spawn({ ...message, profile: 'shell' })
    this.#displayResumeFallback(message.sessionId)
  }

  #displayResumeFallback(sessionId: string): void {
    const shell = this.#sessions.get(sessionId)
    if (shell?.profile === 'shell') {
      shell.display('\r\n\u001b[33m[上次会话无法续接，已回到普通终端]\u001b[0m\r\n')
    }
  }

  async #spawnSerialized(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>
  ): Promise<void> {
    const previous = this.#spawnQueues.get(message.sessionId) ?? Promise.resolve()
    const current = previous.catch(() => undefined).then(() => this.#spawn(message))
    this.#spawnQueues.set(message.sessionId, current)
    try {
      await current
    } finally {
      if (this.#spawnQueues.get(message.sessionId) === current) {
        this.#spawnQueues.delete(message.sessionId)
      }
    }
  }

  #session(sessionId: string): PtySession | undefined {
    const session = this.#attachedSessionIds.has(sessionId) ? this.#sessions.get(sessionId) : undefined
    if (!session) {
      this.#sendError('SESSION_FORBIDDEN', `session ${sessionId} is not attached to this connection`)
    }
    return session
  }

  async #disposeSession(sessionId: string): Promise<void> {
    this.#clearProviderResumeTimer(sessionId)
    const cwdTimer = this.#cwdTimers.get(sessionId)
    if (cwdTimer) clearTimeout(cwdTimer)
    this.#cwdTimers.delete(sessionId)
    const session = this.#sessions.get(sessionId)
    if (!session) return
    this.#control?.backend.unregister(sessionId, session)
    this.#control?.tokens.revokeRun(session.runId ?? sessionId)
    this.#sessions.delete(sessionId, session)
    this.#endedSessionIds.delete(sessionId)
    this.#completedReplayThrough.delete(sessionId)
    this.#spawnDescriptors.delete(sessionId)
    this.#permissionOverrides.delete(sessionId)
    this.#shellInputBuffers.delete(sessionId)
    this.#skipResumeSessionIds.delete(sessionId)
    this.#hud.delete(sessionId)
    this.publishSessionHud(sessionId)
    this.#attachedSessionIds.delete(sessionId)
    session.dispose()
    await session.whenClosed()
  }

  #detachAll(): void {
    this.#workspacePaths.stopPolling()
    for (const sessionId of this.#attachedSessionIds) {
      this.#sessions.get(sessionId)?.detach(this.#sendToPort)
    }
    this.#attachedSessionIds.clear()
    this.#endedSessionIds.clear()
    this.#completedReplayThrough.clear()
    this.#replays.clear()
  }

  #sendError(code: Extract<RuntimeMessage, { type: 'protocol.error' }>['code'], message: string): void {
    console.error(`[protocol.error] ${code}: ${message}`)
    this.#port.postMessage({
      type: 'protocol.error',
      protocolVersion: PROTOCOL_VERSION,
      code,
      message
    })
  }

  publishSessionHud(sessionId: string): void {
    if (this.#closed || !this.#attachedSessionIds.has(sessionId)) return
    this.#port.postMessage({
      type: 'terminal.hud', protocolVersion: PROTOCOL_VERSION,
      sessionId, hud: this.#hud.snapshot(sessionId) ?? null
    })
  }

  async refreshSessionHud(sessionId: string): Promise<void> {
    if (this.#forkIntents.state(sessionId) === 'succeeded') {
      this.#clearProviderResumeTimer(sessionId)
    }
    const current = this.#hud.snapshot(sessionId)
    if (!current?.cwd) {
      this.publishSessionHud(sessionId)
      return
    }
    const git = await gitEnvironment(current.cwd)
    this.#hud.updateEnvironment(sessionId, {
      cwd: current.cwd,
      ...(current.shell ? { shell: current.shell } : {}),
      ...(git ?? {})
    })
    this.publishSessionHud(sessionId)
  }

  async #applyHudRpc(
    method: string,
    payload: unknown,
    previousPermissionMode?: HudPermissionMode
  ): Promise<void> {
    const input = rpcInput(payload)
    const sessionId = typeof input?.sessionId === 'string' ? input.sessionId : undefined
    if (!sessionId || !input) return
    const session = this.#sessions.get(sessionId)
    if (method === 'session.set-model') {
      const strategy = typeof input.modelStrategy === 'string' ? input.modelStrategy : ''
      this.#hud.updateModel(sessionId, strategy)
      session?.write(`/model ${strategy}\r`)
      this.publishSessionHud(sessionId)
      return
    }
    if (method !== 'session.set-permission-mode') return
    const target = typeof input.permissionMode === 'string' ? input.permissionMode : ''
    const respawn = input.respawn === true
    if (respawn) {
      await this.#respawnWithPermission(sessionId, target as HudPermissionMode)
    } else if (session) {
      const order: HudPermissionMode[] = ['default', 'acceptEdits', 'plan']
      const from = order.indexOf(previousPermissionMode ?? 'default')
      const to = order.indexOf(target as HudPermissionMode)
      if (from >= 0 && to >= 0) {
        const steps = (to - from + order.length) % order.length
        for (let index = 0; index < steps; index += 1) session.write('\u001b[Z')
      }
    }
    this.#hud.updatePermission(sessionId, target)
    this.publishSessionHud(sessionId)
  }

  async #respawnWithPermission(sessionId: string, target: HudPermissionMode): Promise<void> {
    const session = this.#sessions.get(sessionId)
    const descriptor = this.#spawnDescriptors.get(sessionId)
    if (!session || !descriptor || session.profile === 'shell') {
      throw new RpcFault('CONFLICT', 'active AI Session is required for permission respawn')
    }
    this.#clearProviderResumeTimer(sessionId)
    this.#sessions.delete(sessionId, session)
    this.#control?.backend.unregister(sessionId, session)
    this.#control?.tokens.revokeRun(session.runId ?? sessionId)
    session.dispose({ notifyExit: false })
    await session.whenClosed()
    this.#permissionOverrides.set(sessionId, target)
    await this.#spawn(descriptor)
    const replacement = this.#sessions.get(sessionId)
    if (!replacement || replacement === session || replacement.profile === 'shell') {
      this.#permissionOverrides.delete(sessionId)
      throw new RpcFault('INTERNAL_ERROR', 'AI Session permission respawn failed')
    }
    await Promise.race([
      replacement.whenClosed(),
      new Promise<void>((resolve) => setTimeout(resolve, 150))
    ])
    if (this.#sessions.get(sessionId) !== replacement) {
      this.#permissionOverrides.delete(sessionId)
      throw new RpcFault('INTERNAL_ERROR', 'AI Session permission respawn failed')
    }
    replacement.display('\u001b[2J\u001b[3J\u001b[H')
  }

  async #maybePromoteShellAgent(session: PtySession, data: string): Promise<boolean> {
    const previous = this.#shellInputBuffers.get(session.sessionId) ?? ''
    const next = updateShellInputBuffer(previous, data)
    this.#shellInputBuffers.set(session.sessionId, next.buffer)
    if (!next.submitted) return false
    this.#shellInputBuffers.delete(session.sessionId)
    const launch = await resolveInteractiveClaudeLaunch(next.command)
    if (!launch) return false

    const descriptor = this.#spawnDescriptors.get(session.sessionId)
    if (!descriptor || this.#sessions.get(session.sessionId) !== session) return false
    session.write('\u0015')
    this.#sessions.delete(session.sessionId, session)
    this.#control?.backend.unregister(session.sessionId, session)
    this.#control?.tokens.revokeRun(session.runId ?? session.sessionId)
    session.dispose({ notifyExit: false })
    await session.whenClosed()
    const now = Date.now()
    try {
      this.#sessionRepository.promoteShellToAgent({
        commandId: `runtime-shell-promote-agent-${session.sessionId}-${now}-${randomUUID()}`,
        commandType: 'session.shell-promote-agent',
        requestHash: `shell-promote-agent:${session.sessionId}:${now}`
      }, session.sessionId, 'claude-code', now)
      const before = this.#hud.snapshot(session.sessionId)
      this.#permissionOverrides.set(session.sessionId, launch.permissionMode)
      this.#hud.spawn({
        sessionId: session.sessionId,
        profile: 'claude-code',
        ...(before?.cwd ? { cwd: before.cwd } : {}),
        startedAt: before?.startedAt ?? now,
        permissionMode: launch.permissionMode,
        modelStrategy: 'opusplan'
      })
      this.publishSessionHud(session.sessionId)
      this.#skipResumeSessionIds.add(session.sessionId)
      await this.#spawn({ ...descriptor, profile: 'claude-code' })
      const replacement = this.#sessions.get(session.sessionId)
      if (!replacement || replacement.profile !== 'claude-code') {
        throw new Error('Claude process did not start')
      }
      replacement.display('\u001b[2J\u001b[3J\u001b[H')
      return true
    } catch (error) {
      console.error(`[session.shell-promote-agent] ${errorMessage(error)}`)
      this.#permissionOverrides.delete(session.sessionId)
      try {
        this.#sessionRepository.returnAgentToShell({
          commandId: `runtime-shell-promote-rollback-${session.sessionId}-${now}-${randomUUID()}`,
          commandType: 'session.shell-promote-rollback',
          requestHash: `shell-promote-rollback:${session.sessionId}:${now}`
        }, session.sessionId, Date.now())
      } catch {}
      await this.#spawn({ ...descriptor, profile: 'shell' })
      return true
    }
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function positiveTimeout(value: number | undefined, fallback: number): number {
  return Number.isFinite(value) && Number(value) > 0 ? Number(value) : fallback
}

function permissionModeFromMetadata(metadata: unknown): string | undefined {
  if (typeof metadata !== 'object' || metadata === null || !('permissionMode' in metadata)) {
    return undefined
  }
  return typeof metadata.permissionMode === 'string' ? metadata.permissionMode : undefined
}

function isMissingFile(error: unknown): boolean {
  return error instanceof Error && 'code' in error && error.code === 'ENOENT'
}

async function processWorkingDirectory(pid: number): Promise<string | undefined> {
  let cwd: string | undefined
  if (process.platform === 'linux') {
    cwd = await readlink(`/proc/${pid}/cwd`)
  } else if (process.platform === 'darwin') {
    const { stdout } = await execFileAsync('lsof', [
      '-a', '-p', String(pid), '-d', 'cwd', '-Fn'
    ])
    cwd = stdout.split('\n').find((line) => line.startsWith('n'))?.slice(1)
  }
  if (!cwd) return undefined
  const info = await stat(cwd)
  return info.isDirectory() ? cwd : undefined
}

async function firstUsableDirectory(candidates: Array<string | undefined>): Promise<string | undefined> {
  for (const candidate of candidates) {
    if (!candidate) continue
    const info = await stat(candidate).catch(() => undefined)
    if (info?.isDirectory()) return candidate
  }
  return undefined
}

function highestDomainCursorAtOrBefore(frames: DecodedJournalFrame[], terminalSequence: number): number {
  let sequence = 0
  for (const frame of frames) {
    if (frame.sequence > terminalSequence) break
    if (frame.kind === 'domain-cursor') sequence = Math.max(sequence, frame.domainEventSequence)
  }
  return sequence
}

function disposedSessionIds(result: unknown): string[] {
  if (typeof result !== 'object' || result === null || !('disposedSessionIds' in result)) {
    return []
  }
  const value = result.disposedSessionIds
  return Array.isArray(value) && value.every((item) => typeof item === 'string')
    ? value
    : []
}

function rpcInput(payload: unknown): Record<string, unknown> | undefined {
  if (typeof payload !== 'object' || payload === null || !('input' in payload)) return undefined
  const input = payload.input
  return typeof input === 'object' && input !== null && !Array.isArray(input)
    ? input as Record<string, unknown> : undefined
}

function textFromRpcInput(payload: unknown, key: string): string | undefined {
  const value = rpcInput(payload)?.[key]
  return typeof value === 'string' ? value : undefined
}

function withSessionHuds(result: unknown, sessionHuds: ReturnType<SessionHudRegistry['snapshots']>): unknown {
  if (typeof result !== 'object' || result === null || !('hierarchy' in result)) return result
  const hierarchy = result.hierarchy
  if (typeof hierarchy !== 'object' || hierarchy === null) return result
  return { ...result, hierarchy: { ...hierarchy, sessionHuds } }
}

function shellName(profile: 'shell' | 'claude-code' | 'codex'): string | undefined {
  if (profile !== 'shell') return undefined
  const shell = process.env.SHELL
  return shell ? basename(shell) : process.platform === 'win32' ? 'PowerShell' : undefined
}

async function gitEnvironment(cwd: string): Promise<{ gitBranch: string; gitDirty: boolean } | undefined> {
  try {
    const [{ stdout: branchOutput }, { stdout: statusOutput }] = await Promise.all([
      execFileAsync('git', ['-C', cwd, 'rev-parse', '--abbrev-ref', 'HEAD']),
      execFileAsync('git', ['-C', cwd, 'status', '--porcelain', '--untracked-files=normal'])
    ])
    const gitBranch = branchOutput.trim()
    return gitBranch ? { gitBranch, gitDirty: statusOutput.trim().length > 0 } : undefined
  } catch {
    return undefined
  }
}

export function terminalSummaryLines(raw: string): string[] {
  const text = raw
    .replace(/\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g, '')
    .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/\r/g, '\n')
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, '')
  return text.split('\n').map((line) => line.trimEnd())
    .filter((line) => line.trim().length > 0).slice(-4)
}

function updateShellInputBuffer(previous: string, data: string): {
  buffer: string
  submitted: boolean
  command: string
} {
  let buffer = previous
  let submitted = false
  let command = ''
  for (const character of data) {
    if (character === '\r' || character === '\n') {
      submitted = true
      command = buffer
      buffer = ''
    } else if (character === '\u007f' || character === '\b') {
      buffer = buffer.slice(0, -1)
    } else if (character === '\u0003' || character === '\u0015') {
      buffer = ''
    } else if (character >= ' ' && character !== '\u007f') {
      buffer += character
    }
  }
  return { buffer, submitted, command }
}

async function resolveInteractiveClaudeLaunch(command: string): Promise<{
  permissionMode: HudPermissionMode
} | undefined> {
  const normalized = command.trim().replace(/\s+/g, ' ')
  if (normalized === 'claude') return { permissionMode: 'default' }
  if (normalized === 'claude --dangerously-skip-permissions') {
    return { permissionMode: 'bypassPermissions' }
  }
  if (normalized !== 'cc') return undefined
  const alias = await configuredShellAlias('cc')
  if (alias === 'claude') return { permissionMode: 'default' }
  if (alias === 'claude --dangerously-skip-permissions') {
    return { permissionMode: 'bypassPermissions' }
  }
  return undefined
}

async function configuredShellAlias(name: string): Promise<string | undefined> {
  const shell = process.env.SHELL ?? (process.platform === 'win32' ? undefined : '/bin/zsh')
  if (!shell) return undefined
  try {
    const { stdout } = await execFileAsync(shell, [
      '-ic', 'alias "$1"', 'matou-alias', name
    ], { timeout: 10_000, maxBuffer: 64 * 1024 })
    const prefix = `${name}=`
    const line = stdout.split('\n').map((value) => value.trim())
      .find((value) => value.startsWith(prefix) || value.startsWith(`alias ${prefix}`))
    if (!line) return undefined
    const value = line.slice(line.indexOf('=') + 1).trim()
    if ((value.startsWith("'") && value.endsWith("'")) ||
        (value.startsWith('"') && value.endsWith('"'))) {
      return value.slice(1, -1).trim().replace(/\s+/g, ' ')
    }
    return value.replace(/\\ /g, ' ').trim().replace(/\s+/g, ' ')
  } catch {
    return undefined
  }
}
