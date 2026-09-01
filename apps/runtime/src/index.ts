import type { MessagePortMain, ParentPort } from 'electron'
import { randomUUID } from 'node:crypto'
import os from 'node:os'
import { resolve } from 'node:path'

import { PROTOCOL_VERSION, type RuntimeConnectRequest } from '@matou/contracts'

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
import { openRecoverableRuntimeDatabase } from './storage/runtime-database-bootstrap'
import { DetachedSessionService } from './hierarchy/detached-session-service'
import { DomainTransactionManager } from './storage/domain-transaction'
import { NotificationProjection } from './product/experience-foundation'
import { RuntimeRpcRouter } from './rpc/runtime-rpc-router'
import { AgentNotificationRepository } from './notifications/agent-notification-repository'
import { ProviderModeService } from './session-canvas/provider-mode-service'
import { nextProviderWorkStatus } from './session/provider-work-status'
import { SessionWorkStatusService } from './session-canvas/session-work-status-service'
import { SessionCanvasService } from './session-canvas/session-canvas-service'
import { RuntimeLifecycleCoordinator } from './runtime-lifecycle-coordinator'
import { RuntimeProcessOrchestrator } from './runtime-process-orchestrator'

type UtilityProcess = NodeJS.Process & { parentPort?: ParentPort }

const parentPort = (process as UtilityProcess).parentPort

if (!parentPort) {
  throw new Error('Matou Runtime must be launched as an Electron UtilityProcess')
}

const servers = new Set<RuntimeServer>()
const sessions = new RuntimeSessionRegistry()
const sessionHuds = new SessionHudRegistry()
const dataRoot = resolve(process.env.MATOU_DATA_DIR ?? resolve(os.homedir(), '.matou'))
interface RuntimeState {
  dataRoot: string
  controlEndpoint: string
  database: RuntimeDatabase
  telemetry: TaskTelemetryRepository
  controlTokens: CapabilityTokenService
  controlBackend: RuntimeControlBackend
  rpcRouter: RuntimeRpcRouter
  hostControl: HostControlServer
  providerHooks: ProviderHookServer
}

let runtimeState: RuntimeState | undefined
const lifecycleCoordinator = new RuntimeLifecycleCoordinator()
const runtimeReady = lifecycleCoordinator.startInitialization(initializeRuntime).then((state) => {
  runtimeState = state
  return state
})

async function initializeRuntime(): Promise<RuntimeState> {
  const opened = await openRecoverableRuntimeDatabase(dataRoot, FOUNDATION_MIGRATIONS, {
    onDatabaseOpened: (database, _effectiveDataRoot, backups) => {
      lifecycleCoordinator.registerDatabaseLifecycle(
        database,
        new DatabaseLifecycleService(database, backups)
      )
    },
    onDatabaseClosed: (database) => {
      lifecycleCoordinator.releaseDatabaseLifecycle(database)
    },
    isShutdownRequested: () => lifecycleCoordinator.shutdownRequested
  })
  lifecycleCoordinator.assertStartupActive()
  const database = opened.database
  const runtimeDataRoot = opened.effectiveDataRoot
  const controlEndpoint = controlEndpointForPlatform(runtimeDataRoot)
  if (opened.recoveredFromCorruption) {
    console.error(`[runtime.storage] corrupt database quarantined at ${opened.quarantinedPath}`)
  }
  const telemetry = new TaskTelemetryRepository(database, database.runtimeGeneration)
  const notifications = new NotificationProjection()
  const transactions = new DomainTransactionManager(database)
  const sessionRepository = new SessionRepository(database, transactions)
  const providerModes = new ProviderModeService(database, transactions)
  const workStatuses = new SessionWorkStatusService(database, transactions)
  const sessionCanvas = new SessionCanvasService(database, transactions)
  const controlTokens = new CapabilityTokenService(database.runtimeGeneration)
  const controlBackend = new RuntimeControlBackend(database, runtimeDataRoot, telemetry, notifications)
  const rpcRouter = new RuntimeRpcRouter(database, notifications)
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
    dataRoot: runtimeDataRoot,
    controlEndpoint,
    database,
    telemetry,
    controlTokens,
    controlBackend,
    rpcRouter,
    hostControl,
    providerHooks
  }
}

function shutdown(): Promise<void> {
  return lifecycleCoordinator.shutdown(runtimeReady, {
    closeIncoming: () => {
      for (const server of servers) server.close()
      servers.clear()
    },
    shutdownSessions: () => sessions.shutdownAll()
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
    const server = new RuntimeServer(port, state.dataRoot, state.database, state.rpcRouter, {
      backend: state.controlBackend,
      tokens: state.controlTokens,
      endpoint: state.controlEndpoint
    }, sessions, state.providerHooks, undefined, { hudRegistry: sessionHuds })
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
