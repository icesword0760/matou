import { randomUUID } from 'node:crypto'
import { execFile } from 'node:child_process'
import { watch, type FSWatcher } from 'node:fs'
import { readlink, stat } from 'node:fs/promises'
import os from 'node:os'
import { basename, delimiter, join } from 'node:path'
import { promisify } from 'node:util'

import {
  PROTOCOL_VERSION,
  parseRendererMessage,
  type RendererMessage,
  type RpcMethod,
  type RuntimeMessage,
  type ProviderCli
} from '@matou/contracts'

import { DomainEventStore } from './events/domain-event-store'
import {
  JournalCorruptionError,
  SegmentJournal,
  type SegmentJournalOptions
} from './journal/segment-journal'
import type { DecodedJournalFrame } from './journal/segment-journal'
import {
  iterateSessionFrames,
  readSessionReplayMetadata
} from './journal/journal-range-reader'
import { JournalHistoryReader } from './journal/journal-history-reader'
import {
  CheckpointManager,
  type LoadedCheckpoint
} from './checkpoints/checkpoint-manager'
import type { CapabilityTokenService } from './control/host-control-server'
import type { RuntimeControlBackend } from './control/runtime-control-backend'
import { RpcFault, RuntimeRpcRouter } from './rpc/runtime-rpc-router'
import { PtySession } from './session/pty-session'
import { RuntimeSessionRegistry } from './session/runtime-session-registry'
import { TerminalCwdTracker } from './session/terminal-cwd-tracker'
import { TerminalWorkStatusTracker } from './session/terminal-work-status-tracker'
import { ClaudePermissionModeTracker } from './session/claude-permission-mode-tracker'
import { ProviderResumeMonitor } from './session/provider-resume-monitor'
import { SessionHudRegistry, type HudPermissionMode } from './session/session-hud-registry'
import { SessionForkIntentRepository } from './session/session-fork-intent-repository'
import {
  SessionExecutionService,
  type ForkExecutionAuthority,
  type ForkExecutionAuthorityInput,
  type SessionExecutionDescriptor,
  type SessionExecutionResult
} from './session/session-execution-service'
import { SessionGitStateRepository } from './session/session-git-state-repository'
import type {
  ProviderHookRegistration,
  ProviderHookServer,
  ProviderIdentityMismatch
} from './session/provider-hook-server'
import { SessionRepository } from './domain/session-repository'
import { SessionEnvironmentRepository } from './session/session-environment-repository'
import { SessionEnvironmentService } from './session/session-environment-service'
import { DomainTransactionManager } from './storage/domain-transaction'
import type { RuntimeDatabase } from './storage/database'
import { StorageReadOnlyError } from './storage/database'
import {
  RuntimeAccessPolicy,
  type TerminalMessageType
} from './storage/runtime-access-policy'
import {
  WorkspacePathInvalidError,
  WorkspacePathService
} from './hierarchy/workspace-path-service'
import { HierarchyApplicationService } from './hierarchy/hierarchy-application-service'
import { SessionInteractionService } from './session-canvas/session-interaction-service'
import { ProviderModeService } from './session-canvas/provider-mode-service'
import { SessionWorkStatusService } from './session-canvas/session-work-status-service'
import { projectSceneGraphFrom } from './session-canvas/session-graph-repository'
import { PreferenceRepository } from './product/experience-foundation'
import {
  managedWorktreeIdentityExpectation,
  WorktreeHealthService
} from './worktrees/worktree-health-service'
import { WorktreeService, type WorktreeSetupStep } from './worktrees/worktree-service'
import {
  ShellCommandBlockCollector,
  ShellHistoryRepository,
  formatShellHistoryForTerminal
} from './shell-history/shell-history'
import type {
  RecoveryJob,
  RecoveryJobSnapshot
} from './recovery/runtime-session-recovery-scheduler'
import type { RuntimeRecoveryCoordinator } from './recovery/runtime-recovery-coordinator'
import { ProviderConfigStore } from './provider-config/provider-config-store'

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
  iterator: AsyncGenerator<DecodedJournalFrame>
  pendingBytes: Map<number, number>
  unackedBytes: number
  liveSequence: number
  requestedFromSequence: number
  activeSession?: PtySession
  pumping?: boolean
  finishing?: boolean
}

type TerminalSpawnMessage = Extract<RendererMessage, { type: 'terminal.spawn' }>

export interface RuntimeServerOptions {
  providerResumeTimeoutMs?: number
  forkProviderIdentityTimeoutMs?: number
  hudRegistry?: SessionHudRegistry
  accessPolicy?: RuntimeAccessPolicy
  journalOptionsForSession?(sessionId: string): SegmentJournalOptions | undefined
  recoveryCoordinator?: RuntimeRecoveryCoordinator
  controlAssetRoot?: string
  controlNodeExecutable?: string
  providerConfigs?: ProviderConfigStore
}

const REPLAY_HIGH_WATERMARK_BYTES = 1024 * 1024
const REPLAY_LOW_WATERMARK_BYTES = 512 * 1024
const MAX_PENDING_PROVIDER_DERIVATION_BYTES = 1024 * 1024
const DEFAULT_PROVIDER_RESUME_TIMEOUT_MS = 10_000
const DEFAULT_FORK_PROVIDER_IDENTITY_TIMEOUT_MS = 60_000
const execFileAsync = promisify(execFile)

interface InteractiveClaudeLaunch {
  permissionMode: HudPermissionMode
}

interface ProviderRecoveryWaiter {
  resolve(): void
  reject(error: Error): void
}

// Resolving an interactive alias starts the user's login shell and may execute
// a costly zsh configuration. Start it with the Runtime instead of making the
// first Enter on `cc` pay that startup cost. The environment key keeps tests,
// alternate shells and ZDOTDIR-based configurations isolated from each other.
const configuredCcLaunches = new Map<string, Promise<InteractiveClaudeLaunch | undefined>>()

export class RuntimeServer {
  static readonly #instances = new Set<RuntimeServer>()
  readonly #runtimeId = randomUUID()
  readonly #port: RuntimePort
  readonly #sessions: RuntimeSessionRegistry
  readonly #execution: SessionExecutionService<void>
  readonly #attachedSessionIds = new Set<string>()
  readonly #endedSessionIds = new Set<string>()
  readonly #completedReplayThrough = new Map<string, number>()
  readonly #sendToPort = (message: RuntimeMessage) => {
    if (!this.#portClosed) this.#port.postMessage(message)
  }
  readonly #dataRoot: string
  readonly #database: RuntimeDatabase
  readonly #router: RuntimeRpcRouter
  readonly #history: JournalHistoryReader
  readonly #eventStore: DomainEventStore
  readonly #transactions: DomainTransactionManager
  readonly #sessionRepository: SessionRepository
  readonly #forkIntents: SessionForkIntentRepository
  readonly #gitStates: SessionGitStateRepository
  readonly #environments: SessionEnvironmentRepository
  readonly #environmentService: SessionEnvironmentService
  readonly #worktreeHealth: WorktreeHealthService
  readonly #workspacePaths: WorkspacePathService
  readonly #hierarchy: HierarchyApplicationService
  readonly #sessionInteractions: SessionInteractionService
  readonly #providerModes: ProviderModeService
  readonly #workStatuses: SessionWorkStatusService
  readonly #preferences: PreferenceRepository
  readonly #providerConfigs: ProviderConfigStore
  readonly #shellHistory: ShellHistoryRepository
  readonly #cancelledRequests = new Set<string>()
  readonly #subscriptions = new Map<string, { afterSequence: number; batchSize: number }>()
  readonly #replays = new Map<string, ReplayState>()
  readonly #replayRequestGenerations = new Map<string, number>()
  readonly #cwdTimers = new Map<string, ReturnType<typeof setTimeout>>()
  readonly #providerResumeTimers = new Map<string, ReturnType<typeof setTimeout>>()
  readonly #providerLaunchRunIds = new Map<string, string>()
  readonly #providerIdentityMismatches = new Map<string, ProviderIdentityMismatch>()
  readonly #providerRecoveryWaiters = new Map<string, Set<ProviderRecoveryWaiter>>()
  readonly #spawnDescriptors = new Map<string, TerminalSpawnMessage>()
  readonly #environmentResumeDescriptors = new Map<string, TerminalSpawnMessage>()
  readonly #permissionOverrides = new Map<string, HudPermissionMode>()
  readonly #shellInputBuffers = new Map<string, string>()
  readonly #terminalInputTails = new Map<string, Promise<void>>()
  readonly #providerInputBuffers = new Map<string, string>()
  readonly #lastProviderInputs = new Map<string, string>()
  readonly #workStatusTrackers = new Map<string, TerminalWorkStatusTracker>()
  readonly #permissionModeTrackers = new Map<string, ClaudePermissionModeTracker>()
  readonly #summaryBuffers = new Map<string, string>()
  readonly #summaryTimers = new Map<string, ReturnType<typeof setTimeout>>()
  readonly #hudFileWatchers = new Map<string, Map<string, FSWatcher>>()
  readonly #hudFileRefreshTimers = new Map<string, ReturnType<typeof setTimeout>>()
  readonly #skipResumeSessionIds = new Set<string>()
  readonly #providerHooks: ProviderHookServer | undefined
  readonly #providerResumeTimeoutMs: number
  readonly #forkProviderIdentityTimeoutMs: number
  #portClosed = false
  readonly #hud: SessionHudRegistry
  readonly #control:
    | { backend: RuntimeControlBackend; tokens: CapabilityTokenService; endpoint: string }
    | undefined
  readonly #accessPolicy: RuntimeAccessPolicy
  readonly #journalOptionsForSession: NonNullable<RuntimeServerOptions['journalOptionsForSession']>
  readonly #recoveryCoordinator: RuntimeRecoveryCoordinator | undefined
  readonly #controlAssetRoot: string | undefined
  readonly #controlNodeExecutable: string
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
    this.#history = new JournalHistoryReader(dataRoot)
    this.#eventStore = new DomainEventStore(database)
    const transactions = new DomainTransactionManager(database)
    this.#transactions = transactions
    this.#hierarchy = new HierarchyApplicationService(database, transactions)
    this.#sessionInteractions = new SessionInteractionService(database, transactions)
    this.#providerModes = new ProviderModeService(database, transactions)
    this.#workStatuses = new SessionWorkStatusService(database, transactions)
    this.#preferences = new PreferenceRepository(database)
    this.#providerConfigs = options.providerConfigs ?? new ProviderConfigStore(dataRoot)
    this.#shellHistory = new ShellHistoryRepository(database)
    this.#sessionRepository = new SessionRepository(database, new DomainTransactionManager(database))
    this.#forkIntents = new SessionForkIntentRepository(database)
    this.#gitStates = new SessionGitStateRepository(database)
    this.#environments = new SessionEnvironmentRepository(database)
    this.#worktreeHealth = new WorktreeHealthService()
    const worktrees = new WorktreeService(database, transactions, {
      stopRuns: async (runIds) => {
        const sessionIds = runIds.flatMap((runId) => {
          const row = this.#database.get<{ session_id: string }>(
            'SELECT session_id FROM session_runs WHERE id = ?', runId
          )
          return row ? [row.session_id] : []
        })
        for (const sessionId of sessionIds) await this.#pauseForEnvironmentTransition(sessionId)
      }
    })
    this.#environmentService = new SessionEnvironmentService(
      this.#environments,
      {
        restoreOwnedWorktree: async (identity) => {
          const row = this.#database.get<{ setup_policy_json: string }>(
            'SELECT setup_policy_json FROM worktrees WHERE id = ?', identity.worktreeId
          )
          const setupPolicy = row
            ? JSON.parse(row.setup_policy_json) as WorktreeSetupStep[]
            : []
          const operationId = randomUUID()
          await worktrees.create({
            commandId: `session-environment-restore-${identity.sessionId}-${operationId}`,
            commandType: 'session.environment-restore',
            requestHash: `${identity.sessionId}:${identity.worktreeId}:${identity.path}`
          }, {
            id: identity.worktreeId,
            executionContextId: identity.executionContextId,
            workspaceId: identity.workspaceId,
            repositoryRoot: identity.repositoryRoot,
            path: identity.path,
            branch: identity.branch,
            baseRef: identity.baseRef ?? identity.baseRevision ?? 'HEAD',
            setupPolicy,
            now: Date.now()
          })
        },
        pauseSession: (sessionId) => this.#pauseForEnvironmentTransition(sessionId),
        resumeSession: (sessionId) => this.#resumeAfterEnvironmentTransition(sessionId)
      },
      this.#worktreeHealth
    )
    this.#control = control
    this.#sessions = sessions
    this.#execution = new SessionExecutionService(database, sessions, {
      startOrResume: (descriptor, authority, mode, attachView) => this.#spawn({
        ...descriptor,
        type: 'terminal.spawn',
        protocolVersion: PROTOCOL_VERSION
      }, authority, mode, attachView)
    })
    this.#providerHooks = providerHooks
    this.#accessPolicy = options.accessPolicy ?? new RuntimeAccessPolicy(
      database.readOnly ? 'read-only' : 'normal'
    )
  this.#journalOptionsForSession = options.journalOptionsForSession ?? (() => undefined)
  this.#recoveryCoordinator = options.recoveryCoordinator
    this.#controlAssetRoot = options.controlAssetRoot ?? process.env.MATOU_CONTROL_ASSET_ROOT
    this.#controlNodeExecutable = options.controlNodeExecutable ??
      process.env.MATOU_CONTROL_NODE_EXECUTABLE ?? process.execPath
    this.#hud = options.hudRegistry ?? new SessionHudRegistry()
    this.#providerResumeTimeoutMs = positiveTimeout(
      options.providerResumeTimeoutMs,
      DEFAULT_PROVIDER_RESUME_TIMEOUT_MS
    )
    this.#forkProviderIdentityTimeoutMs = positiveTimeout(
      options.forkProviderIdentityTimeoutMs,
      DEFAULT_FORK_PROVIDER_IDENTITY_TIMEOUT_MS
    )
    this.#workspacePaths = workspacePaths
    RuntimeServer.#instances.add(this)
    if (this.#accessPolicy.startBackgroundServices) {
      void configuredCcLaunch()
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
    }
    port.on('message', (event) => {
      void this.#receive(event.data).catch((error) => {
        this.#sendError('INVALID_MESSAGE', errorMessage(error))
      })
    })
    port.on('close', () => {
      this.#disconnectPort()
    })
    port.start()
  }

  flushSemanticEvents(): void {
    if (this.#closed || this.#portClosed) return
    for (const consumerId of this.#subscriptions.keys()) this.#pumpSubscription(consumerId)
  }

  async ensureSessionRunning(job: RecoveryJob): Promise<void> {
    if (this.#closed) throw new Error('Runtime connection closed during Session recovery')
    if (!job.executionContextId || !job.profile) {
      throw new Error(`Session ${job.sessionId} is missing a recovery launch descriptor`)
    }
    const providerRecovery = job.profile === 'claude-code' &&
      this.#sessionRepository.getResumeBinding(job.sessionId, 'claude-code')
      ? this.#waitForProviderRecovery(job.sessionId)
      : undefined
    try {
      await this.#spawnSerialized({
        type: 'terminal.spawn',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: job.sessionId,
        executionContextId: job.executionContextId,
        profile: job.profile,
        cols: 80,
        rows: 24
      }, false)
      if (!this.#sessions.has(job.sessionId) && !this.#hasParkedProviderRestoreFailure(job.sessionId)) {
        throw new Error(`Session ${job.sessionId} did not reach a running state`)
      }
      await providerRecovery?.promise
    } finally {
      providerRecovery?.cancel()
    }
  }

  publishRecovery(snapshot: readonly RecoveryJobSnapshot[]): void {
    if (this.#closed || !this.#handshakeComplete) return
    for (const job of snapshot) {
      this.#port.postMessage({
        type: 'session.recovery-status',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: job.sessionId,
        sceneId: job.sceneId,
        priority: job.priority,
        state: job.state,
        ...(job.error === undefined ? {} : { error: job.error })
      })
    }
  }

  publishRecoverySnapshot(snapshot: readonly RecoveryJobSnapshot[]): void {
    if (this.#closed || !this.#handshakeComplete) return
    this.#port.postMessage({
      type: 'session.recovery-snapshot',
      protocolVersion: PROTOCOL_VERSION,
      statuses: snapshot.map((job) => ({
        sessionId: job.sessionId,
        sceneId: job.sceneId,
        priority: job.priority,
        state: job.state,
        ...(job.error === undefined ? {} : { error: job.error })
      }))
    })
  }

  providerIdentityRecorded(sessionId: string, runId: string): void {
    this.#providerIdentityMismatches.delete(sessionId)
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
    const launchMatches = this.#providerLaunchRunIds.get(sessionId) === runId ||
      this.#sessions.get(sessionId)?.runId === runId
    if (!launchMatches) return
    this.#confirmProviderDerivedOutput(sessionId, runId)
    if (this.#providerResumeTimers.has(sessionId)) {
      this.#clearProviderResumeTimer(sessionId)
      this.#providerLaunchRunIds.delete(sessionId)
      this.#settleProviderRecovery(sessionId)
      return
    }
    this.#settleProviderRecovery(sessionId)
  }

  providerIdentityMismatch(event: ProviderIdentityMismatch): void {
    const activeRunId = this.#providerLaunchRunIds.get(event.sessionId) ??
      this.#sessions.get(event.sessionId)?.runId
    if (activeRunId !== event.runId) return
    this.#rejectProviderDerivedOutput(event.sessionId, event.runId)
    this.#providerIdentityMismatches.set(event.sessionId, event)
    this.#applyProviderIdentityMismatch(event.sessionId)
  }

  startOrResumeSession(
    descriptor: SessionExecutionDescriptor,
    authority?: ForkExecutionAuthorityInput
  ): Promise<SessionExecutionResult<void>> {
    return this.#execution.startOrResume(descriptor.sessionId, descriptor, authority, false)
  }

  close(): void {
    if (this.#closed) return
    for (const timer of this.#summaryTimers.values()) clearTimeout(timer)
    this.#summaryTimers.clear()
    for (const sessionId of this.#summaryBuffers.keys()) this.#flushSessionSummary(sessionId)
    this.#closed = true
    this.#portClosed = true
    RuntimeServer.#instances.delete(this)
    for (const timer of this.#cwdTimers.values()) clearTimeout(timer)
    this.#cwdTimers.clear()
    for (const timer of this.#providerResumeTimers.values()) clearTimeout(timer)
    this.#providerResumeTimers.clear()
    this.#closeAllHudFileWatchers()
    this.#providerLaunchRunIds.clear()
    this.#providerIdentityMismatches.clear()
    this.#rejectAllProviderRecoveries(
      new Error('Runtime connection closed during provider recovery')
    )
    this.#providerInputBuffers.clear()
    this.#lastProviderInputs.clear()
    this.#workStatusTrackers.clear()
    this.#environmentResumeDescriptors.clear()
    this.#permissionModeTrackers.clear()
    this.#detachAll()
    this.#port.close()
  }

  #disconnectPort(): void {
    if (this.#portClosed) return
    this.#portClosed = true
    RuntimeServer.#instances.delete(this)
    this.#subscriptions.clear()
    this.#rejectAllProviderRecoveries(
      new Error('Runtime connection closed during provider recovery')
    )
    this.#detachAll()
  }

  async #receive(rawMessage: unknown): Promise<void> {
    if (this.#closed || this.#portClosed) return
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
        capabilities: this.#accessPolicy.capabilities
      })
      this.publishRecoverySnapshot(this.#recoveryCoordinator?.snapshot() ?? [])
      return
    }

    if (!this.#messageAllowed(message)) return

    switch (message.type) {
      case 'protocol.hello':
        this.#sendError('INVALID_MESSAGE', 'protocol handshake is already complete')
        break
      case 'terminal.spawn':
        await this.#spawnSerialized(message)
        break
      case 'terminal.storage-retry': {
        const session = this.#session(message.sessionId)
        if (!session) break
        try {
          await session.retryDurability()
        } catch {
          // PtySession publishes the authoritative scoped fault again. Keep
          // this connection and every other Session live while the user fixes
          // the storage condition.
        }
        break
      }
      case 'terminal.storage-end': {
        const session = this.#session(message.sessionId)
        if (!session) break
        const sequence = session.lastSequence
        await session.endDurability()
        this.#port.postMessage({
          type: 'terminal.exited', protocolVersion: PROTOCOL_VERSION,
          sessionId: message.sessionId, sequence, exitCode: 1
        })
        this.#setWorkStatus(message.sessionId, 'exited')
        await this.#disposeSession(message.sessionId)
        break
      }
      case 'terminal.view-detach': {
        this.#replayRequestGenerations.set(
          message.sessionId,
          (this.#replayRequestGenerations.get(message.sessionId) ?? 0) + 1
        )
        this.#sessions.get(message.sessionId)?.detach(this.#sendToPort)
        this.#attachedSessionIds.delete(message.sessionId)
        this.#completedReplayThrough.delete(message.sessionId)
        this.#replays.delete(message.sessionId)
        break
      }
      case 'session.recovery-prioritize':
        this.#recoveryCoordinator?.prioritizeScene(
          message.sceneId, message.activeSessionId, message.foregroundSessionIds
        )
        break
      case 'session.recovery-retry':
        this.#recoveryCoordinator?.retry(message.sessionId)
        break
      case 'terminal.user-interaction': {
        // This message is telemetry for task ordering, not terminal control.
        // Shell -> provider promotion briefly swaps the live PTY while the
        // logical Session stays attached, so an interaction arriving in that
        // window must not be surfaced as a Session startup failure.
        const session = this.#attachedSessionIds.has(message.sessionId)
          ? this.#sessions.get(message.sessionId)
          : undefined
        if (!session) break
        const now = Date.now()
        this.#sessionInteractions.record({
          commandId: `terminal-interaction-${message.sessionId}-${randomUUID()}`,
          commandType: 'session.user-interaction',
          requestHash: `${message.sessionId}:${message.interactionKind}:${now}`
        }, {
          sessionId: message.sessionId,
          interactionKind: message.interactionKind,
          ...(message.deferOrdering === undefined ? {} : {
            deferOrdering: message.deferOrdering
          }),
          now
        })
        this.flushSemanticEvents()
        break
      }
      case 'terminal.input':
        await this.#enqueueTerminalInput(message)
        break
      case 'terminal.retry-last-input': {
        const session = this.#session(message.sessionId)
        const lastInput = this.#lastProviderInputs.get(message.sessionId)
        if (!session || session.profile === 'shell' || !lastInput) {
          this.#sendError(
            'INVALID_MESSAGE', '当前会话没有可重试的上一轮输入', message.sessionId
          )
          break
        }
        this.#assertSessionDurabilityHealthy(message.sessionId)
        const now = Date.now()
        this.#sessionInteractions.record({
          commandId: `terminal-retry-${message.sessionId}-${randomUUID()}`,
          commandType: 'session.user-interaction',
          requestHash: `${message.sessionId}:provider-retry:${now}`
        }, {
          sessionId: message.sessionId,
          interactionKind: 'provider-action',
          now
        })
        this.#workStatusTrackers.get(message.sessionId)?.beginAttempt()
        this.#setWorkStatus(message.sessionId, 'running')
        session.write(`${lastInput}\r`)
        this.flushSemanticEvents()
        break
      }
      case 'terminal.resize':
        if (this.#attachedSessionIds.has(message.sessionId)) {
          const descriptor = this.#spawnDescriptors.get(message.sessionId)
          if (descriptor) {
            this.#spawnDescriptors.set(message.sessionId, {
              ...descriptor, cols: message.cols, rows: message.rows
            })
          }
          const session = this.#sessions.get(message.sessionId)
          if (session?.durabilityState === 'healthy') {
            session.resize(message.cols, message.rows)
            this.#port.postMessage({
              type: 'terminal.resized', protocolVersion: PROTOCOL_VERSION,
              sessionId: message.sessionId, resizeId: message.resizeId,
              cols: message.cols, rows: message.rows
            })
          }
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
      case 'terminal.checkpoint':
        await this.#storeCheckpoint(message)
        break
      case 'terminal.hud-refresh':
        if (this.#attachedSessionIds.has(message.sessionId)) {
          await this.refreshSessionHud(message.sessionId)
        }
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

  #messageAllowed(message: RendererMessage): boolean {
    try {
      if (message.type === 'rpc.request') {
        this.#accessPolicy.assertRpcAllowed(message.method)
      } else if (message.type.startsWith('terminal.')) {
        this.#accessPolicy.assertTerminalAllowed(message.type as TerminalMessageType)
      }
      return true
    } catch (error) {
      if (!(error instanceof StorageReadOnlyError)) throw error
      if (message.type === 'rpc.request') {
        this.#sendRpcError(
          message.requestId,
          error.code as Extract<RuntimeMessage, { type: 'rpc.error' }>['code'],
          error.message,
          false
        )
      } else {
        this.#sendError(
          error.code as Extract<RuntimeMessage, { type: 'protocol.error' }>['code'],
          error.message,
          'sessionId' in message ? String(message.sessionId) : undefined
        )
      }
      return false
    }
  }

  #scheduleCwdCapture(session: PtySession): void {
    const pending = this.#cwdTimers.get(session.sessionId)
    if (pending) clearTimeout(pending)
    this.#cwdTimers.set(session.sessionId, setTimeout(() => {
      void this.#captureCwd(session)
      // Capture once more after the prompt has fully settled. Fast commands
      // update the title almost immediately; shell plugins and long wrapped
      // input still converge without requiring another user action.
      this.#cwdTimers.set(session.sessionId, setTimeout(() => {
        this.#cwdTimers.delete(session.sessionId)
        void this.#captureCwd(session)
      }, 1_050))
    }, 180))
  }

  async #enqueueTerminalInput(
    message: Extract<RendererMessage, { type: 'terminal.input' }>
  ): Promise<void> {
    const inputAllowed = this.#workspacePaths.assertSessionInputAllowed(message.sessionId)
      .then(() => undefined, (error: unknown) => error)
    const previous = this.#terminalInputTails.get(message.sessionId) ?? Promise.resolve()
    const current = previous.catch(() => undefined).then(async () => {
      try {
        const inputError = await inputAllowed
        if (inputError !== undefined) throw inputError
        if (this.#closed) return
        const session = this.#session(message.sessionId)
        // The preceding Enter may atomically replace a Shell with Claude while
        // later keystrokes are already queued for the same logical Session.
        // Resolve the live instance at execution time so those bytes reach the
        // replacement instead of being silently dropped with the old object.
        if (!session || session.durabilityState !== 'healthy') return
        if (session && /[\r\n]/.test(message.data)) {
          this.#workStatusTrackers.get(message.sessionId)?.beginAttempt()
          this.#setWorkStatus(message.sessionId, 'running')
        }
        if (session && session.profile !== 'shell') {
          this.#observeProviderInput(message.sessionId, message.data)
        }
        const promoted = session?.profile === 'shell'
          ? await this.#maybePromoteShellAgent(session, message.data)
          : false
        if (!promoted) session?.write(message.data)
        if (session?.profile === 'shell' && /[\r\n]/.test(message.data)) {
          this.#scheduleCwdCapture(session)
        }
      } catch (error) {
        if (error instanceof WorkspacePathInvalidError) {
          this.#sendError(error.code, error.message, message.sessionId)
          return
        }
        throw error
      }
    })
    this.#terminalInputTails.set(message.sessionId, current)
    try {
      await current
    } finally {
      if (this.#terminalInputTails.get(message.sessionId) === current) {
        this.#terminalInputTails.delete(message.sessionId)
      }
    }
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
        await RuntimeServer.#refreshAttachedSessionHud(sessionId)
      }
      const authority = this.#sessionRepository.getSession(sessionId)
      if (!authority || authority.cwd === cwd) return
      const now = Date.now()
      this.#sessionRepository.updateCwd({
        commandId: `runtime-cwd-${sessionId}-${now}-${randomUUID()}`,
        commandType: 'session.cwd-update',
        requestHash: `cwd:${sessionId}:${cwd}:${now}`
      }, sessionId, cwd, now)
      this.flushSemanticEvents()
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

  #publishShellHistory(sessionId: string): void {
    if (!this.#preferences.get('shell.restoreHistoryEnabled')) return
    const blocks = this.#shellHistory.listForLaunch(sessionId, true)
    const restored = formatShellHistoryForTerminal(blocks)
    if (!restored) return
    this.#port.postMessage({
      type: 'terminal.restored-history',
      protocolVersion: PROTOCOL_VERSION,
      sessionId,
      blockCount: blocks.length,
      data: new TextEncoder().encode(restored)
    })
  }

  #flushSessionSummary(sessionId: string): void {
    const raw = this.#summaryBuffers.get(sessionId)
    if (raw === undefined) return
    try {
      // The channel smoke fixture intentionally creates a transient PTY without
      // a durable Session. Only graph-owned Sessions own persisted summaries.
      if (!this.#sessionRepository.getSession(sessionId)) return
      const latestLines = terminalSummaryLines(raw)
      // A restored PTY initially emits only a fresh prompt (and Agent TUIs
      // repaint only their chrome). That output is not newer user content and
      // must not erase the last useful lines from the prior run.
      if (latestLines.length === 0) return
      this.#database.run(
        `INSERT INTO session_graph_summaries (session_id, latest_lines_json, updated_at)
         VALUES (?, ?, ?)
         ON CONFLICT(session_id) DO UPDATE SET
           latest_lines_json = excluded.latest_lines_json,
           updated_at = excluded.updated_at`,
        sessionId, JSON.stringify(latestLines), Date.now()
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
      this.#accessPolicy.assertRpcAllowed(message.method)
      if (permissionSessionId !== undefined) {
        this.#assertProviderMutationAllowed(permissionSessionId)
      }
      let result = ephemeralPermission
        ? {
            sessionId: permissionSessionId,
            permissionMode: textFromRpcInput(message.payload, 'permissionMode'),
            persisted: false
          }
        : isSessionEnvironmentRpc(message.method)
          ? await this.#handleSessionEnvironmentRpc(message.method, message.payload)
          : isTerminalHistoryRpc(message.method)
            ? await this.#handleTerminalHistoryRpc(message.method, message.payload)
            : await this.#router.handle(message.method, message.payload)
      if (message.method === 'provider-config.activate') {
        const input = isRecord(message.payload) ? message.payload : undefined
        const cli = input?.cli === 'claude-code' || input?.cli === 'codex' ? input.cli : undefined
        if (cli === 'claude-code') await RuntimeServer.#restartProviderSessions(cli)
      }
      if (isGitMutation(message.method)) {
        await Promise.all([...this.#attachedSessionIds].map((sessionId) =>
          this.refreshSessionHud(sessionId)
        ))
      }
      await this.#applyHudRpc(message.method, message.payload, beforeHud?.permissionMode)
      const sessionHuds = this.#hud.snapshots()
      if (message.method === 'projection.snapshot') result = withSessionHuds(result, sessionHuds)
      if (message.method === 'hierarchy.get-scene-session-graph') {
        result = withSessionRuntimeEnvironment(result, sessionHuds)
      }
      for (const sessionId of disposedSessionIds(result)) {
        this.#recoveryCoordinator?.cancel([sessionId])
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

  async #handleTerminalHistoryRpc(
    method: Extract<RpcMethod, 'terminal.history-page' | 'terminal.history-search'>,
    payload: unknown
  ): Promise<unknown> {
    const input = recordFromRpc(payload)
    const sessionId = terminalHistorySessionId(input.sessionId)
    const before = terminalHistoryCursor(input.before)
    if (method === 'terminal.history-page') {
      const around = terminalHistoryCursor(input.around)
      if (before && around) {
        throw new RpcFault('INVALID_REQUEST', 'history page accepts before or around, not both')
      }
      const lineLimit = optionalBoundedInteger(input.lineLimit, 'lineLimit', 1, 1_000)
      const beforeLines = optionalBoundedInteger(input.beforeLines, 'beforeLines', 0, 499)
      const afterLines = optionalBoundedInteger(input.afterLines, 'afterLines', 0, 499)
      return this.#history.page({
        sessionId,
        ...(before ? { before } : {}),
        ...(lineLimit === undefined ? {} : { lineLimit }),
        ...(around ? { around } : {}),
        ...(beforeLines === undefined ? {} : { beforeLines }),
        ...(afterLines === undefined ? {} : { afterLines })
      })
    }
    const query = terminalHistoryQuery(input.query)
    const options = terminalHistorySearchOptions(input.options)
    const limit = optionalBoundedInteger(input.limit, 'limit', 1, 1_000)
    return this.#history.search({
      sessionId,
      query,
      options,
      ...(before ? { before } : {}),
      ...(limit === undefined ? {} : { limit })
    })
  }

  async #handleSessionEnvironmentRpc(method: RpcMethod, payload: unknown): Promise<unknown> {
    const direct = isRecord(payload) ? payload : undefined
    const input = rpcInput(payload) ?? direct
    if (!input || typeof input.sessionId !== 'string' || !input.sessionId.trim()) {
      throw new RpcFault('INVALID_REQUEST', 'sessionId is required')
    }
    const sessionId = input.sessionId
    if (method === 'session.environment-open') {
      return this.#environmentService.open(sessionId)
    }
    this.#assertSessionDurabilityHealthy(sessionId)
    const now = typeof input.now === 'number' && Number.isInteger(input.now) && input.now > 0
      ? input.now
      : Date.now()
    return this.#sessions.runExclusive(sessionId, async () => {
      if (method === 'session.environment-restore') {
        return this.#environmentService.restore({ sessionId, now })
      }
      if (method === 'session.environment-locate') {
        if (typeof input.path !== 'string' || !input.path.trim()) {
          throw new RpcFault('INVALID_REQUEST', 'path is required')
        }
        return this.#environmentService.locate({ sessionId, path: input.path, now })
      }
      if (method === 'session.environment-handoff') {
        if (input.target !== 'local' && input.target !== 'worktree') {
          throw new RpcFault('INVALID_REQUEST', 'target must be local or worktree')
        }
        return this.#environmentService.handoff({ sessionId, target: input.target, now })
      }
      throw new RpcFault('INVALID_REQUEST', `unsupported environment method ${method}`)
    })
  }

  async #replay(message: Extract<RendererMessage, { type: 'terminal.replay-request' }>): Promise<void> {
    if (
      !this.#attachedSessionIds.has(message.sessionId) &&
      !this.#database.get('SELECT id FROM sessions WHERE id = ?', message.sessionId)
    ) {
      this.#sendError('SESSION_FORBIDDEN', `session ${message.sessionId} is outside this Renderer capability`)
      return
    }
    const generation = (this.#replayRequestGenerations.get(message.sessionId) ?? 0) + 1
    this.#replayRequestGenerations.set(message.sessionId, generation)
    this.#replays.delete(message.sessionId)
    const isCurrentRequest = () => this.#replayRequestGenerations.get(message.sessionId) === generation
    let detachedSession: PtySession | undefined
    try {
      this.#completedReplayThrough.delete(message.sessionId)
      const activeSession = this.#attachedSessionIds.has(message.sessionId)
        ? this.#sessions.get(message.sessionId)
        : undefined
      activeSession?.detach(this.#sendToPort)
      detachedSession = activeSession
      const metadata = activeSession
        ? await activeSession.replayMetadata(10_000)
        : await readSessionReplayMetadata(this.#dataRoot, message.sessionId, 10_000)
      if (!isCurrentRequest()) return
      const availableFromSequence = metadata.firstSequence
      const liveSequence = metadata.lastSequence
      const tailFromSequence = metadata.tailFromSequence
      const checkpointTerminalWatermark = liveSequence
      const checkpointDomainWatermark = metadata.domainEventSequence
      const candidateCheckpoint = await new CheckpointManager(this.#dataRoot, this.#database).loadLatest(
        message.sessionId,
        {
          terminalSequence: checkpointTerminalWatermark,
          domainEventSequence: checkpointDomainWatermark
        }
      )
      if (!isCurrentRequest()) return
      // A re-attached PTY asks for the beginning of its current run. An older
      // checkpoint cannot replace that prefix: resetting to it and then
      // starting at requestedFrom would leave a visible hole. A newer
      // checkpoint already contains that prefix and is the fastest exact base.
      const checkpoint = !message.preserveExistingModel && checkpointHasGrid(candidateCheckpoint) && (
        message.fromSequence === 0 ||
        candidateCheckpoint.terminalSequence >= message.fromSequence
      ) ? candidateCheckpoint : undefined
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
            cols: checkpoint.cols,
            rows: checkpoint.rows,
            snapshot: checkpoint.snapshot
          }
        }),
        availableFromSequence,
        liveSequence,
        source: checkpoint === undefined ? 'tail' : 'checkpoint',
        fromSequence: checkpoint === undefined
          ? (message.fromSequence === 0
              ? tailFromSequence
              : Math.max(message.fromSequence, availableFromSequence))
          : checkpoint.terminalSequence + 1,
        throughSequence: liveSequence,
        instantLineLimit: 10_000
      })
      const requestedFrom = message.fromSequence === 0
        ? tailFromSequence
        : Math.max(message.fromSequence, availableFromSequence)
      const effectiveFrom = checkpoint === undefined
        ? requestedFrom
        : Math.max(requestedFrom, checkpoint.terminalSequence + 1)
      this.#replays.set(message.sessionId, {
        sessionId: message.sessionId,
        iterator: activeSession
          ? activeSession.iterateFrames({
              fromSequence: effectiveFrom,
              throughSequence: liveSequence
            })
          : iterateSessionFrames(this.#dataRoot, message.sessionId, {
              fromSequence: effectiveFrom,
              throughSequence: liveSequence
            }),
        pendingBytes: new Map(),
        unackedBytes: 0,
        liveSequence,
        requestedFromSequence: message.fromSequence,
        ...(activeSession === undefined ? {} : { activeSession })
      })
      this.#pumpReplay(message.sessionId)
    } catch (error) {
      if (!isCurrentRequest()) return
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

  async #storeCheckpoint(
    message: Extract<RendererMessage, { type: 'terminal.checkpoint' }>
  ): Promise<void> {
    if (
      !this.#attachedSessionIds.has(message.sessionId) &&
      !this.#database.get('SELECT id FROM sessions WHERE id = ?', message.sessionId)
    ) {
      this.#sendError(
        'SESSION_FORBIDDEN',
        `session ${message.sessionId} is outside this Renderer capability`,
        message.sessionId
      )
      return
    }
    try {
      const activeSession = this.#attachedSessionIds.has(message.sessionId)
        ? this.#sessions.get(message.sessionId)
        : undefined
      const metadata = activeSession
        ? await activeSession.replayMetadata(10_000)
        : await readSessionReplayMetadata(
            this.#dataRoot,
            message.sessionId,
            10_000,
            message.throughSequence
          )
      const liveSequence = metadata.lastSequence
      if (message.throughSequence > liveSequence) {
        this.#port.postMessage({
          type: 'terminal.checkpoint-rejected',
          protocolVersion: PROTOCOL_VERSION,
          sessionId: message.sessionId,
          throughSequence: message.throughSequence,
          reason: `checkpoint sequence ${message.throughSequence} exceeds live sequence ${liveSequence}`
        })
        return
      }
      const checkpoints = new CheckpointManager(this.#dataRoot, this.#database)
      await checkpoints.create({
        sessionId: message.sessionId,
        terminalSequence: message.throughSequence,
        domainEventSequence: activeSession
          ? activeSession.domainEventSequenceAtOrBefore(message.throughSequence)
          : metadata.domainEventSequence,
        screenEpoch: message.screenEpoch,
        cols: message.cols,
        rows: message.rows,
        snapshot: new TextEncoder().encode(message.snapshot)
      })
      await activeSession?.protectCheckpointSequences(
        checkpoints.protectedTerminalSequences(message.sessionId)
      )
      this.#port.postMessage({
        type: 'terminal.checkpoint-stored',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: message.sessionId,
        throughSequence: message.throughSequence
      })
    } catch (error) {
      this.#port.postMessage({
        type: 'terminal.checkpoint-rejected',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: message.sessionId,
        throughSequence: message.throughSequence,
        reason: errorMessage(error)
      })
    }
  }

  #acknowledge(sessionId: string, throughSequence: number): void {
    const wasAttached = this.#attachedSessionIds.has(sessionId)
    const session = wasAttached ? this.#sessions.get(sessionId) : undefined
    const replay = this.#replays.get(sessionId)
    if (
      !replay &&
      throughSequence <= (this.#completedReplayThrough.get(sessionId) ?? -1)
    ) return
    if (!session && !replay) {
      if (wasAttached || this.#endedSessionIds.has(sessionId)) return
      this.#sendError('SESSION_FORBIDDEN', `session ${sessionId} is not attached to this connection`)
      return
    }
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
    if (!replay || replay.pumping) return
    replay.pumping = true
    void this.#runReplayPump(replay).catch((error: unknown) => {
      this.#failReplay(replay, error)
    }).finally(() => {
      replay.pumping = false
      if (
        this.#replays.get(sessionId) === replay &&
        !replay.finishing &&
        replay.unackedBytes <= REPLAY_LOW_WATERMARK_BYTES
      ) {
        this.#pumpReplay(sessionId)
      }
    })
  }

  async #runReplayPump(replay: ReplayState): Promise<void> {
    while (this.#replays.get(replay.sessionId) === replay) {
      if (replay.unackedBytes > REPLAY_HIGH_WATERMARK_BYTES) return
      const next = await replay.iterator.next()
      if (next.done) {
        if (!replay.finishing) {
          replay.finishing = true
          await this.#finishReplay(replay)
        }
        return
      }
      const frame = next.value
      if (frame.kind === 'output') {
        this.#port.postMessage({
          type: 'terminal.data', protocolVersion: PROTOCOL_VERSION,
          sessionId: replay.sessionId, sequence: frame.sequence, data: frame.data
        })
        replay.pendingBytes.set(frame.sequence, frame.data.byteLength)
        replay.unackedBytes += frame.data.byteLength
      } else if (frame.kind === 'resize') {
        this.#port.postMessage({
          type: 'terminal.replay-resize', protocolVersion: PROTOCOL_VERSION,
          sessionId: replay.sessionId, sequence: frame.sequence, cols: frame.cols, rows: frame.rows
        })
      } else if (frame.kind === 'reset') {
        this.#port.postMessage({
          type: 'terminal.replay-reset', protocolVersion: PROTOCOL_VERSION,
          sessionId: replay.sessionId, sequence: frame.sequence, screenEpoch: frame.screenEpoch
        })
      } else if (
        frame.kind === 'exit' &&
        (
          replay.activeSession === undefined ||
          frame.sequence >= replay.activeSession.replayFromSequence
        )
      ) {
        // A restarted Session appends a new live run to the same Journal. Exit
        // frames before replayFromSequence belong to older runs and must remain
        // terminal history; reporting one as the current process exit would
        // turn a successfully spawned terminal back into an inert card.
        this.#port.postMessage({
          type: 'terminal.exited', protocolVersion: PROTOCOL_VERSION,
          sessionId: replay.sessionId, sequence: frame.sequence, exitCode: frame.exitCode,
          ...(frame.signal === undefined ? {} : { signal: frame.signal })
        })
      }
    }
  }

  async #finishReplay(replay: ReplayState): Promise<void> {
    if (this.#replays.get(replay.sessionId) !== replay) return
    if (replay.activeSession) {
      const metadata = await replay.activeSession.replayMetadata(10_000)
      // A second Renderer attachment can replace this replay while journal
      // metadata is being read. Only the newest replay may reattach the live
      // PTY or clear replay state; otherwise the replacement is left detached
      // and its terminal stops receiving live output.
      if (this.#replays.get(replay.sessionId) !== replay) return
      if (metadata.lastSequence > replay.liveSequence) {
        const fromSequence = replay.liveSequence + 1
        replay.liveSequence = metadata.lastSequence
        replay.iterator = replay.activeSession.iterateFrames({
          fromSequence,
          throughSequence: replay.liveSequence
        })
        replay.finishing = false
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

  #failReplay(replay: ReplayState, error: unknown): void {
    if (this.#replays.get(replay.sessionId) !== replay) return
    this.#replays.delete(replay.sessionId)
    replay.activeSession?.attach(this.#sendToPort)
    if (error instanceof JournalCorruptionError) {
      this.#port.postMessage({
        type: 'terminal.gap',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: replay.sessionId,
        requestedFromSequence: replay.requestedFromSequence,
        availableFromSequence: 0,
        reason: 'corruption'
      })
      return
    }
    this.#sendError('INTERNAL_ERROR', errorMessage(error), replay.sessionId)
  }

  #pumpSubscription(consumerId: string): void {
    if (this.#portClosed) return
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

  async #spawn(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    forkAuthority?: ForkExecutionAuthority,
    executionMode?: 'attach-only',
    attachView = true
  ): Promise<void> {
    attachView = attachView && !this.#portClosed
    const persistentAuthority = this.#database.get<{
      execution_context_id: string
      cwd: string
      kind: 'shell' | 'claude-code' | 'codex'
      workspace_root: string
    }>(
      `SELECT sessions.execution_context_id, sessions.cwd, sessions.kind,
              workspaces.root_directory AS workspace_root
       FROM sessions
       JOIN tasks ON tasks.id = sessions.task_id
       JOIN workspaces ON workspaces.id = tasks.workspace_id
       WHERE sessions.id = ? AND sessions.archived_at IS NULL`,
      message.sessionId
    )
    if (this.#sessions.has(message.sessionId)) {
      const existing = this.#sessions.get(message.sessionId)!
      if (existing.executionContextId !== message.executionContextId) {
        this.#sendError('SESSION_FORBIDDEN', 'live Session identity does not match the attach request', message.sessionId)
        return
      }
      const previousSpawnRevision = this.#spawnDescriptors.get(message.sessionId)?.spawnRevision ?? 0
      const nextSpawnRevision = message.spawnRevision ?? 0
      if (executionMode === 'attach-only' && existing.profile !== message.profile) {
        this.#sendError(
          'SESSION_FORBIDDEN',
          'an in-flight Fork may only attach its existing provider process',
          message.sessionId
        )
        return
      }
      const revisionReplacement = executionMode !== 'attach-only' &&
        nextSpawnRevision > previousSpawnRevision
      if (existing.profile !== message.profile || revisionReplacement) {
        // A provider-mode transition may deliberately replace the process while keeping
        // the same stable Session.
        // Replace only when persisted domain state already authorizes the exact
        // requested profile; arbitrary Renderer profile changes stay rejected.
        if (persistentAuthority?.kind !== message.profile) {
          this.#sendError('SESSION_FORBIDDEN', 'live Session identity does not match the attach request', message.sessionId)
          return
        }
        this.#clearProviderResumeTimer(message.sessionId)
        this.#sessions.delete(message.sessionId, existing)
        this.#control?.backend.unregister(message.sessionId, existing)
        this.#control?.tokens.revokeRun(existing.runId ?? message.sessionId)
        if (existing.profile !== message.profile) {
          this.#hud.delete(message.sessionId)
          RuntimeServer.#publishAttachedSessionHud(message.sessionId)
        }
        this.#shellInputBuffers.delete(message.sessionId)
        this.#providerInputBuffers.delete(message.sessionId)
        this.#workStatusTrackers.delete(message.sessionId)
        this.#permissionModeTrackers.delete(message.sessionId)
        existing.dispose({ notifyExit: false })
        await existing.whenClosed()
      } else {
        if (!attachView) return
        existing.attach(this.#sendToPort)
        this.#endedSessionIds.delete(message.sessionId)
        this.#completedReplayThrough.delete(message.sessionId)
        this.#attachedSessionIds.add(message.sessionId)
        if (existing.profile === 'shell') this.#publishShellHistory(message.sessionId)
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
    }
    if (executionMode === 'attach-only') {
      this.#sendError(
        'SESSION_FORBIDDEN',
        'the in-flight Fork process is no longer available to attach',
        message.sessionId
      )
      return
    }
    if (
      persistentAuthority !== undefined &&
      persistentAuthority.execution_context_id !== message.executionContextId
    ) {
      this.#sendError(
        'SESSION_FORBIDDEN',
        'persisted Session execution context does not match the attach request',
        message.sessionId
      )
      return
    }
    if (!(await this.#sessionEnvironmentAvailable(message.sessionId))) return
    const contextCwd = message.executionContextId === 'local-default'
      ? undefined
      : this.#database.get<{ cwd: string }>(
          'SELECT cwd FROM execution_contexts WHERE id = ? AND archived_at IS NULL',
          message.executionContextId
        )?.cwd
    const cwd = await firstUsableDirectory([
      persistentAuthority?.cwd,
      persistentAuthority?.workspace_root,
      contextCwd,
      process.env.HOME,
      os.homedir()
    ])
    if (!cwd) {
      this.#sendError('SESSION_FORBIDDEN', 'execution context is not registered', message.sessionId)
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
          this.#sendError(error.code, error.message, message.sessionId)
          return
        }
        throw error
      }
    }

    const forkDecision = message.profile === 'claude-code' && forkAuthority === undefined
      ? this.#forkIntents.claimForLaunch(message.sessionId, Date.now())
      : undefined
    if (forkDecision?.kind === 'failed') {
      await this.#presentForkFailure(message, forkDecision.error, undefined, attachView)
      return
    }
    const forkLaunch = forkAuthority ??
      (forkDecision?.kind === 'launch' ? forkDecision : undefined)
    const skipResume = this.#skipResumeSessionIds.delete(message.sessionId)
    if (
      message.profile === 'claude-code' && forkLaunch === undefined && !skipResume &&
      this.#hasParkedProviderRestoreFailure(message.sessionId)
    ) {
      return
    }
    const resumeBinding = message.profile === 'shell' || skipResume || forkLaunch
      ? undefined
      : this.#sessionRepository.getResumeBinding(message.sessionId, message.profile)
    const providerSessionId = forkLaunch?.sourceProviderSessionId ?? resumeBinding?.providerSessionId
    const supersedesRestoreFailure = message.profile === 'claude-code' &&
      providerSessionId === undefined && Boolean(this.#database.get(
        `SELECT 1 FROM provider_bindings
         WHERE session_id = ? AND provider = 'claude-code'
           AND (restore_state = 'failed' OR resume_state = 'expired')
         LIMIT 1`,
        message.sessionId
      ))
    const persistOrdinaryShellHistory = message.profile === 'shell' &&
      persistentAuthority?.kind === 'shell' &&
      this.#preferences.get('shell.restoreHistoryEnabled')
    if (persistOrdinaryShellHistory && attachView) {
      this.#publishShellHistory(message.sessionId)
    }
    let providerProcessStarted = false
    let hookRegistration: ProviderHookRegistration | undefined
    try {
      const runId = forkAuthority?.runId ?? randomUUID()
      const shellBlockCollector = persistOrdinaryShellHistory
        ? new ShellCommandBlockCollector()
        : undefined
      const cwdTracker = new TerminalCwdTracker()
      const workStatusTracker = message.profile === 'shell' || message.profile === 'claude-code'
        ? new TerminalWorkStatusTracker(
            message.profile === 'claude-code' ? { provider: 'claude-code' } : {}
          )
        : undefined
      const permissionModeTracker = message.profile === 'claude-code'
        ? new ClaudePermissionModeTracker() : undefined
      if (permissionModeTracker) this.#permissionModeTrackers.set(message.sessionId, permissionModeTracker)
      const permissionMode = this.#permissionOverrides.get(message.sessionId) ??
        forkLaunch?.permissionMode ??
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
      const resumeMonitor = providerSessionId === undefined
        ? undefined
        : new ProviderResumeMonitor(providerSessionId)
      let activeSession: PtySession | undefined
      let pendingResumeFailure: string | undefined
      let emittedTerminalOutput = false
      let controlEnvironment: Record<string, string> | undefined
      if (this.#control) {
        const token = this.#control.tokens.issue(
          { runId, sessionId: message.sessionId },
          [
            'host.identify', 'host.list', 'terminal.read-current', 'terminal.read-history',
            'terminal.read-commands', 'terminal.send-text', 'terminal.send-key'
          ],
          Date.now() + 24 * 60 * 60 * 1000
        )
        controlEnvironment = {
          MATOU_CONTROL_ENDPOINT: this.#control.endpoint,
          MATOU_CONTROL_TOKEN: token,
          MATOU_CONTROL_PROTOCOL: '1',
          MATOU_CONTROL_CALLER_SESSION: message.sessionId,
          MATOU_CONTROL_CALLER_RUN: runId,
          MATOU_CONTROL_NODE_EXECUTABLE: this.#controlNodeExecutable,
          ...(this.#controlAssetRoot === undefined ? {} : {
            MATOU_CONTROL_ASSET_ROOT: this.#controlAssetRoot,
            ...prependedPathEnvironment(join(this.#controlAssetRoot, 'bin'), process.env)
          })
        }
      }
      const providerLaunch = message.profile === 'shell'
        ? { env: {} as Record<string, string> }
        : await this.#providerConfigs.launchConfig(message.profile)
      if (message.profile === 'claude-code' && runId && this.#providerHooks) {
        hookRegistration = await this.#providerHooks.registerClaudeSession({
          runId,
          sessionId: message.sessionId,
          // A fresh Claude process that replaces an invalid resume is already live
          // when its statusline arrives. Accept that new identity immediately.
          acceptStatuslineIdentity: providerSessionId !== undefined || supersedesRestoreFailure,
          ...(resumeBinding === undefined || forkLaunch !== undefined
            ? {}
            : { expectedProviderSessionId: resumeBinding.providerSessionId }),
          inheritedConversation: forkLaunch !== undefined,
          ...(forkAuthority === undefined ? {} : { forkAuthority }),
          ...(permissionMode === undefined ? {} : { permissionMode })
        })
        if (providerSessionId !== undefined) {
          this.#providerLaunchRunIds.set(message.sessionId, runId)
        }
      }
      const applyDerivedOutput = (
        data: string,
        options: { deriveWorkStatus?: boolean } = {}
      ) => {
        this.#recordSessionSummary(message.sessionId, data)
        let completedShellBlock = false
        for (const block of shellBlockCollector?.ingest(data) ?? []) {
          if (!this.#closed) {
            try {
              this.#shellHistory.complete({
                sessionId: message.sessionId,
                cwd,
                ...block
              })
              completedShellBlock = true
            } catch (error) {
              // PTY output can race a Runtime teardown by one event-loop turn.
              // History is supplementary; never crash or corrupt the live
              // terminal because storage has already closed.
              if (!/database is closed/i.test(errorMessage(error))) {
                console.error(`[shell.history] ${errorMessage(error)}`)
              }
            }
          }
        }
        // A completed command Block is already a durable user-visible fact.
        // Persist its DAG summary in the same output turn so an immediate app
        // restart cannot restore the Block while leaving its graph card blank.
        if (completedShellBlock) this.#flushSessionSummary(message.sessionId)
        const reportedCwd = cwdTracker.ingest(data)
        if (reportedCwd) void this.#persistCwd(message.sessionId, reportedCwd)
        if (options.deriveWorkStatus !== false) {
          for (const status of workStatusTracker?.ingest(data) ?? []) {
            if (status === 'error') this.#flushSessionSummary(message.sessionId)
            this.#setWorkStatus(message.sessionId, status)
          }
        }
        const visiblePermissionMode = permissionModeTracker?.ingest(data)
        if (visiblePermissionMode) {
          this.#hud.ingestProvider(message.sessionId, { permission_mode: visiblePermissionMode })
          RuntimeServer.#publishAttachedSessionHud(message.sessionId)
        }
      }
      let providerDerivationState: 'pending' | 'confirmed' | 'rejected' =
        message.profile === 'claude-code' && providerSessionId !== undefined && runId !== undefined
          ? 'pending'
          : 'confirmed'
      let pendingProviderOutput = ''
      if (providerDerivationState === 'pending') {
        this.#rejectProviderDerivedOutput(message.sessionId)
        const gatedRunId = runId!
        this.#sessions.markProviderIdentityPending(message.sessionId, gatedRunId, {
          confirm: () => {
            if (providerDerivationState !== 'pending') return
            providerDerivationState = 'confirmed'
            this.#clearProviderResumeTimer(message.sessionId)
            if (this.#providerLaunchRunIds.get(message.sessionId) === gatedRunId) {
              this.#providerLaunchRunIds.delete(message.sessionId)
            }
            this.#settleProviderRecovery(message.sessionId)
            if (workStatusTracker) {
              this.#workStatusTrackers.set(message.sessionId, workStatusTracker)
            }
            const acceptedOutput = pendingProviderOutput
            pendingProviderOutput = ''
            // A resumed provider redraws its conversation before identity is
            // confirmed. Keep that history for the card summary and cwd, but
            // do not reinterpret an old final error as a failure of this new
            // live run. Work status starts deriving from output emitted after
            // the resumed identity is accepted.
            if (acceptedOutput) applyDerivedOutput(acceptedOutput, { deriveWorkStatus: false })
          },
          reject: () => {
            providerDerivationState = 'rejected'
            pendingProviderOutput = ''
          }
        })
      } else if (workStatusTracker) {
        this.#workStatusTrackers.set(message.sessionId, workStatusTracker)
      }
      const session = await PtySession.create({
        sessionId: message.sessionId,
        executionContextId: message.executionContextId,
        cols: message.cols,
        rows: message.rows,
        cwd,
        dataRoot: this.#dataRoot,
        profile: message.profile,
        journalOptions: this.#journalOptions(message.sessionId),
        ...(providerSessionId === undefined ? {} : { providerSessionId }),
        ...(forkLaunch === undefined ? {} : { forkSession: true }),
        ...(permissionMode === undefined ? {} : { permissionMode }),
        ...(hookRegistration === undefined ? {} : {
          settingsPath: hookRegistration.settingsPath
        }),
        ...(this.#controlAssetRoot === undefined ? {} : {
          controlAssetRoot: this.#controlAssetRoot
        }),
        ...(providerLaunch.model === undefined ? {} : { model: providerLaunch.model }),
        ...(runId === undefined ? {} : { runId }),
        ...((controlEnvironment === undefined && Object.keys(providerLaunch.env).length === 0) ? {} : {
          env: { ...providerLaunch.env, ...controlEnvironment }
        }),
        ...(attachView ? { send: this.#sendToPort } : {}),
        onOutput: (data) => {
          emittedTerminalOutput = true
          if (providerDerivationState === 'pending') {
            pendingProviderOutput = (pendingProviderOutput + data)
              .slice(-MAX_PENDING_PROVIDER_DERIVATION_BYTES)
          } else if (providerDerivationState === 'confirmed') {
            applyDerivedOutput(data)
          }
          const resumeFailure = resumeMonitor?.ingest(data)
          if (resumeFailure) {
            pendingResumeFailure = resumeFailure
            if (activeSession && forkLaunch) {
              hookRegistration?.retire()
              this.#beginForkFailure(message, activeSession, resumeFailure, forkAuthority)
            } else if (activeSession && resumeBinding) {
              hookRegistration?.retire()
              this.#parkResumeFailure(message, activeSession, resumeBinding.id, resumeFailure)
            }
          } else if (resumeMonitor?.isSettled || resumeMonitor?.hasVisibleOutput) {
            // A full provider screen is the fallback readiness signal when a
            // Claude release delays or omits its statusline HTTP hook. Visible
            // provider content can also be an interactive trust prompt, so the
            // card must become operable before the user can answer it.
            this.#settleProviderRecovery(message.sessionId)
          }
        },
        onExit: (exited, exitCode, signal, exitReason) => {
          this.#rejectProviderDerivedOutput(message.sessionId, exited.runId)
          this.#flushSessionSummary(message.sessionId)
          this.#clearProviderResumeTimer(message.sessionId)
          const providerIdentityConfirmed = exited.runId !== undefined &&
            this.#sessions.providerIdentityConfirmed(exited.runId)
          this.#forgetProviderLaunch(message.sessionId, exited.runId)
          hookRegistration?.retire()
          if (exitReason === 'runtime-shutdown' || exitReason === 'environment-transition') {
            this.#sessions.delete(message.sessionId, exited)
            this.#control?.backend.unregister(message.sessionId, exited)
            this.#control?.tokens.revokeRun(exited.runId ?? message.sessionId)
            if (exitReason === 'environment-transition') return false
            const workStatus = this.#database.get<{ work_status: string }>(
              'SELECT work_status FROM sessions WHERE id = ?',
              message.sessionId
            )?.work_status
            const preserveInterruptedRun = exited.profile === 'shell' &&
              (workStatus === 'starting' || workStatus === 'running' || workStatus === 'needs-input')
            if (exited.runId && !preserveInterruptedRun) {
              try {
                this.#sessionRepository.finishRun(
                  {
                    commandId: `runtime-shutdown-${exited.runId}`,
                    commandType: 'session.run-exit',
                    requestHash: `shutdown:${exited.runId}:${exited.pid}`
                  },
                  exited.runId,
                  { exitCode, ...(signal === undefined ? {} : { signal }), now: Date.now() }
                )
              } catch (error) {
                console.error(`[session.run-shutdown] ${errorMessage(error)}`)
              }
            }
            return false
          }
          const resumeExitFallback = Boolean(
            resumeBinding &&
            !providerIdentityConfirmed &&
            resumeMonitor?.isMonitoring &&
            this.#sessions.get(message.sessionId) === exited &&
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
            if (this.#failFork(message.sessionId, reason, forkAuthority)) {
              forkFailure = true
              void this.#appendForkExitFailure(message.sessionId, reason, exited.lastSequence + 1)
            }
          }
          const naturalAgentFallback = wasCurrent && exited.profile !== 'shell' &&
            !resumeExitFallback && (!forkLaunch || forkState === 'succeeded')
          const shellStartupFailure = wasCurrent && exited.profile === 'shell' &&
            !emittedTerminalOutput && exitReason !== 'runtime-shutdown'
          if (wasCurrent) {
            if (!resumeExitFallback && !naturalAgentFallback) {
              this.#attachedSessionIds.delete(message.sessionId)
              this.#endedSessionIds.add(message.sessionId)
            }
            this.#control?.backend.unregister(message.sessionId, exited)
            this.#control?.tokens.revokeRun(exited.runId ?? message.sessionId)
            this.#hud.exit(message.sessionId, {
              fallbackToShell: exited.profile !== 'shell' && !forkLaunch && !resumeExitFallback
            })
            RuntimeServer.#publishAttachedSessionHud(message.sessionId)
            if (!resumeExitFallback && !naturalAgentFallback) {
              this.#spawnDescriptors.delete(message.sessionId)
            }
            this.#shellInputBuffers.delete(message.sessionId)
            this.#providerInputBuffers.delete(message.sessionId)
            this.#workStatusTrackers.delete(message.sessionId)
            this.#permissionModeTrackers.delete(message.sessionId)
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
              this.#setWorkStatus(message.sessionId, shellStartupFailure ? 'error' : 'exited')
            } catch (error) {
              console.error(`[session.run-exit] ${errorMessage(error)}`)
            }
          }
          if (shellStartupFailure) {
            const executable = process.env.SHELL ?? '系统默认 Shell'
            const termination = signal === undefined
              ? `退出代码 ${exitCode}`
              : `信号 ${signal}`
            this.#sendError(
              'INTERNAL_ERROR',
              `Shell 进程启动失败：${executable} 未产生可用输出并退出（${termination}）`,
              message.sessionId
            )
            return false
          }
          if (resumeExitFallback) {
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
              void this.#spawnSerialized({ ...message, profile: 'shell' }, attachView)
            } catch (error) {
              try {
                this.#sessionRepository.returnAgentToShell({
                  commandId: `runtime-agent-return-shell-legacy-${message.sessionId}-${now}-${randomUUID()}`,
                  commandType: 'session.agent-return-shell',
                  requestHash: `agent-return-shell-legacy:${message.sessionId}:${now}`
                }, message.sessionId, now)
                void this.#spawnSerialized({ ...message, profile: 'shell' }, attachView)
              } catch (fallbackError) {
                console.error(`[session.agent-return-shell] ${errorMessage(fallbackError)}`)
              }
            }
            return false
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
      if (this.#applyProviderIdentityMismatch(message.sessionId)) return
      this.#endedSessionIds.delete(message.sessionId)
      this.#completedReplayThrough.delete(message.sessionId)
      if (attachView) this.#attachedSessionIds.add(message.sessionId)
      this.#control?.backend.register(message.sessionId, session)
      if (attachView) {
        this.#port.postMessage({
          type: 'terminal.spawned',
          protocolVersion: PROTOCOL_VERSION,
          sessionId: message.sessionId,
          pid: session.pid
        })
      }
      this.publishSessionHud(message.sessionId)
      void this.refreshSessionHud(message.sessionId)
      if (pendingResumeFailure && forkLaunch) {
        this.#beginForkFailure(message, session, pendingResumeFailure, forkAuthority)
      } else if (pendingResumeFailure && resumeBinding) {
        this.#parkResumeFailure(message, session, resumeBinding.id, pendingResumeFailure)
      } else if (forkLaunch && forkAuthority) {
        this.#scheduleForkProviderIdentityTimeout(message, session, forkAuthority)
      } else if (resumeMonitor && resumeBinding) {
        this.#scheduleProviderResumeTimeout(message, session, resumeBinding.id, resumeMonitor)
      }
    } catch (error) {
      this.#rejectProviderDerivedOutput(message.sessionId)
      await hookRegistration?.dispose()
      if (forkLaunch && !providerProcessStarted) {
        const reason = `Fork 会话进程启动失败：${errorMessage(error)}`
        await this.#presentForkFailure(message, reason, forkAuthority, attachView)
        return
      }
      if (resumeBinding && !providerProcessStarted) {
        if (this.#markResumeFailed(
          message.sessionId,
          resumeBinding.id,
          `provider process could not start: ${errorMessage(error)}`
        )) {
          this.#settleProviderRecovery(message.sessionId, new Error(errorMessage(error)))
          return
        }
      }
      this.#sendError('INTERNAL_ERROR', errorMessage(error), message.sessionId)
    }
  }

  #beginForkFailure(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    session: PtySession,
    reason: string,
    authority?: ForkExecutionAuthority
  ): void {
    if (this.#sessions.get(message.sessionId) !== session) return
    this.#rejectProviderDerivedOutput(message.sessionId, session.runId)
    this.#clearProviderResumeTimer(message.sessionId)
    if (!this.#failFork(message.sessionId, reason, authority)) {
      this.#sessions.delete(message.sessionId, session)
      this.#control?.backend.unregister(message.sessionId, session)
      this.#control?.tokens.revokeRun(session.runId ?? message.sessionId)
      session.dispose({ notifyExit: false })
      return
    }
    session.display('\r\n\u001b[33m[Fork 未完成，请检查上方原因后重试]\u001b[0m\r\n')
    this.#sessions.delete(message.sessionId, session)
    this.#control?.backend.unregister(message.sessionId, session)
    this.#control?.tokens.revokeRun(session.runId ?? message.sessionId)
    session.dispose({ notifyExit: false })
  }

  async #presentForkFailure(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    reason: string,
    authority?: ForkExecutionAuthority,
    attachView = true
  ): Promise<void> {
    if (!this.#failFork(message.sessionId, reason, authority)) return
    const banner = '[Fork 未完成，请检查上方原因后重试]'
    const tail = await this.#history.page({
      sessionId: message.sessionId,
      lineLimit: 16
    }).catch(() => ({ lines: [] }))
    const alreadyPresented = tail.lines.some((line) => line.text.includes(banner))
    if (!alreadyPresented) {
      const journal = await SegmentJournal.open(
        this.#dataRoot,
        message.sessionId,
        this.#journalOptions(message.sessionId)
      )
      const sequence = journal.lastSequence + 1
      await journal.appendOutput(sequence, new TextEncoder().encode(
        `\r\n\u001b[31m${reason}\u001b[0m\r\n` +
        `\u001b[33m${banner}\u001b[0m\r\n`
      ))
      await journal.close()
    }
    if (attachView) {
      this.#attachedSessionIds.add(message.sessionId)
      this.#port.postMessage({
        type: 'terminal.spawned', protocolVersion: PROTOCOL_VERSION,
        sessionId: message.sessionId, pid: 0, reattached: true, replayFromSequence: 0
      })
    }
    this.#hud.exit(message.sessionId, { fallbackToShell: false })
    this.publishSessionHud(message.sessionId)
  }

  #failFork(
    sessionId: string,
    reason: string,
    authority?: ForkExecutionAuthority
  ): boolean {
    const now = Date.now()
    const operation = authority
      ? this.#forkIntents.operationById(authority.operationId)
      : this.#forkIntents.nonTerminalBySession(sessionId)
    if (!operation || operation.identity.sessionId !== sessionId) {
      if (authority !== undefined) {
        return this.#forkIntents.failOperation({
          operationId: authority.operationId,
          lease: authority.lease,
          error: reason,
          now
        }).kind === 'applied'
      }
      this.#forkIntents.fail(sessionId, reason, now)
      return true
    }
    const commandId = `runtime-fork-failed:${operation.identity.operationId}:${randomUUID()}`
    const applied = this.#transactions.execute({
      commandId,
      commandType: 'session.fork-failed',
      requestHash: `${operation.identity.operationId}:${reason}:${now}`
    }, ({ tx, emit }) => {
      const failed = authority
        ? this.#forkIntents.failOperation({
            operationId: authority.operationId,
            lease: authority.lease,
            error: reason,
            now
          }, tx).kind === 'applied'
        : this.#forkIntents.fail(sessionId, reason, now, tx)
      if (!failed) return false
      tx.run(
        `UPDATE sessions SET status = 'interrupted', updated_at = ?,
           version = version + 1 WHERE id = ?`,
        now, sessionId
      )
      const owner = tx.get<{ task_id: string; workspace_id: string }>(
        `SELECT sessions.task_id, tasks.workspace_id
         FROM sessions JOIN tasks ON tasks.id = sessions.task_id
         WHERE sessions.id = ?`,
        sessionId
      )
      emit({
        eventId: `${commandId}:graph`,
        eventType: 'session.fork-failed',
        aggregateType: 'session',
        aggregateId: sessionId,
        ...(owner === undefined ? {} : {
          workspaceId: owner.workspace_id,
          taskId: owner.task_id
        }),
        sessionId,
        payload: {
          error: reason,
          graph: projectSceneGraphFrom(tx, operation.sceneId, operation.windowId)
        },
        occurredAt: now
      })
      return true
    }).result
    if (applied) this.flushSemanticEvents()
    return applied
  }

  async #appendForkExitFailure(sessionId: string, reason: string, sequence: number): Promise<void> {
    try {
      const banner = '[Fork 未完成，请检查上方原因后重试]'
      const journal = await SegmentJournal.open(
        this.#dataRoot,
        sessionId,
        this.#journalOptions(sessionId)
      )
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

  #journalOptions(sessionId: string): SegmentJournalOptions {
    const configured = this.#journalOptionsForSession(sessionId)
    const retained = new CheckpointManager(
      this.#dataRoot,
      this.#database
    ).protectedTerminalSequences(sessionId)
    return {
      ...configured,
      checkpointProtectedSequences: [
        ...(configured?.checkpointProtectedSequences ?? []),
        ...retained
      ]
    }
  }

  #parkResumeFailure(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    session: PtySession,
    bindingId: string,
    reason: string
  ): void {
    if (this.#sessions.get(message.sessionId) !== session) return
    this.#rejectProviderDerivedOutput(message.sessionId, session.runId)
    this.#clearProviderResumeTimer(message.sessionId)
    this.#settleProviderRecovery(message.sessionId, new Error(reason))
    if (!this.#markResumeFailed(message.sessionId, bindingId, reason)) return
    this.#sessions.delete(message.sessionId, session)
    this.#control?.backend.unregister(message.sessionId, session)
    this.#control?.tokens.revokeRun(session.runId ?? message.sessionId)
    this.#attachedSessionIds.delete(message.sessionId)
    this.#spawnDescriptors.delete(message.sessionId)
    this.#shellInputBuffers.delete(message.sessionId)
    this.#providerInputBuffers.delete(message.sessionId)
    this.#workStatusTrackers.delete(message.sessionId)
    this.#hud.exit(message.sessionId, { fallbackToShell: false })
    this.publishSessionHud(message.sessionId)
    session.dispose({ notifyExit: false })
  }

  #applyProviderIdentityMismatch(sessionId: string): boolean {
    const mismatch = this.#providerIdentityMismatches.get(sessionId)
    const session = this.#sessions.get(sessionId)
    const message = this.#spawnDescriptors.get(sessionId)
    if (!mismatch || !session || !message || session.runId !== mismatch.runId) return false
    const binding = this.#database.get<{ id: string }>(
      `SELECT id FROM provider_bindings
       WHERE session_id = ? AND provider = 'claude-code' AND provider_session_id = ?
         AND resume_state NOT IN ('failed', 'expired')
       ORDER BY updated_at DESC, id DESC LIMIT 1`,
      sessionId,
      mismatch.expectedProviderSessionId
    )
    this.#providerIdentityMismatches.delete(sessionId)
    if (!binding) return false
    this.#parkResumeFailure(
      message,
      session,
      binding.id,
      'Claude Code 返回的会话与待恢复会话不一致，请重试恢复'
    )
    this.flushSemanticEvents()
    return true
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
      if (this.#sessions.get(message.sessionId) !== session) return
      if (!reason) {
        if (monitor.isSettled) this.#settleProviderRecovery(message.sessionId)
        return
      }
      this.#parkResumeFailure(message, session, bindingId, reason)
    }, this.#providerResumeTimeoutMs))
  }

  #scheduleForkProviderIdentityTimeout(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    session: PtySession,
    authority: ForkExecutionAuthority
  ): void {
    if (this.#consumeProviderIdentityConfirmation(message.sessionId, session.runId)) return
    this.#clearProviderResumeTimer(message.sessionId)
    this.#providerResumeTimers.set(message.sessionId, setTimeout(() => {
      this.#providerResumeTimers.delete(message.sessionId)
      if (this.#sessions.get(message.sessionId) !== session) return
      this.#beginForkFailure(
        message,
        session,
        'Fork 会话身份确认超时，请重试',
        authority
      )
      this.flushSemanticEvents()
    }, this.#forkProviderIdentityTimeoutMs))
  }

  #clearProviderResumeTimer(sessionId: string): void {
    const timer = this.#providerResumeTimers.get(sessionId)
    if (!timer) return
    clearTimeout(timer)
    this.#providerResumeTimers.delete(sessionId)
  }

  #consumeProviderIdentityConfirmation(sessionId: string, runId: string | undefined): boolean {
    if (!runId || !this.#sessions.providerIdentityConfirmed(runId)) return false
    if (this.#providerLaunchRunIds.get(sessionId) === runId) {
      this.#providerLaunchRunIds.delete(sessionId)
    }
    return true
  }

  #confirmProviderDerivedOutput(sessionId: string, runId: string): void {
    this.#sessions.confirmProviderIdentity(sessionId, runId)
  }

  #rejectProviderDerivedOutput(sessionId: string, runId?: string): void {
    this.#sessions.rejectProviderIdentity(sessionId, runId)
  }

  #forgetProviderLaunch(sessionId: string, runId: string | undefined): void {
    this.#providerIdentityMismatches.delete(sessionId)
    this.#sessions.forgetProviderIdentity(sessionId, runId)
    if (!runId) return
    if (this.#providerLaunchRunIds.get(sessionId) === runId) {
      this.#providerLaunchRunIds.delete(sessionId)
    }
  }

  #waitForProviderRecovery(sessionId: string): {
    promise: Promise<void>
    cancel(): void
  } {
    let waiter: ProviderRecoveryWaiter
    let settled = false
    const promise = new Promise<void>((resolve, reject) => {
      waiter = {
        resolve: () => {
          if (settled) return
          settled = true
          resolve()
        },
        reject: (error) => {
          if (settled) return
          settled = true
          reject(error)
        }
      }
    })
    // The provider can fail while its PTY is still being constructed. Attach a
    // handler immediately; ensureSessionRunning awaits the original promise
    // once spawning settles and still receives the same rejection.
    void promise.catch(() => undefined)
    const waiters = this.#providerRecoveryWaiters.get(sessionId) ?? new Set()
    waiters.add(waiter!)
    this.#providerRecoveryWaiters.set(sessionId, waiters)
    return {
      promise,
      cancel: () => {
        waiters.delete(waiter!)
        if (waiters.size === 0) this.#providerRecoveryWaiters.delete(sessionId)
      }
    }
  }

  #settleProviderRecovery(sessionId: string, error?: Error): void {
    const waiters = this.#providerRecoveryWaiters.get(sessionId)
    if (!waiters) return
    this.#providerRecoveryWaiters.delete(sessionId)
    for (const waiter of waiters) {
      if (error) waiter.reject(error)
      else waiter.resolve()
    }
  }

  #rejectAllProviderRecoveries(error: Error): void {
    const sessionIds = [...this.#providerRecoveryWaiters.keys()]
    for (const sessionId of sessionIds) this.#settleProviderRecovery(sessionId, error)
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
        this.#sessionRepository.failResume(
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

  async #sessionEnvironmentAvailable(sessionId: string): Promise<boolean> {
    const binding = this.#environments.get(sessionId)
    if (!binding) return true
    if (binding.state !== 'ready') {
      this.#sendError(
        'SESSION_ENVIRONMENT_UNAVAILABLE',
        '该会话的运行目录正在恢复或需要重新定位，请先处理运行环境后继续',
        sessionId
      )
      return false
    }
    if (binding.activeTarget === 'local') return true

    const worktreeId = binding.managedWorktreeId
    const worktree = worktreeId === undefined
      ? undefined
      : this.#database.get<{
          repository_root: string
          worktree_path: string
          branch_name: string
          base_revision: string | null
          state: string
        }>(
          `SELECT repository_root, worktree_path, branch_name, base_revision, state
           FROM worktrees WHERE id = ?`,
          worktreeId
        )
    if (!worktree || !['ready', 'dirty', 'retained'].includes(worktree.state)) {
      this.#markEnvironmentUnavailable(
        sessionId,
        worktreeId,
        'failed',
        `worktree-state:${worktree?.state ?? 'missing'}`
      )
      this.#sendError(
        'SESSION_ENVIRONMENT_UNAVAILABLE',
        '该会话的 Worktree 当前不可用，请恢复、重新定位或切换到 Local 后继续',
        sessionId
      )
      return false
    }

    let health
    try {
      health = await this.#worktreeHealth.check(managedWorktreeIdentityExpectation({
        repositoryRoot: worktree.repository_root,
        path: worktree.worktree_path,
        branch: worktree.branch_name,
        baseRevision: worktree.base_revision
      }))
    } catch (error) {
      this.#markEnvironmentUnavailable(
        sessionId,
        worktreeId,
        'failed',
        `health-check-failed:${errorMessage(error)}`
      )
      this.#sendError(
        'SESSION_ENVIRONMENT_UNAVAILABLE',
        '该会话的 Worktree 当前不可用，请恢复、重新定位或切换到 Local 后继续',
        sessionId
      )
      return false
    }
    if (health.kind === 'ready') return true

    this.#markEnvironmentUnavailable(
      sessionId,
      worktreeId,
      health.kind === 'missing' ? 'missing' : 'failed',
      `${health.kind}:${health.reason}`
    )
    this.#sendError(
      'SESSION_ENVIRONMENT_UNAVAILABLE',
      '该会话的 Worktree 当前不可用，请恢复、重新定位或切换到 Local 后继续',
      sessionId
    )
    return false
  }

  #markEnvironmentUnavailable(
    sessionId: string,
    worktreeId: string | undefined,
    state: 'missing' | 'failed',
    reason: string
  ): void {
    const now = Date.now()
    const operationId = randomUUID()
    this.#transactions.execute({
      commandId: `runtime-worktree-health-${sessionId}-${operationId}`,
      commandType: 'session.environment-health-degraded',
      requestHash: `${sessionId}:${worktreeId ?? 'none'}:${state}:${reason}`
    }, ({ tx, emit }) => {
      if (worktreeId !== undefined) {
        tx.run("UPDATE worktrees SET state = 'failed', updated_at = ? WHERE id = ?", now, worktreeId)
      }
      if (state === 'missing') this.#environments.markMissing(sessionId, reason, now, tx)
      else this.#environments.markFailed(sessionId, reason, now, tx)
      emit({
        eventId: `runtime-worktree-health-${sessionId}-${operationId}`,
        eventType: 'session.environment-degraded',
        aggregateType: 'session',
        aggregateId: sessionId,
        sessionId,
        payload: { sessionId, worktreeId, state, reason },
        occurredAt: now
      })
      return null
    })
    this.flushSemanticEvents()
  }

  #hasParkedProviderRestoreFailure(sessionId: string): boolean {
    return this.#database.get<{ restore_state: string }>(
      `SELECT restore_state FROM provider_bindings
       WHERE session_id = ? AND provider = 'claude-code'
       ORDER BY updated_at DESC, id DESC LIMIT 1`,
      sessionId
    )?.restore_state === 'failed'
  }

  async #spawnSerialized(
    message: Extract<RendererMessage, { type: 'terminal.spawn' }>,
    attachView = true
  ): Promise<void> {
    await this.#execution.startOrResume(message.sessionId, message, undefined, attachView)
  }

  #session(sessionId: string): PtySession | undefined {
    const session = this.#attachedSessionIds.has(sessionId) ? this.#sessions.get(sessionId) : undefined
    if (!session) {
      this.#sendError(
        'SESSION_FORBIDDEN',
        `session ${sessionId} is not attached to this connection`,
        sessionId
      )
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
    this.#providerInputBuffers.delete(sessionId)
    this.#lastProviderInputs.delete(sessionId)
    this.#workStatusTrackers.delete(sessionId)
    this.#environmentResumeDescriptors.delete(sessionId)
    this.#permissionModeTrackers.delete(sessionId)
    this.#skipResumeSessionIds.delete(sessionId)
    this.#closeHudFileWatchers(sessionId)
    this.#hud.delete(sessionId)
    this.publishSessionHud(sessionId)
    this.#attachedSessionIds.delete(sessionId)
    session.dispose()
    await session.whenClosed()
  }

  async #pauseForEnvironmentTransition(sessionId: string): Promise<void> {
    this.#clearProviderResumeTimer(sessionId)
    const cwdTimer = this.#cwdTimers.get(sessionId)
    if (cwdTimer) clearTimeout(cwdTimer)
    this.#cwdTimers.delete(sessionId)
    const descriptor = this.#spawnDescriptors.get(sessionId)
    if (descriptor) this.#environmentResumeDescriptors.set(sessionId, descriptor)
    const session = this.#sessions.get(sessionId)
    if (!session) return

    if (session.runId) {
      const run = this.#database.get<{ status: string }>(
        'SELECT status FROM session_runs WHERE id = ?', session.runId
      )
      if (run?.status === 'starting' || run?.status === 'running') {
        const now = Date.now()
        this.#sessionRepository.interruptRun({
          commandId: `session-environment-interrupt-${session.runId}-${now}`,
          commandType: 'session.environment-run-interrupted',
          requestHash: `${sessionId}:${session.runId}:${now}`
        }, session.runId, now)
      }
    }

    this.#sessions.delete(sessionId, session)
    this.#control?.backend.unregister(sessionId, session)
    this.#control?.tokens.revokeRun(session.runId ?? sessionId)
    session.dispose({ notifyExit: false, reason: 'environment-transition' })
    await session.whenClosed()

    this.#attachedSessionIds.delete(sessionId)
    this.#endedSessionIds.delete(sessionId)
    this.#completedReplayThrough.delete(sessionId)
    this.#spawnDescriptors.delete(sessionId)
    this.#permissionOverrides.delete(sessionId)
    this.#shellInputBuffers.delete(sessionId)
    this.#providerInputBuffers.delete(sessionId)
    this.#lastProviderInputs.delete(sessionId)
    this.#workStatusTrackers.delete(sessionId)
    this.#skipResumeSessionIds.delete(sessionId)
    this.#hud.delete(sessionId)
    this.publishSessionHud(sessionId)
  }

  async #resumeAfterEnvironmentTransition(sessionId: string): Promise<void> {
    const descriptor = this.#environmentResumeDescriptors.get(sessionId)
    if (!descriptor) return
    const authority = this.#database.get<{
      execution_context_id: string
      kind: 'shell' | 'claude-code' | 'codex'
    }>('SELECT execution_context_id, kind FROM sessions WHERE id = ? AND archived_at IS NULL', sessionId)
    if (!authority) {
      this.#environmentResumeDescriptors.delete(sessionId)
      return
    }
    await this.#spawn({
      ...descriptor,
      executionContextId: authority.execution_context_id,
      profile: authority.kind,
      spawnRevision: (descriptor.spawnRevision ?? 0) + 1
    })
    this.#environmentResumeDescriptors.delete(sessionId)
  }

  #detachAll(): void {
    this.#workspacePaths.stopPolling()
    this.#closeAllHudFileWatchers()
    for (const sessionId of this.#attachedSessionIds) {
      this.#sessions.get(sessionId)?.detach(this.#sendToPort)
    }
    this.#attachedSessionIds.clear()
    this.#endedSessionIds.clear()
    this.#completedReplayThrough.clear()
    this.#replays.clear()
    this.#replayRequestGenerations.clear()
  }

  #sendError(
    code: Extract<RuntimeMessage, { type: 'protocol.error' }>['code'],
    message: string,
    sessionId?: string
  ): void {
    if (this.#portClosed) return
    console.error(`[protocol.error] ${code}: ${message}`)
    this.#port.postMessage({
      type: 'protocol.error',
      protocolVersion: PROTOCOL_VERSION,
      code,
      message,
      ...(sessionId === undefined ? {} : { sessionId })
    })
  }

  publishSessionHud(sessionId: string): void {
    if (this.#closed || this.#portClosed || !this.#attachedSessionIds.has(sessionId)) return
    this.#port.postMessage({
      type: 'terminal.hud', protocolVersion: PROTOCOL_VERSION,
      sessionId, hud: this.#hud.snapshot(sessionId) ?? null
    })
  }

  static #publishAttachedSessionHud(sessionId: string): void {
    for (const server of RuntimeServer.#instances) server.publishSessionHud(sessionId)
  }

  static #flushSemanticEventsForAll(): void {
    for (const server of RuntimeServer.#instances) server.flushSemanticEvents()
  }

  static async #refreshAttachedSessionHud(sessionId: string): Promise<void> {
    await Promise.all([...RuntimeServer.#instances].map((server) =>
      server.refreshSessionHud(sessionId)
    ))
  }

  async refreshSessionHud(sessionId: string): Promise<void> {
    if (this.#closed) return
    if (this.#forkIntents.state(sessionId) === 'succeeded') {
      this.#clearProviderResumeTimer(sessionId)
    }
    const current = this.#hud.snapshot(sessionId)
    if (!current?.cwd) {
      this.publishSessionHud(sessionId)
      return
    }
    const context = (() => {
      try {
        return this.#database.get<{ execution_context_id: string; cwd: string }>(
          `SELECT sessions.execution_context_id, execution_contexts.cwd
           FROM sessions
           JOIN execution_contexts ON execution_contexts.id = sessions.execution_context_id
           WHERE sessions.id = ?`, sessionId
        )
      } catch (error) {
        if (this.#closed || /database is closed/i.test(errorMessage(error))) return undefined
        throw error
      }
    })()
    let git: LiveGitEnvironment | undefined
    let gitDirectory: string | undefined
    let probedCurrentContext = false
    if (context && !this.#database.readOnly) {
      const persisted = await this.#gitStates.refresh(context.execution_context_id).catch((error) => {
        if (this.#closed || /database is closed/i.test(errorMessage(error))) return undefined
        throw error
      })
      if (!persisted) return
      if (current.cwd === context.cwd) {
        probedCurrentContext = true
        if (persisted.git.state === 'ready') {
          git = {
            gitBranch: persisted.git.branch ?? persisted.git.detachedHead,
            gitDirty: persisted.git.dirty
          }
        }
      }
    }
    if (!probedCurrentContext) {
      git = await gitEnvironment(current.cwd)
      gitDirectory = git?.gitDirectory
    } else {
      gitDirectory = await resolveGitDirectory(current.cwd)
    }
    this.#hud.refreshConfig(sessionId)
    this.#hud.updateEnvironment(sessionId, {
      cwd: current.cwd,
      ...(current.shell ? { shell: current.shell } : {}),
      ...(git ? { gitBranch: git.gitBranch, gitDirty: git.gitDirty } : {})
    })
    this.#syncHudFileWatchers(sessionId, gitDirectory)
    this.publishSessionHud(sessionId)
  }

  #syncHudFileWatchers(sessionId: string, gitDirectory?: string): void {
    if (this.#closed || !this.#attachedSessionIds.has(sessionId)) return
    const desired = new Map<string, () => FSWatcher>()
    for (const target of this.#hud.configWatchTargets(sessionId)) {
      const names = new Set(target.names)
      const key = `config:${target.directory}:${[...names].sort().join(',')}`
      desired.set(key, () => watch(target.directory, { persistent: false }, (_event, filename) => {
        const changed = filename?.toString()
        if (changed && !names.has(changed)) return
        this.#scheduleHudFileRefresh(sessionId)
      }))
    }
    if (gitDirectory) {
      const key = `git:${gitDirectory}`
      desired.set(key, () => watch(gitDirectory, {
        persistent: false,
        recursive: process.platform === 'darwin' || process.platform === 'win32'
      }, () => this.#scheduleHudFileRefresh(sessionId)))
    }

    const current = this.#hudFileWatchers.get(sessionId) ?? new Map<string, FSWatcher>()
    for (const [key, watcher] of current) {
      if (desired.has(key)) continue
      watcher.close()
      current.delete(key)
    }
    for (const [key, create] of desired) {
      if (current.has(key)) continue
      try {
        current.set(key, create())
      } catch {
        // Parent-directory watches add newly created optional config folders on the next refresh.
      }
    }
    if (current.size > 0) this.#hudFileWatchers.set(sessionId, current)
    else this.#hudFileWatchers.delete(sessionId)
  }

  #scheduleHudFileRefresh(sessionId: string): void {
    if (this.#closed || !this.#attachedSessionIds.has(sessionId)) return
    const pending = this.#hudFileRefreshTimers.get(sessionId)
    if (pending) clearTimeout(pending)
    const timer = setTimeout(() => {
      this.#hudFileRefreshTimers.delete(sessionId)
      void this.refreshSessionHud(sessionId)
    }, 300)
    timer.unref?.()
    this.#hudFileRefreshTimers.set(sessionId, timer)
  }

  #closeHudFileWatchers(sessionId: string): void {
    const timer = this.#hudFileRefreshTimers.get(sessionId)
    if (timer) clearTimeout(timer)
    this.#hudFileRefreshTimers.delete(sessionId)
    for (const watcher of this.#hudFileWatchers.get(sessionId)?.values() ?? []) watcher.close()
    this.#hudFileWatchers.delete(sessionId)
  }

  #closeAllHudFileWatchers(): void {
    for (const sessionId of new Set([
      ...this.#hudFileWatchers.keys(), ...this.#hudFileRefreshTimers.keys()
    ])) this.#closeHudFileWatchers(sessionId)
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

  #assertProviderMutationAllowed(sessionId: string): void {
    this.#assertSessionDurabilityHealthy(sessionId)
    const recovery = this.#recoveryCoordinator?.snapshot()
      .find((job) => job.sessionId === sessionId)
    if (
      (recovery !== undefined && recovery.state !== 'ready') ||
      this.#sessions.providerIdentityPending(sessionId)
    ) {
      throw new RpcFault(
        'CONFLICT',
        'Session recovery must finish before changing permissions',
        true
      )
    }
  }

  #assertSessionDurabilityHealthy(sessionId: string): void {
    const session = this.#sessions.get(sessionId)
    if (session && session.durabilityState !== 'healthy') {
      throw new RpcFault(
        'CONFLICT',
        'terminal storage is paused; recover or end the Session before changing it',
        true
      )
    }
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

  static async #restartProviderSessions(cli: ProviderCli): Promise<void> {
    const restarted = new Set<string>()
    for (const server of RuntimeServer.#instances) {
      for (const [sessionId, descriptor] of server.#spawnDescriptors) {
        if (descriptor.profile !== cli || restarted.has(sessionId)) continue
        restarted.add(sessionId)
        await server.#respawnForProviderConfig(sessionId, descriptor)
      }
    }
  }

  async #respawnForProviderConfig(
    sessionId: string,
    descriptor: TerminalSpawnMessage
  ): Promise<void> {
    await this.#sessions.runExclusive(sessionId, async () => {
      const session = this.#sessions.get(sessionId)
      if (!session || session.profile !== descriptor.profile) return
      const recovery = this.#recoveryCoordinator?.snapshot()
        .find((job) => job.sessionId === sessionId)
      if (
        session.durabilityState !== 'healthy' ||
        (recovery !== undefined && recovery.state !== 'ready') ||
        this.#sessions.providerIdentityPending(sessionId)
      ) return
      const wasAttached = this.#attachedSessionIds.has(sessionId)
      this.#clearProviderResumeTimer(sessionId)
      this.#sessions.delete(sessionId, session)
      this.#control?.backend.unregister(sessionId, session)
      this.#control?.tokens.revokeRun(session.runId ?? sessionId)
      this.#providerInputBuffers.delete(sessionId)
      this.#workStatusTrackers.delete(sessionId)
      session.dispose({ notifyExit: false })
      await session.whenClosed()
      await this.#spawn(descriptor, undefined, undefined, wasAttached)
      const replacement = this.#sessions.get(sessionId)
      if (replacement && replacement.profile === descriptor.profile) {
        replacement.display('\u001b[2J\u001b[3J\u001b[H')
      }
    })
  }

  async #maybePromoteShellAgent(session: PtySession, data: string): Promise<boolean> {
    const previous = this.#shellInputBuffers.get(session.sessionId) ?? ''
    const next = updateShellInputBuffer(previous, data)
    this.#shellInputBuffers.set(session.sessionId, next.buffer)
    if (!next.submitted) return false
    this.#shellInputBuffers.delete(session.sessionId)
    const launch = await resolveInteractiveClaudeLaunch(next.command)
    if (!launch) return false
    return this.#sessions.runExclusive(session.sessionId, async () => {
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
        const latestDescriptor = this.#spawnDescriptors.get(session.sessionId) ?? descriptor
        await this.#spawn({ ...latestDescriptor, profile: 'claude-code' })
        const replacement = this.#sessions.get(session.sessionId)
        if (!replacement || replacement.profile !== 'claude-code') {
          throw new Error('Claude process did not start')
        }
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
    })
  }

  #observeProviderInput(sessionId: string, data: string): void {
    const normalized = data
      .replace(/\u001b\[200~/g, '')
      .replace(/\u001b\[201~/g, '')
      .replace(/\u001b\[[0-?]*[ -\/]*[@-~]/g, '')
    let buffer = this.#providerInputBuffers.get(sessionId) ?? ''
    for (const character of normalized) {
      if (character === '\r' || character === '\n') {
        const submitted = buffer.trim()
        if (submitted) this.#lastProviderInputs.set(sessionId, submitted)
        buffer = ''
      } else if (character === '\u007f' || character === '\b') {
        buffer = buffer.slice(0, -1)
      } else if (character === '\u0015') {
        buffer = ''
      } else if (character >= ' ' || character === '\t') {
        buffer += character
      }
    }
    this.#providerInputBuffers.set(sessionId, buffer.slice(-128 * 1024))
  }

  #setWorkStatus(
    sessionId: string,
    workStatus: import('@matou/domain').SessionWorkStatus
  ): void {
    try {
      if (this.#closed || this.#workStatuses.get(sessionId) === workStatus) return
      const now = Date.now()
      this.#workStatuses.set({
        commandId: `runtime-work-status-${sessionId}-${workStatus}-${now}-${randomUUID()}`,
        commandType: 'session.work-status',
        requestHash: `${sessionId}:${workStatus}:${now}`
      }, { sessionId, workStatus, now })
      RuntimeServer.#flushSemanticEventsForAll()
    } catch (error) {
      if (!this.#closed) console.error(`[session.work-status] ${errorMessage(error)}`)
    }
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function positiveTimeout(value: number | undefined, fallback: number): number {
  return Number.isFinite(value) && Number(value) > 0 ? Number(value) : fallback
}

function prependedPathEnvironment(
  entry: string,
  environment: NodeJS.ProcessEnv
): Record<string, string> {
  const pathKey = Object.keys(environment).find((key) => key.toLowerCase() === 'path') ?? 'PATH'
  const inherited = environment[pathKey]
  return { [pathKey]: inherited ? `${entry}${delimiter}${inherited}` : entry }
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
  const enriched = withSessionRuntimeEnvironment(result, sessionHuds)
  if (typeof enriched !== 'object' || enriched === null || !('hierarchy' in enriched)) return enriched
  const hierarchy = enriched.hierarchy
  if (typeof hierarchy !== 'object' || hierarchy === null) return result
  return { ...enriched, hierarchy: { ...hierarchy, sessionHuds } }
}

export function withSessionRuntimeEnvironment(
  result: unknown,
  sessionHuds: ReturnType<SessionHudRegistry['snapshots']>
): unknown {
  const hudBySession = new Map(sessionHuds.map((hud) => [hud.sessionId, hud] as const))
  const enrichGraph = (candidate: unknown): unknown => {
    if (!isRecord(candidate) || !Array.isArray(candidate.nodes) || !Array.isArray(candidate.edges)) return candidate
    return {
      ...candidate,
      nodes: candidate.nodes.map((node) => {
        if (!isRecord(node) || typeof node.sessionId !== 'string') return node
        const hud = hudBySession.get(node.sessionId)
        if (!hud) return node
        return {
          ...node,
          ...(hud.cwd ? { cwd: hud.cwd } : {}),
          ...(hud.gitBranch ? { git: { branch: hud.gitBranch, dirty: hud.gitDirty === true } } : {})
        }
      })
    }
  }
  const enrichGraphMap = (candidate: unknown): unknown => {
    if (!isRecord(candidate)) return candidate
    return Object.fromEntries(Object.entries(candidate).map(([key, graph]) => [key, enrichGraph(graph)]))
  }
  if (isRecord(result) && Array.isArray(result.nodes) && Array.isArray(result.edges)) return enrichGraph(result)
  if (!isRecord(result)) return result
  const next = {
    ...result,
    ...('sessionGraphs' in result ? { sessionGraphs: enrichGraphMap(result.sessionGraphs) } : {})
  }
  if (!isRecord(result.hierarchy)) return next
  return {
    ...next,
    hierarchy: {
      ...result.hierarchy,
      ...('sessionGraphs' in result.hierarchy
        ? { sessionGraphs: enrichGraphMap(result.hierarchy.sessionGraphs) }
        : {})
    }
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function shellName(profile: 'shell' | 'claude-code' | 'codex'): string | undefined {
  if (profile !== 'shell') return undefined
  const shell = process.env.SHELL
  return shell ? basename(shell) : process.platform === 'win32' ? 'PowerShell' : undefined
}

interface LiveGitEnvironment {
  gitBranch: string
  gitDirty: boolean
  gitDirectory?: string
}

async function gitEnvironment(cwd: string): Promise<LiveGitEnvironment | undefined> {
  try {
    const environment = { ...process.env, GIT_OPTIONAL_LOCKS: '0' }
    const [{ stdout: branchOutput }, { stdout: statusOutput }, { stdout: directoryOutput }] = await Promise.all([
      execFileAsync('git', ['-C', cwd, 'rev-parse', '--abbrev-ref', 'HEAD'], { env: environment }),
      execFileAsync('git', ['-C', cwd, 'status', '--porcelain', '--untracked-files=normal'], { env: environment }),
      execFileAsync('git', ['-C', cwd, 'rev-parse', '--absolute-git-dir'], { env: environment })
    ])
    const gitBranch = branchOutput.trim()
    const gitDirectory = directoryOutput.trim()
    return gitBranch ? {
      gitBranch,
      gitDirty: statusOutput.trim().length > 0,
      ...(gitDirectory ? { gitDirectory } : {})
    } : undefined
  } catch {
    return undefined
  }
}

async function resolveGitDirectory(cwd: string): Promise<string | undefined> {
  try {
    const environment = { ...process.env, GIT_OPTIONAL_LOCKS: '0' }
    const { stdout } = await execFileAsync(
      'git', ['-C', cwd, 'rev-parse', '--absolute-git-dir'], { env: environment }
    )
    const path = stdout.trim()
    return path || undefined
  } catch {
    return undefined
  }
}

function isGitMutation(method: string): boolean {
  return method === 'git.checkout' || method === 'git.create-branch' ||
    method === 'git.commit' || method === 'git.push' ||
    method === 'git.worktree-create' || method === 'git.worktree-open' ||
    method === 'git.worktree-remove'
}

function isSessionEnvironmentRpc(method: RpcMethod): boolean {
  return method === 'session.environment-open' ||
    method === 'session.environment-restore' ||
    method === 'session.environment-locate' ||
    method === 'session.environment-handoff'
}

function isTerminalHistoryRpc(
  method: RpcMethod
): method is Extract<RpcMethod, 'terminal.history-page' | 'terminal.history-search'> {
  return method === 'terminal.history-page' || method === 'terminal.history-search'
}

function recordFromRpc(value: unknown): Record<string, unknown> {
  if (!isRecord(value)) throw new RpcFault('INVALID_REQUEST', 'history payload must be an object')
  return value
}

function terminalHistorySessionId(value: unknown): string {
  if (typeof value !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,159}$/.test(value)) {
    throw new RpcFault('INVALID_REQUEST', 'sessionId contains unsupported characters')
  }
  return value
}

function terminalHistoryCursor(value: unknown): { sequence: number; lineIndex: number } | undefined {
  if (value === undefined) return undefined
  if (!isRecord(value) || !Number.isSafeInteger(value.sequence) || Number(value.sequence) < 0 ||
      !Number.isSafeInteger(value.lineIndex) || Number(value.lineIndex) < 0) {
    throw new RpcFault('INVALID_REQUEST', 'history cursor is invalid')
  }
  return { sequence: Number(value.sequence), lineIndex: Number(value.lineIndex) }
}

function terminalHistoryQuery(value: unknown): string {
  if (typeof value !== 'string' || value.length > 10_000) {
    throw new RpcFault('INVALID_REQUEST', 'history query must be at most 10000 characters')
  }
  return value
}

function terminalHistorySearchOptions(value: unknown): {
  caseSensitive: boolean
  regex: boolean
  wholeWord: boolean
} {
  if (!isRecord(value) || typeof value.caseSensitive !== 'boolean' ||
      typeof value.regex !== 'boolean' || typeof value.wholeWord !== 'boolean') {
    throw new RpcFault('INVALID_REQUEST', 'history search options are invalid')
  }
  return {
    caseSensitive: value.caseSensitive,
    regex: value.regex,
    wholeWord: value.wholeWord
  }
}

function optionalBoundedInteger(
  value: unknown,
  name: string,
  minimum: number,
  maximum: number
): number | undefined {
  if (value === undefined) return undefined
  if (!Number.isSafeInteger(value) || Number(value) < minimum || Number(value) > maximum) {
    throw new RpcFault('INVALID_REQUEST', `${name} must be between ${minimum} and ${maximum}`)
  }
  return Number(value)
}

function checkpointHasGrid(
  checkpoint: LoadedCheckpoint | undefined
): checkpoint is LoadedCheckpoint & { cols: number; rows: number } {
  if (checkpoint === undefined) return false
  const { cols, rows } = checkpoint
  return typeof cols === 'number' && typeof rows === 'number' &&
    Number.isSafeInteger(cols) && cols >= 2 && cols <= 1000 &&
    Number.isSafeInteger(rows) && rows >= 1 && rows <= 500
}

export function terminalSummaryLines(raw: string): string[] {
  const text = raw
    .replace(/\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g, '')
    .replace(/\u001b[()][0-2A-Z]/g, '')
    .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/\r/g, '\n')
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, '')
  return text.split('\n').map((line) => line.trimEnd())
    .filter((line) => line.trim().length > 0)
    .filter((line) => !looksLikeShellPrompt(line))
    .filter((line) => !looksLikeProviderChrome(line))
    .slice(-4)
}

function looksLikeShellPrompt(line: string): boolean {
  const value = line.trim()
  if (/^[%$#>]$/.test(value)) return true
  if (/^(?:\([^)]*\)\s*)?➜\s+\S+(?:\s+git:\([^)]+\)\s*\S*)?$/.test(value)) return true
  return /^[^\s@]+@[^\s]+\s+.+\s+[%$#>]$/.test(value)
}

function looksLikeProviderChrome(line: string): boolean {
  const value = line.trim()
  if (/^[❯›»]$/.test(value)) return true
  if (/^[─━═╌╍┄┅┈┉\s]+$/.test(value)) return true
  return /^[⏸⏵▶▷]\s*.*\b(?:manual mode on|for agents)\b/i.test(value)
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

async function resolveInteractiveClaudeLaunch(
  command: string
): Promise<InteractiveClaudeLaunch | undefined> {
  const normalized = command.trim().replace(/\s+/g, ' ')
  if (normalized === 'claude') return { permissionMode: 'default' }
  if (normalized === 'claude --dangerously-skip-permissions') {
    return { permissionMode: 'bypassPermissions' }
  }
  if (normalized !== 'cc') return undefined
  return configuredCcLaunch()
}

function configuredCcLaunch(): Promise<InteractiveClaudeLaunch | undefined> {
  const key = [
    process.env.SHELL ?? '', process.env.HOME ?? '', process.env.ZDOTDIR ?? ''
  ].join('\u0000')
  const existing = configuredCcLaunches.get(key)
  if (existing) return existing
  const pending = configuredShellAlias('cc').then((alias) => {
    if (alias === 'claude') return { permissionMode: 'default' as const }
    if (alias === 'claude --dangerously-skip-permissions') {
      return { permissionMode: 'bypassPermissions' as const }
    }
    return undefined
  })
  configuredCcLaunches.set(key, pending)
  return pending
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
