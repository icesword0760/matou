import type { MessagePortMain, ParentPort } from 'electron'
import { randomUUID } from 'node:crypto'
import os from 'node:os'
import { resolve } from 'node:path'

import {
  PROTOCOL_VERSION,
  parseRuntimeRecoveryCommand,
  type RuntimeConnectRequest,
  type RuntimeRecoveryCommand
} from '@matou/contracts'

import { RuntimeServer } from './runtime-server'
import {
  HostControlServer,
  CapabilityTokenService,
  controlEndpointForPlatform
} from './control/host-control-server'
import { RuntimeControlBackend } from './control/runtime-control-backend'
import { TaskTelemetryRepository } from './domain/product-foundation-repository'
import type { RuntimeDatabase } from './storage/database'
import { RuntimeRecoveryService } from './recovery/runtime-recovery-service'
import { RuntimeSessionRegistry } from './session/runtime-session-registry'
import { ProviderHookServer } from './session/provider-hook-server'
import { SessionHudRegistry } from './session/session-hud-registry'
import { SessionRepository } from './domain/session-repository'
import { FOUNDATION_MIGRATIONS } from './storage/migrations'
import { DatabaseLifecycleService } from './storage/database-lifecycle-service'
import type { DatabaseBackupService } from './storage/database-backup-service'
import {
  openRecoverableRuntimeDatabase,
  type RuntimeDatabaseBootstrapObserver,
  type RuntimeDatabaseBootstrapResult
} from './storage/runtime-database-bootstrap'
import { DetachedSessionService } from './hierarchy/detached-session-service'
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

type UtilityProcess = NodeJS.Process & { parentPort?: ParentPort }

const parentPort = (process as UtilityProcess).parentPort

if (!parentPort) {
  throw new Error('Matou Runtime must be launched as an Electron UtilityProcess')
}

const servers = new Set<RuntimeServer>()
const sessions = new RuntimeSessionRegistry()
const sessionHuds = new SessionHudRegistry()
const dataRoot = resolve(process.env.MATOU_DATA_DIR ?? resolve(os.homedir(), '.matou'))
interface RuntimeStateBase {
  mode: 'normal' | 'read-only'
  dataRoot: string
  database: RuntimeDatabase
  rpcRouter: RuntimeRpcRouter
  accessPolicy: RuntimeAccessPolicy
}

interface WritableRuntimeState extends RuntimeStateBase {
  mode: 'normal'
  controlEndpoint: string
  telemetry: TaskTelemetryRepository
  controlTokens: CapabilityTokenService
  controlBackend: RuntimeControlBackend
  hostControl: HostControlServer
  providerHooks: ProviderHookServer
}

interface ReadOnlyRuntimeState extends RuntimeStateBase {
  mode: 'read-only'
}

type RuntimeState = WritableRuntimeState | ReadOnlyRuntimeState

let runtimeState: RuntimeState | undefined
let readOnlyDatabase: RuntimeDatabase | undefined
const lifecycleCoordinator = new RuntimeLifecycleCoordinator()
const lifecyclePublisher = new RuntimeLifecyclePublisher(parentPort)
const bootstrapObserver: RuntimeDatabaseBootstrapObserver = {
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
  const accessPolicy = new RuntimeAccessPolicy(
    opened.kind === 'read-only' ? 'read-only' : 'normal'
  )
  const rpcRouter = new RuntimeRpcRouter(database, notifications, { accessPolicy })
  if (opened.kind === 'read-only') {
    console.error(`[runtime.storage] database opened read-only: ${opened.reason}`)
    return {
      mode: 'read-only',
      dataRoot: runtimeDataRoot,
      database,
      rpcRouter,
      accessPolicy
    }
  }
  const controlEndpoint = controlEndpointForPlatform(runtimeDataRoot)
  const telemetry = new TaskTelemetryRepository(database, database.runtimeGeneration)
  const transactions = new DomainTransactionManager(database)
  const sessionRepository = new SessionRepository(database, transactions)
  const providerModes = new ProviderModeService(database, transactions)
  const workStatuses = new SessionWorkStatusService(database, transactions)
  const sessionCanvas = new SessionCanvasService(database, transactions)
  const controlTokens = new CapabilityTokenService(database.runtimeGeneration)
  const controlBackend = new RuntimeControlBackend(database, runtimeDataRoot, telemetry, notifications)
  const hostControl = new HostControlServer({
    socketPath: controlEndpoint,
    tokenService: controlTokens,
    backend: controlBackend
  })
  lifecycleCoordinator.registerHostControl(hostControl)
  const agentNotifications = new AgentNotificationRepository(database, transactions)
  const providerHooks = new ProviderHookServer(runtimeDataRoot, sessionRepository, {
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
    onIdentityRecorded: ({ sessionId, runId, providerSessionId, eventName }) => {
      sessionHuds.markResumable(sessionId)
      const now = Date.now()
      try {
        providerModes.observeHook({
          commandId: `provider-mode-${runId}-${eventName}-${randomUUID()}`,
          commandType: 'provider-hook.mode',
          requestHash: `${sessionId}:${providerSessionId}:${eventName}:${now}`
        }, { sessionId, providerSessionId, eventName, now })
      } catch (error) {
        console.error(`[provider-mode] ${errorMessage(error)}`)
      }
      for (const server of servers) {
        server.providerIdentityRecorded(sessionId, runId)
        server.flushSemanticEvents()
        void server.refreshSessionHud(sessionId)
      }
    }
  })
  lifecycleCoordinator.registerProviderHooks(providerHooks)
  new DetachedSessionService(database, transactions)
    .normalizeOnStartup(Date.now())
  const recovery = await new RuntimeRecoveryService(runtimeDataRoot, database).recoverAll()
  for (const failure of recovery.failed) {
    console.error(`[runtime.recovery] ${failure.sessionId} ${failure.code}: ${failure.message}`)
  }
  telemetry.purgeStaleGenerations()
  lifecycleCoordinator.assertStartupActive()
  await hostControl.start()
  lifecycleCoordinator.assertStartupActive()
  await providerHooks.start()
  lifecycleCoordinator.assertStartupActive()
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
    providerHooks
  }
}

function shutdown(): Promise<void> {
  settleDatabaseRecovery?.reject(new RuntimeShutdownRequestedError())
  settleDatabaseRecovery = undefined
  pendingDatabaseRecovery = undefined
  return lifecycleCoordinator.shutdown(runtimeReady, {
    closeIncoming: () => {
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
      { hudRegistry: sessionHuds, accessPolicy: state.accessPolicy }
    )
    servers.add(server)
    port.once('close', () => servers.delete(server))
  } catch (error) {
    console.error('Matou Runtime schema migration failed', error)
    port.close()
    process.exitCode = 1
    return
  }
})

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
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
