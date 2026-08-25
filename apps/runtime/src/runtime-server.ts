import { randomUUID } from 'node:crypto'
import { execFile } from 'node:child_process'
import { readlink, stat } from 'node:fs/promises'
import os from 'node:os'
import { promisify } from 'node:util'

import {
  PROTOCOL_VERSION,
  parseRendererMessage,
  type RendererMessage,
  type RuntimeMessage
} from '@matou/contracts'

import { DomainEventStore } from './events/domain-event-store'
import { JournalCorruptionError, readSessionFrames } from './journal/segment-journal'
import type { DecodedJournalFrame } from './journal/segment-journal'
import { CheckpointManager } from './checkpoints/checkpoint-manager'
import type { CapabilityTokenService } from './control/host-control-server'
import type { RuntimeControlBackend } from './control/runtime-control-backend'
import { RpcFault, RuntimeRpcRouter } from './rpc/runtime-rpc-router'
import { PtySession } from './session/pty-session'
import { RuntimeSessionRegistry } from './session/runtime-session-registry'
import { TerminalCwdTracker } from './session/terminal-cwd-tracker'
import { ProviderResumeMonitor } from './session/provider-resume-monitor'
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

export interface RuntimeServerOptions {
  providerResumeTimeoutMs?: number
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
  readonly #workspacePaths: WorkspacePathService
  readonly #cancelledRequests = new Set<string>()
  readonly #subscriptions = new Map<string, { afterSequence: number; batchSize: number }>()
  readonly #replays = new Map<string, ReplayState>()
  readonly #cwdTimers = new Map<string, ReturnType<typeof setTimeout>>()
  readonly #providerResumeTimers = new Map<string, ReturnType<typeof setTimeout>>()
  readonly #pendingShellFallbacks = new Map<string, PendingShellFallback>()
  readonly #spawnQueues = new Map<string, Promise<void>>()
  readonly #providerHooks: ProviderHookServer | undefined
  readonly #providerResumeTimeoutMs: number
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
    this.#sessionRepository = new SessionRepository(database, new DomainTransactionManager(database))
    this.#control = control
    this.#sessions = sessions
    this.#providerHooks = providerHooks
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

  close(): void {
    if (this.#closed) return
    this.#closed = true
    for (const timer of this.#cwdTimers.values()) clearTimeout(timer)
    this.#cwdTimers.clear()
    for (const timer of this.#providerResumeTimers.values()) clearTimeout(timer)
    this.#providerResumeTimers.clear()
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
          'projection-v1'
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
      case 'terminal.input':
        try {
          await this.#workspacePaths.assertSessionInputAllowed(message.sessionId)
          if (this.#closed) break
          const session = this.#session(message.sessionId)
          session?.write(message.data)
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
        this.#disposeSession(message.sessionId)
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
    if (this.#sessions.get(session.sessionId) !== session) return
    const cwd = await processWorkingDirectory(session.pid).catch(() => undefined)
    if (!cwd || this.#sessions.get(session.sessionId) !== session) return
    await this.#persistCwd(session.sessionId, cwd)
  }

  async #persistCwd(sessionId: string, cwd: string): Promise<void> {
    const directory = await stat(cwd).catch(() => undefined)
    if (!directory?.isDirectory()) return
    const authority = this.#sessionRepository.getSession(sessionId)
    if (!authority || authority.cwd === cwd) return
    try {
      const now = Date.now()
      this.#sessionRepository.updateCwd({
        commandId: `runtime-cwd-${sessionId}-${now}-${randomUUID()}`,
        commandType: 'session.cwd-update',
        requestHash: `cwd:${sessionId}:${cwd}:${now}`
      }, sessionId, cwd, now)
    } catch (error) {
      console.error(`[session.cwd-update] ${errorMessage(error)}`)
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
      const result = await this.#router.handle(message.method, message.payload)
      for (const sessionId of disposedSessionIds(result)) {
        this.#disposeSession(sessionId)
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

    const resumeBinding = message.profile === 'shell'
      ? undefined
      : this.#sessionRepository.getResumeBinding(message.sessionId, message.profile)
    let providerProcessStarted = false
    let hookRegistration: ProviderHookRegistration | undefined
    try {
      const runId = persistentAuthority ? randomUUID() : undefined
      const cwdTracker = new TerminalCwdTracker()
      const permissionMode = permissionModeFromMetadata(resumeBinding?.metadata)
      const resumeMonitor = resumeBinding === undefined ? undefined : new ProviderResumeMonitor()
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
          ...(permissionMode === undefined ? {} : { permissionMode })
        })
      }
      const session = await PtySession.create({
        sessionId: message.sessionId,
        executionContextId: message.executionContextId,
        cols: message.cols,
        rows: message.rows,
        cwd,
        dataRoot: this.#dataRoot,
        profile: message.profile,
        ...(resumeBinding === undefined ? {} : {
          providerSessionId: resumeBinding.providerSessionId
        }),
        ...(permissionMode === undefined ? {} : { permissionMode }),
        ...(hookRegistration === undefined ? {} : {
          settingsPath: hookRegistration.settingsPath
        }),
        ...(runId === undefined ? {} : { runId }),
        ...(controlEnvironment === undefined ? {} : { env: controlEnvironment }),
        send: this.#sendToPort,
        onOutput: (data) => {
          const reportedCwd = cwdTracker.ingest(data)
          if (reportedCwd) void this.#persistCwd(message.sessionId, reportedCwd)
          const resumeFailure = resumeMonitor?.ingest(data)
          if (resumeFailure) {
            pendingResumeFailure = resumeFailure
            if (activeSession && resumeBinding) {
              void hookRegistration?.dispose()
              this.#beginResumeFallback(message, activeSession, resumeBinding.id, resumeFailure)
            }
          }
        },
        onExit: (exited, exitCode, signal) => {
          this.#clearProviderResumeTimer(message.sessionId)
          void hookRegistration?.dispose()
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
          if (wasCurrent) {
            if (!resumeExitFallback) {
              this.#attachedSessionIds.delete(message.sessionId)
              this.#endedSessionIds.add(message.sessionId)
            }
            this.#control?.backend.unregister(message.sessionId, exited)
            this.#control?.tokens.revokeRun(exited.runId ?? message.sessionId)
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
          const fallback = this.#pendingShellFallbacks.get(message.sessionId)
          if (fallback?.session === exited) {
            this.#pendingShellFallbacks.delete(message.sessionId)
            void this.#spawnShellFallback(fallback.message)
          }
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
      if (pendingResumeFailure && resumeBinding) {
        this.#beginResumeFallback(message, session, resumeBinding.id, pendingResumeFailure)
      } else if (resumeMonitor && resumeBinding) {
        this.#scheduleProviderResumeTimeout(message, session, resumeBinding.id, resumeMonitor)
      }
    } catch (error) {
      await hookRegistration?.dispose()
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

  #markResumeFailed(sessionId: string, bindingId: string, reason: string): boolean {
    const now = Date.now()
    try {
      this.#sessionRepository.failResumeToShell(
        {
          commandId: `runtime-resume-failed-${sessionId}-${now}-${randomUUID()}`,
          commandType: 'session.resume-failed',
          requestHash: `resume-failed:${sessionId}:${bindingId}:${now}`
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

  #disposeSession(sessionId: string): void {
    this.#clearProviderResumeTimer(sessionId)
    const session = this.#sessions.get(sessionId)
    if (!session) return
    this.#control?.backend.unregister(sessionId, session)
    this.#control?.tokens.revokeRun(session.runId ?? sessionId)
    session.dispose()
    this.#sessions.delete(sessionId, session)
    this.#attachedSessionIds.delete(sessionId)
    this.#endedSessionIds.delete(sessionId)
    this.#completedReplayThrough.delete(sessionId)
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
