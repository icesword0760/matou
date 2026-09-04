import type { MessagePortMain, ParentPort } from 'electron'
import { randomUUID } from 'node:crypto'
import os from 'node:os'
import { resolve } from 'node:path'
import { monitorEventLoopDelay } from 'node:perf_hooks'

import {
  PROTOCOL_VERSION,
  parseRuntimeRecoveryCommand,
  type RuntimeConnectRequest,
  type RuntimeRecoveryCommand
} from '@matou/contracts'

import { RuntimeServer, type RuntimePort } from './runtime-server'
import {
  HostControlServer,
  CapabilityTokenService,
  controlEndpointForPlatform
} from './control/host-control-server'
import { RuntimeControlBackend } from './control/runtime-control-backend'
import { ForkBatchCoordinator } from './control/fork-batch-coordinator'
import { waitUntilForkSettled } from './control/fork-settlement-waiter'
import { HOST_CONTROL_FORK_PROVIDER_READY_TIMEOUT_MS } from './control/host-control-deadlines'
import { ProviderReadyRegistry } from './control/provider-ready-registry'
import {
  createE2eHostActionConfirmationOptions,
  HostActionConfirmationService
} from './control/host-action-confirmation-service'
import { HostActionTargetResolver } from './control/host-action-target-resolver'
import { RuntimeHostActionFacade } from './control/runtime-host-action-facade'
import { HostNavigationBroker } from './control/host-navigation-broker'
import { TaskTelemetryRepository } from './domain/product-foundation-repository'
import type { RuntimeDatabase } from './storage/database'
import { RuntimeRecoveryService } from './recovery/runtime-recovery-service'
import { RuntimeRecoveryCoordinator } from './recovery/runtime-recovery-coordinator'
import type { RecoveryJob } from './recovery/runtime-session-recovery-scheduler'
import { RuntimeRecoveryE2eObserver } from './recovery/runtime-recovery-e2e-observer'
import { RuntimeSessionRegistry } from './session/runtime-session-registry'
import { ProviderHookServer, providerTranscriptPath } from './session/provider-hook-server'
import { SessionHudRegistry } from './session/session-hud-registry'
import { SessionRepository } from './domain/session-repository'
import { FOUNDATION_MIGRATIONS } from './storage/migrations'
import { createE2eMigrationInterruptionObserver } from './storage/e2e-migration-interruption-observer'
import { DatabaseLifecycleService } from './storage/database-lifecycle-service'
import type { DatabaseBackupService } from './storage/database-backup-service'
import {
  openRecoverableRuntimeDatabase,
  type RuntimeDatabaseBootstrapObserver,
  type RuntimeDatabaseBootstrapResult
} from './storage/runtime-database-bootstrap'
import { DetachedSessionService } from './hierarchy/detached-session-service'
import { HierarchyApplicationService } from './hierarchy/hierarchy-application-service'
import { DomainTransactionManager } from './storage/domain-transaction'
import { RuntimeAccessPolicy } from './storage/runtime-access-policy'
import { NotificationProjection } from './product/experience-foundation'
import { RuntimeRpcRouter } from './rpc/runtime-rpc-router'
import { AgentNotificationRepository } from './notifications/agent-notification-repository'
import { ProviderModeService } from './session-canvas/provider-mode-service'
import { nextProviderWorkStatus } from './session/provider-work-status'
import { SessionWorkStatusService } from './session-canvas/session-work-status-service'
import { SessionCanvasService } from './session-canvas/session-canvas-service'
import {
  RuntimeLifecycleCoordinator,
  RuntimeShutdownRequestedError
} from './runtime-lifecycle-coordinator'
import { RuntimeProcessOrchestrator } from './runtime-process-orchestrator'
import { RuntimeLifecyclePublisher } from './runtime-lifecycle-publisher'
import { DatabaseRecoveryController } from './storage/database-recovery-controller'
import { exportReadOnlyDatabaseBundle } from './storage/read-only-database-export'
import { WorktreeHealthService } from './worktrees/worktree-health-service'
import { WorktreeReconciler } from './worktrees/worktree-reconciler'
import { WorktreeService } from './worktrees/worktree-service'
import { SessionEnvironmentRepository } from './session/session-environment-repository'
import { SessionEnvironmentService } from './session/session-environment-service'
import { SessionForkIntentRepository } from './session/session-fork-intent-repository'
import type { SessionExecutionDescriptor } from './session/session-execution-service'
import { ForkWorkflowService } from './session-canvas/fork-workflow-service'
import { ForkOperationCoordinator } from './session-canvas/fork-operation-coordinator'
import { createE2eForkCrashObserver } from './session-canvas/fork-operation-e2e-crash-controller'
import { createE2eForkSetupPolicyProvider } from './session-canvas/e2e-fork-setup-policy'
import { createE2eJournalOptionsProvider } from './journal/e2e-journal-fault-controller'
import { ProviderConfigStore } from './provider-config/provider-config-store'

type UtilityProcess = NodeJS.Process & { parentPort?: ParentPort }

const parentPort = (process as UtilityProcess).parentPort

if (!parentPort) {
  throw new Error('Matou Runtime must be launched as an Electron UtilityProcess')
}

const servers = new Set<RuntimeServer>()
const recoveryServerWaiters = new Set<(server: RuntimeServer) => void>()
const sessions = new RuntimeSessionRegistry()
const hostNavigation = new HostNavigationBroker()
const recoveryE2eObserver = process.env.MATOU_E2E === '1'
  ? new RuntimeRecoveryE2eObserver()
  : undefined
const scaleEventLoopDelay = process.env.MATOU_E2E_SCALE === '1'
  ? monitorEventLoopDelay({ resolution: 10 })
  : undefined
scaleEventLoopDelay?.enable()
const sessionHuds = new SessionHudRegistry()
const dataRoot = resolve(process.env.MATOU_DATA_DIR ?? resolve(os.homedir(), '.matou'))
const e2eJournalOptionsForSession = createE2eJournalOptionsProvider(process.env)
const e2eForkCrashObserver = createE2eForkCrashObserver(process.env)
const e2eForkSetupPolicyForWorkspace = createE2eForkSetupPolicyProvider(process.env)
const e2eMigrationObserver = createE2eMigrationInterruptionObserver(process.env)
interface RuntimeStateBase {
  mode: 'normal' | 'read-only'
  dataRoot: string
  database: RuntimeDatabase
  rpcRouter: RuntimeRpcRouter
  accessPolicy: RuntimeAccessPolicy
  providerConfigs: ProviderConfigStore
}

interface WritableRuntimeState extends RuntimeStateBase {
  mode: 'normal'
  controlEndpoint: string
  telemetry: TaskTelemetryRepository
  controlTokens: CapabilityTokenService
  controlBackend: RuntimeControlBackend
  hostControl: HostControlServer
  providerHooks: ProviderHookServer
  providerReady: ProviderReadyRegistry
  forkCoordinator: ForkOperationCoordinator
  forkBatchCoordinator: ForkBatchCoordinator
  hostActions: RuntimeHostActionFacade
  recoveryCoordinator: RuntimeRecoveryCoordinator
}

interface ReadOnlyRuntimeState extends RuntimeStateBase {
  mode: 'read-only'
}

type RuntimeState = WritableRuntimeState | ReadOnlyRuntimeState

let runtimeState: RuntimeState | undefined
let readOnlyDatabase: RuntimeDatabase | undefined
let forkCoordinator: ForkOperationCoordinator | undefined
const lifecycleCoordinator = new RuntimeLifecycleCoordinator()
const lifecyclePublisher = new RuntimeLifecyclePublisher(parentPort)
const bootstrapObserver: RuntimeDatabaseBootstrapObserver = {
  ...(e2eMigrationObserver ? { migrationObserver: e2eMigrationObserver } : {}),
  onDatabaseOpened: (
    database: RuntimeDatabase,
    _effectiveDataRoot: string,
    backups: DatabaseBackupService
  ) => {
    if (database.readOnly) {
      readOnlyDatabase = database
      return
    }
    lifecycleCoordinator.registerDatabaseLifecycle(
      database,
      new DatabaseLifecycleService(database, backups)
    )
  },
  onDatabaseClosed: (database: RuntimeDatabase) => {
    if (readOnlyDatabase === database) readOnlyDatabase = undefined
    lifecycleCoordinator.releaseDatabaseLifecycle(database)
  },
  isShutdownRequested: () => lifecycleCoordinator.shutdownRequested
}
const databaseRecovery = new DatabaseRecoveryController(
  dataRoot,
  FOUNDATION_MIGRATIONS,
  bootstrapObserver
)
let pendingDatabaseRecovery: Extract<RuntimeDatabaseBootstrapResult, { kind: 'recovery-required' }> | undefined
let settleDatabaseRecovery: {
  resolve(result: RuntimeDatabaseBootstrapResult): void
  reject(error: unknown): void
} | undefined
lifecyclePublisher.opening()
const runtimeReady = lifecycleCoordinator.startInitialization(initializeRuntime).then((state) => {
  runtimeState = state
  lifecyclePublisher.ready(state.mode)
  return state
})
void runtimeReady.catch((error: unknown) => {
  if (lifecycleCoordinator.shutdownRequested) return
  const failure = deterministicStartupFailure(error)
  if (failure) {
    console.error('[runtime.initialization]', error)
    parentPort.postMessage(failure)
    return
  }
  console.error('[runtime.initialization]', error)
  process.exitCode = 1
  setImmediate(() => process.exit(1))
})

async function initializeRuntime(): Promise<RuntimeState> {
  let opened = await openRecoverableRuntimeDatabase(
    dataRoot,
    FOUNDATION_MIGRATIONS,
    bootstrapObserver
  )
  while (opened.kind === 'recovery-required') {
    lifecyclePublisher.recoveryRequired(opened)
    opened = await waitForDatabaseRecovery(opened)
  }
  lifecycleCoordinator.assertStartupActive()
  const database = opened.database
  const runtimeDataRoot = opened.dataRoot
  const notifications = new NotificationProjection()
  const providerConfigs = new ProviderConfigStore(runtimeDataRoot)
  const accessPolicy = new RuntimeAccessPolicy(
    opened.kind === 'read-only' ? 'read-only' : 'normal'
  )
  const rpcRouter = new RuntimeRpcRouter(database, notifications, {
    accessPolicy,
    ...(e2eForkSetupPolicyForWorkspace ? {
      setupPolicyForWorkspace: e2eForkSetupPolicyForWorkspace
    } : {}),
    providerConfigs
  })
  if (opened.kind === 'read-only') {
    console.error(`[runtime.storage] database opened read-only: ${opened.reason}`)
    return {
      mode: 'read-only',
      dataRoot: runtimeDataRoot,
      database,
      rpcRouter,
      accessPolicy,
      providerConfigs
    }
  }
  const controlEndpoint = controlEndpointForPlatform(runtimeDataRoot)
  const telemetry = new TaskTelemetryRepository(database, database.runtimeGeneration)
  const transactions = new DomainTransactionManager(database)
  const stopSessions = async (sessionIds: string[]) => {
    for (const sessionId of new Set(sessionIds)) {
      const live = sessions.get(sessionId)
      if (!live) continue
      live.dispose({ notifyExit: false })
      await live.whenClosed()
      sessions.delete(sessionId, live)
    }
  }
  const stopRuns = async (runIds: string[]) => {
    const sessionIds = runIds.flatMap((runId) => {
      const sessionId = database.get<{ session_id: string }>(
        'SELECT session_id FROM session_runs WHERE id = ?', runId
      )?.session_id
      return sessionId === undefined ? [] : [sessionId]
    })
    await stopSessions(sessionIds)
  }
  const worktreeService = new WorktreeService(database, transactions, { stopRuns })
  const worktreeReconciliation = await new WorktreeReconciler(
    database,
    transactions,
    worktreeService,
    new WorktreeHealthService()
  ).reconcileAll(Date.now())
  if (worktreeReconciliation.degraded > 0) {
    console.error(
      `[runtime.worktree-reconciliation] ${worktreeReconciliation.degraded} environment(s) need attention`
    )
  }
  const environmentTransitions = await new SessionEnvironmentService(
    new SessionEnvironmentRepository(database),
    {
      restoreOwnedWorktree: async (identity) => {
        const row = database.get<{ setup_policy_json: string }>(
          'SELECT setup_policy_json FROM worktrees WHERE id = ?', identity.worktreeId
        )
        const operationId = randomUUID()
        await worktreeService.create({
          commandId: `startup-environment-restore-${identity.sessionId}-${operationId}`,
          commandType: 'session.environment-startup-restore',
          requestHash: `${identity.sessionId}:${identity.worktreeId}:${identity.path}`
        }, {
          id: identity.worktreeId,
          executionContextId: identity.executionContextId,
          workspaceId: identity.workspaceId,
          repositoryRoot: identity.repositoryRoot,
          path: identity.path,
          branch: identity.branch,
          baseRef: identity.baseRef ?? identity.baseRevision ?? 'HEAD',
          setupPolicy: row
            ? JSON.parse(row.setup_policy_json) as Array<{ command: string; args: string[] }>
            : [],
          now: Date.now()
        })
      },
      pauseSession: async (sessionId) => {
        const live = sessions.get(sessionId)
        if (!live) return
        live.dispose({ notifyExit: false, reason: 'environment-transition' })
        await live.whenClosed()
        sessions.delete(sessionId, live)
      },
      resumeSession: async () => undefined
    },
    new WorktreeHealthService()
  ).reconcileTransitions(Date.now())
  if (environmentTransitions.failed > 0) {
    console.error(
      `[runtime.environment-reconciliation] ${environmentTransitions.failed} transition(s) need attention`
    )
  }
  const sessionRepository = new SessionRepository(database, transactions)
  const providerModes = new ProviderModeService(database, transactions)
  const workStatuses = new SessionWorkStatusService(database, transactions)
  const hierarchy = new HierarchyApplicationService(database, transactions)
  const sessionCanvas = new SessionCanvasService(database, transactions)
  const hostActionResolver = new HostActionTargetResolver(database)
  const hostActionConfirmations = new HostActionConfirmationService(
    createE2eHostActionConfirmationOptions(process.env)
  )
  const controlTokens = new CapabilityTokenService(database.runtimeGeneration, {
    onRunRevoked: (runId) => hostActionConfirmations.revokeRun(runId)
  })
  const controlBackend = new RuntimeControlBackend(database, runtimeDataRoot, telemetry, notifications)
  const providerReady = new ProviderReadyRegistry()
  const hostControl = new HostControlServer({
    socketPath: controlEndpoint,
    tokenService: controlTokens,
    backend: controlBackend
  })
  lifecycleCoordinator.registerHostControl(hostControl)
  const agentNotifications = new AgentNotificationRepository(database, transactions)
  const providerHooks = new ProviderHookServer(runtimeDataRoot, sessionRepository, {
    currentRunId: (sessionId) => sessions.get(sessionId)?.runId,
    onNotification: (notification) => {
      const now = Date.now()
      const eventId = `agent-notification-${randomUUID()}`
      agentNotifications.publish({
        commandId: `publish-${eventId}`,
        commandType: 'agent.notification.publish',
        requestHash: `${notification.runId}:${notification.sessionId}:${notification.event.eventType}:${now}`
      }, { ...notification, eventId, now })
      try {
        const currentWorkStatus = workStatuses.get(notification.sessionId)
        const workStatus = nextProviderWorkStatus(
          currentWorkStatus, notification.event.eventType
        )
        if (currentWorkStatus !== workStatus) {
          workStatuses.set({
            commandId: `provider-work-status-${notification.runId}-${eventId}`,
            commandType: 'session.provider-work-status',
            requestHash: `${notification.sessionId}:${workStatus}:${now}`
          }, { sessionId: notification.sessionId, workStatus, now })
        }
      } catch (error) {
        console.error(`[provider-work-status] ${errorMessage(error)}`)
      }
      for (const server of servers) server.flushSemanticEvents()
    },
    onHudPayload: ({ sessionId, payload }) => {
      sessionHuds.ingestProvider(sessionId, payload)
      for (const server of servers) void server.refreshSessionHud(sessionId)
      const transcriptPath = providerTranscriptPath(payload)
      if (transcriptPath) void sessionHuds.refreshTranscript(sessionId, transcriptPath).then((changed) => {
        if (changed) for (const server of servers) void server.refreshSessionHud(sessionId)
      }).catch((error) => console.error(`[provider-hud-history] ${errorMessage(error)}`))
    },
    onTitleObserved: ({ sessionId, providerSessionId, title, runId }) => {
      const now = Date.now()
      sessionHuds.updateSessionName(sessionId, title)
      try {
        sessionRepository.observeProviderTitle({
          commandId: `provider-title-${runId}-${randomUUID()}`,
          commandType: 'provider-hook.title',
          requestHash: `${sessionId}:${providerSessionId}:${title}:${now}`
        }, { sessionId, title, now })
        for (const server of servers) server.flushSemanticEvents()
      } catch (error) {
        console.error(`[provider-title] ${errorMessage(error)}`)
      }
    },
    onTeamObservations: async (observations) => {
      let changed = false
      for (const observation of observations) {
        const now = Date.now()
        try {
          sessionCanvas.upsertAgentTeamMember({
            commandId: `provider-team-${observation.runId}-${randomUUID()}`,
            commandType: 'provider-hook.team-member',
            requestHash: `${observation.leadSessionId}:${observation.teammateId}:${observation.workStatus}:${JSON.stringify(observation.latestLines)}`
          }, {
            leadSessionId: observation.leadSessionId,
            teammateId: observation.teammateId,
            teamId: observation.teamId,
            name: observation.name,
            workStatus: observation.workStatus,
            latestLines: observation.latestLines,
            now
          })
          changed = true
        } catch (error) {
          console.error(`[provider-team-member] ${errorMessage(error)}`)
        }
      }
      if (changed) for (const server of servers) server.flushSemanticEvents()
    },
    onIdentityRecorded: ({
      sessionId, runId, provider, providerSessionId, eventName, forkAuthority
    }) => {
      providerReady.record(sessionId, runId, sessions.get(sessionId)?.runId ?? '')
      sessionHuds.markResumable(sessionId)
      const now = Date.now()
      if (provider === 'claude-code') {
        try {
          providerModes.observeHook({
            commandId: `provider-mode-${runId}-${eventName}-${randomUUID()}`,
            commandType: 'provider-hook.mode',
            requestHash: `${sessionId}:${providerSessionId}:${eventName}:${now}`
          }, { sessionId, providerSessionId, eventName, now })
        } catch (error) {
          console.error(`[provider-mode] ${errorMessage(error)}`)
        }
      }
      for (const server of servers) {
        server.providerIdentityRecorded(sessionId, runId)
        server.flushSemanticEvents()
        void server.refreshSessionHud(sessionId)
      }
      if (forkAuthority) {
        void forkCoordinator?.confirmAuthoritativeIdentity(
          sessionId,
          providerSessionId,
          forkAuthority.operationId
        )
      }
    },
    onIdentityMismatch: (event) => {
      for (const server of servers) server.providerIdentityMismatch(event)
    }
  })
  lifecycleCoordinator.registerProviderHooks(providerHooks)
  new DetachedSessionService(database, transactions)
    .normalizeOnStartup(Date.now())
  const recoveryService = new RuntimeRecoveryService(runtimeDataRoot, database)
  const recovery = await recoveryService.recoverAll()
  for (const failure of recovery.failed) {
    console.error(`[runtime.recovery] ${failure.sessionId} ${failure.code}: ${failure.message}`)
  }
  const recoveryCoordinator = new RuntimeRecoveryCoordinator({
    concurrency: 4,
    jobs: recoveryService.planSessionRecovery(),
    start: (job) => startRecoveryJob(job),
    publish: (snapshot) => {
      recoveryE2eObserver?.record(snapshot)
      for (const server of servers) server.publishRecovery(snapshot)
    }
  })
  recoveryCoordinator.start()
  telemetry.purgeStaleGenerations()
  lifecycleCoordinator.assertStartupActive()
  await providerHooks.start()
  lifecycleCoordinator.assertStartupActive()
  const backgroundPort = new BackgroundRuntimePort()
  const backgroundServer = new RuntimeServer(
    backgroundPort,
    runtimeDataRoot,
    database,
    rpcRouter,
    { backend: controlBackend, tokens: controlTokens, endpoint: controlEndpoint },
    sessions,
    providerHooks,
    undefined,
    {
      hudRegistry: sessionHuds,
      accessPolicy,
      providerConfigs,
      ...(process.env.MATOU_CONTROL_ASSET_ROOT === undefined ? {} : {
        controlAssetRoot: process.env.MATOU_CONTROL_ASSET_ROOT
      }),
      ...(process.env.MATOU_CONTROL_NODE_EXECUTABLE === undefined ? {} : {
        controlNodeExecutable: process.env.MATOU_CONTROL_NODE_EXECUTABLE
      }),
      ...(e2eJournalOptionsForSession ? {
        journalOptionsForSession: e2eJournalOptionsForSession
      } : {}),
      recoveryCoordinator,
      navigationBroker: hostNavigation
    }
  )
  servers.add(backgroundServer)
  const forkWorkflow = new ForkWorkflowService(runtimeDataRoot, database, transactions, {
    stopRuns,
    providerConfigs,
    ...(e2eForkSetupPolicyForWorkspace ? {
      setupPolicyForWorkspace: e2eForkSetupPolicyForWorkspace
    } : {}),
    onProgressCommitted: () => {
      for (const server of servers) server.flushSemanticEvents()
    }
  })
  const forkBatchCoordinator = new ForkBatchCoordinator({
    database,
    createChild: (command, input) => forkWorkflow.createForkChild(command, input),
    retryChild: (command, input) => forkWorkflow.retryFork(command, input),
    startSession: async (sessionId) => {
      const descriptor = forkExecutionDescriptor(database, sessionId)
      if (!descriptor) throw new Error(`会话 ${sessionId} 尚未准备完成`)
      await backgroundServer.startOrResumeSession(descriptor)
    },
    waitUntilReady: (sessionId, signal) => providerReady.wait(
      sessionId, HOST_CONTROL_FORK_PROVIDER_READY_TIMEOUT_MS, signal
    ),
    waitUntilForkSettled: (sessionId) => waitUntilForkSettled(database, sessionId),
    sendPrompt: (sessionId, prompt) => controlBackend.sendText(sessionId, prompt, true)
  })
  const hostActions = new RuntimeHostActionFacade({
    database,
    resolver: hostActionResolver,
    confirmations: hostActionConfirmations,
    hierarchy,
    sessionCanvas,
    forkWorkflow,
    forkBatches: forkBatchCoordinator,
    navigation: hostNavigation,
    disposeSessions: (sessionIds) => backgroundServer.disposeSessions(sessionIds)
  })
  // Do not accept Host Control requests until the complete facade owns every action.
  controlBackend.setHostActionExecutor((method, caller, params) =>
    hostActions.execute(method, caller, params)
  )
  forkCoordinator = new ForkOperationCoordinator(
    new SessionForkIntentRepository(database),
    {
      ownerId: `runtime-${database.runtimeGeneration}`,
      executeFork: (command, input) => forkWorkflow.executeFork(command, input),
      startOrResume: async (sessionId, authority) => {
        const descriptor = forkExecutionDescriptor(database, sessionId)
        if (!descriptor) throw new Error(`Fork Session ${sessionId} is unavailable`)
        const recoveryJob = recoveryService.describeExternalForkRecovery(sessionId)
        if (recoveryJob) recoveryCoordinator.trackExternal(recoveryJob)
        return backgroundServer.startOrResumeSession(descriptor, authority)
      },
      notify: (notification) => {
        recoveryCoordinator.settleExternal(
          notification.sessionId,
          notification.status === 'succeeded' ? 'ready' : 'failed',
          notification.error
        )
        const now = Date.now()
        agentNotifications.publish({
          commandId: notification.eventId,
          commandType: 'fork-operation.notification',
          requestHash: notification.replacementKey
        }, {
          eventId: notification.eventId,
          runId: notification.operationId,
          sessionId: notification.sessionId,
          provider: 'claude-code',
          event: {
            eventType: notification.status === 'succeeded' ? 'completed' : 'error',
            title: 'Claude Code',
            subtitle: notification.status === 'succeeded' ? '分支已就绪' : '分支创建失败',
            body: notification.status === 'succeeded'
              ? '新的分支会话已经可以继续工作'
              : notification.error ?? '分支创建未完成',
            sound: true,
            cooldownKey: 'Notification',
            replacementKey: notification.replacementKey
          },
          now
        })
        for (const server of servers) server.flushSemanticEvents()
      },
      ...(e2eForkCrashObserver ? { observer: e2eForkCrashObserver } : {})
    }
  )
  await hostControl.start()
  lifecycleCoordinator.assertStartupActive()
  forkCoordinator.start()
  return {
    mode: 'normal',
    dataRoot: runtimeDataRoot,
    controlEndpoint,
    database,
    telemetry,
    controlTokens,
    controlBackend,
    rpcRouter,
    accessPolicy,
    hostControl,
    providerHooks,
    providerReady,
    forkCoordinator,
    forkBatchCoordinator,
    hostActions,
    recoveryCoordinator,
    providerConfigs
  }
}

function shutdown(): Promise<void> {
  settleDatabaseRecovery?.reject(new RuntimeShutdownRequestedError())
  settleDatabaseRecovery = undefined
  pendingDatabaseRecovery = undefined
  return lifecycleCoordinator.shutdown(runtimeReady, {
    closeIncoming: () => {
      if (runtimeState?.mode === 'normal') {
        runtimeState.providerReady.cancelAll(new Error('Runtime 正在关闭'))
      }
      forkCoordinator?.stop()
      forkCoordinator = undefined
      hostNavigation.close()
      for (const server of servers) server.close()
      servers.clear()
    },
    shutdownSessions: () => sessions.shutdownAll()
  }).finally(() => {
    readOnlyDatabase?.close()
    readOnlyDatabase = undefined
  })
}
const processOrchestrator = new RuntimeProcessOrchestrator({
  runtimeReady,
  shutdown,
  reportError: (label, error) => console.error(label, error),
  exit: (code) => process.exit(code)
})
void processOrchestrator.watchInitialization()
process.once('SIGTERM', () => {
  void processOrchestrator.terminateFromSignal()
})

parentPort.on('message', async (event) => {
  if (
    process.env.MATOU_E2E_SCALE === '1' &&
    event.data &&
    typeof event.data === 'object' &&
    event.data.type === 'runtime.scale-metrics-request'
  ) {
    const request = event.data as {
      requestId?: unknown
      resetStatementCount?: unknown
    }
    const state = await runtimeReady
    const eventLoopDelayP99Ms = nanosecondsToMilliseconds(scaleEventLoopDelay?.percentile(99) ?? 0)
    const eventLoopDelayMaxMs = nanosecondsToMilliseconds(scaleEventLoopDelay?.max ?? 0)
    const memory = process.memoryUsage()
    parentPort.postMessage({
      type: 'runtime.scale-metrics-result',
      requestId: String(request.requestId ?? 'invalid'),
      runtimePid: process.pid,
      ptyCount: sessions.size,
      ptyPids: sessions.pids(),
      ptySessions: sessions.sessionPids(),
      recoveryObservation: recoveryE2eObserver?.snapshot(),
      statementCount: state.database.readStatementCount(request.resetStatementCount === true),
      statementProfile: state.database.readStatementProfile(),
      eventLoopDelayP99Ms,
      eventLoopDelayMaxMs,
      maxUnackedBytes: sessions.maxUnackedBytes(),
      retainedDurabilityBytes: sessions.retainedDurabilityBytes(),
      heapUsedBytes: memory.heapUsed,
      externalBytes: memory.external,
      arrayBufferBytes: memory.arrayBuffers
    })
    if (request.resetStatementCount === true) scaleEventLoopDelay?.reset()
    return
  }
  if (event.data && typeof event.data === 'object' && event.data.type === 'runtime.recovery-command') {
    let command: RuntimeRecoveryCommand
    try {
      command = parseRuntimeRecoveryCommand(event.data)
    } catch (error) {
      parentPort.postMessage({
        type: 'runtime.recovery-result',
        requestId: String((event.data as { requestId?: unknown }).requestId ?? 'invalid'),
        ok: false,
        error: errorMessage(error)
      })
      return
    }
    void executeDatabaseRecoveryCommand(command)
    return
  }
  const request = event.data as Partial<RuntimeConnectRequest>
  const port = event.ports[0] as MessagePortMain | undefined
  if (
    request.type !== 'runtime.connect' ||
    request.protocolVersion !== PROTOCOL_VERSION ||
    !port
  ) {
    port?.close()
    return
  }

  try {
    const state = await runtimeReady
    if (lifecycleCoordinator.shutdownRequested) {
      port.close()
      return
    }
    const control = state.mode === 'normal'
      ? {
          backend: state.controlBackend,
          tokens: state.controlTokens,
          endpoint: state.controlEndpoint
        }
      : undefined
    const providerHooks = state.mode === 'normal' ? state.providerHooks : undefined
    const server = new RuntimeServer(
      port,
      state.dataRoot,
      state.database,
      state.rpcRouter,
      control,
      sessions,
      providerHooks,
      undefined,
      {
        hudRegistry: sessionHuds,
        accessPolicy: state.accessPolicy,
        providerConfigs: state.providerConfigs,
        ...(process.env.MATOU_CONTROL_ASSET_ROOT === undefined ? {} : {
          controlAssetRoot: process.env.MATOU_CONTROL_ASSET_ROOT
        }),
        ...(process.env.MATOU_CONTROL_NODE_EXECUTABLE === undefined ? {} : {
          controlNodeExecutable: process.env.MATOU_CONTROL_NODE_EXECUTABLE
        }),
        ...(e2eJournalOptionsForSession ? {
          journalOptionsForSession: e2eJournalOptionsForSession
        } : {}),
        ...(state.mode === 'normal' ? { recoveryCoordinator: state.recoveryCoordinator } : {}),
        navigationBroker: hostNavigation
      }
    )
    servers.add(server)
    for (const resolve of recoveryServerWaiters) resolve(server)
    recoveryServerWaiters.clear()
    port.once('close', () => servers.delete(server))
  } catch (error) {
    console.error('Matou Runtime schema migration failed', error)
    port.close()
    process.exitCode = 1
    return
  }
})

function nanosecondsToMilliseconds(value: number): number {
  if (!Number.isFinite(value)) return 0
  return Math.round(value / 10_000) / 100
}

async function startRecoveryJob(job: RecoveryJob): Promise<void> {
  const server = [...servers][0] ?? await new Promise<RuntimeServer>((resolve) => {
    recoveryServerWaiters.add(resolve)
  })
  await server.ensureSessionRunning(job)
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function deterministicStartupFailure(error: unknown): {
  type: 'runtime.startup-failure'
  code: 'MIGRATION_HISTORY_MISMATCH' | 'DATABASE_SCHEMA_UNSUPPORTED'
  message: string
  retryable: false
} | undefined {
  const message = errorMessage(error)
  if (/checksum mismatch for applied migration \d+/i.test(message)) {
    return {
      type: 'runtime.startup-failure',
      code: 'MIGRATION_HISTORY_MISMATCH',
      message: '工作区升级记录与当前版本不一致，原数据保持原样。',
      retryable: false
    }
  }
  if (/database schema version \d+ is newer than supported version \d+/i.test(message)) {
    return {
      type: 'runtime.startup-failure',
      code: 'DATABASE_SCHEMA_UNSUPPORTED',
      message: '工作区数据来自较新的 Matou 版本，请更新应用后重新检查。',
      retryable: false
    }
  }
  return undefined
}

function forkExecutionDescriptor(
  database: RuntimeDatabase,
  sessionId: string
): SessionExecutionDescriptor | undefined {
  const row = database.get<{
    execution_context_id: string
    kind: string
    cols: number | null
    rows: number | null
  }>(
    `SELECT sessions.execution_context_id, sessions.kind,
            (SELECT runs.cols FROM session_runs AS runs
             WHERE runs.session_id = sessions.id
             ORDER BY runs.started_at DESC, runs.id DESC LIMIT 1) AS cols,
            (SELECT runs.rows FROM session_runs AS runs
             WHERE runs.session_id = sessions.id
             ORDER BY runs.started_at DESC, runs.id DESC LIMIT 1) AS rows
     FROM sessions WHERE sessions.id = ? AND sessions.archived_at IS NULL`,
    sessionId
  )
  if (!row || row.kind !== 'claude-code') return undefined
  return {
    sessionId,
    executionContextId: row.execution_context_id,
    profile: 'claude-code',
    cols: validTerminalSize(row.cols, 120, 2, 1_000),
    rows: validTerminalSize(row.rows, 40, 1, 500)
  }
}

function validTerminalSize(
  value: number | null,
  fallback: number,
  minimum: number,
  maximum: number
): number {
  return Number.isInteger(value) && value !== null && value >= minimum && value <= maximum
    ? value
    : fallback
}

class BackgroundRuntimePort implements RuntimePort {
  readonly #closeListeners = new Set<() => void>()
  #closed = false

  on(event: 'message', listener: (event: { data: unknown }) => void): this
  on(event: 'close', listener: () => void): this
  on(
    event: 'message' | 'close',
    listener: ((event: { data: unknown }) => void) | (() => void)
  ): this {
    if (event === 'close') this.#closeListeners.add(listener as () => void)
    return this
  }

  postMessage(): void {}
  start(): void {}

  close(): void {
    if (this.#closed) return
    this.#closed = true
    for (const listener of this.#closeListeners) listener()
    this.#closeListeners.clear()
  }
}

async function waitForDatabaseRecovery(
  recovery: Extract<RuntimeDatabaseBootstrapResult, { kind: 'recovery-required' }>
): Promise<RuntimeDatabaseBootstrapResult> {
  console.error(
    `[runtime.storage] database recovery required; quarantined=${recovery.quarantinedPath}; ` +
    `backups=${recovery.backups.length}`
  )
  pendingDatabaseRecovery = recovery
  return new Promise<RuntimeDatabaseBootstrapResult>((resolve, reject) => {
    settleDatabaseRecovery = { resolve, reject }
  }).finally(() => {
    settleDatabaseRecovery = undefined
  })
}

async function executeDatabaseRecoveryCommand(command: RuntimeRecoveryCommand): Promise<void> {
  const recovery = pendingDatabaseRecovery
  if (!recovery || !settleDatabaseRecovery) {
    if (command.action === 'export-recovery-bundle') {
      try {
        const state = await runtimeReady
        if (state.mode !== 'read-only') throw new Error('当前数据库不在只读恢复模式')
        const destinationRoot = resolve(
          process.env.MATOU_RECOVERY_EXPORT_DIR ?? resolve(os.homedir(), 'Downloads', 'Matou-Recovery')
        )
        const exportedPath = await exportReadOnlyDatabaseBundle(
          state.database.path,
          destinationRoot
        )
        parentPort.postMessage({
          type: 'runtime.recovery-result', requestId: command.requestId, ok: true,
          value: { exportedPath }
        })
      } catch (error) {
        parentPort.postMessage({
          type: 'runtime.recovery-result', requestId: command.requestId, ok: false,
          error: errorMessage(error)
        })
      }
      return
    }
    parentPort.postMessage({
      type: 'runtime.recovery-result', requestId: command.requestId, ok: false,
      error: '当前没有待处理的数据库恢复操作'
    })
    return
  }
  if (
    command.action !== 'export-recovery-bundle' &&
    command.expectedRecoveryId !== recovery.recoveryId
  ) {
    parentPort?.postMessage({
      type: 'runtime.recovery-result', requestId: command.requestId, ok: false,
      error: '数据库恢复周期已更新，本次操作已停止'
    })
    return
  }
  if (command.action !== 'export-recovery-bundle') lifecyclePublisher.openingNewAttempt()
  try {
    const result = await databaseRecovery.execute(recovery, command)
    if (result.bootstrap) {
      if (result.bootstrap.kind === 'recovery-required') {
        pendingDatabaseRecovery = result.bootstrap
        lifecyclePublisher.recoveryRequired(result.bootstrap)
        throw new Error('重新检查后数据库仍需要恢复')
      }
      pendingDatabaseRecovery = undefined
      settleDatabaseRecovery.resolve(result.bootstrap)
    }
    parentPort.postMessage({
      type: 'runtime.recovery-result', requestId: command.requestId, ok: true,
      value: result.value
    })
  } catch (error) {
    lifecyclePublisher.recoveryRequired(pendingDatabaseRecovery ?? recovery)
    parentPort.postMessage({
      type: 'runtime.recovery-result', requestId: command.requestId, ok: false,
      error: errorMessage(error)
    })
  }
}
