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
import { openRecoverableRuntimeDatabase } from './storage/runtime-database-bootstrap'
import { DetachedSessionService } from './hierarchy/detached-session-service'
import { DomainTransactionManager } from './storage/domain-transaction'
import { NotificationProjection } from './product/experience-foundation'
import { RuntimeRpcRouter } from './rpc/runtime-rpc-router'
import { AgentNotificationRepository } from './notifications/agent-notification-repository'

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
const runtimeReady = initializeRuntime().then((state) => {
  runtimeState = state
  return state
})

async function initializeRuntime(): Promise<RuntimeState> {
  const opened = await openRecoverableRuntimeDatabase(dataRoot, FOUNDATION_MIGRATIONS)
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
  const controlTokens = new CapabilityTokenService(database.runtimeGeneration)
  const controlBackend = new RuntimeControlBackend(database, runtimeDataRoot, telemetry, notifications)
  const rpcRouter = new RuntimeRpcRouter(database, notifications)
  const hostControl = new HostControlServer({
    socketPath: controlEndpoint,
    tokenService: controlTokens,
    backend: controlBackend
  })
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
      for (const server of servers) server.flushSemanticEvents()
    },
    onHudPayload: ({ sessionId, payload }) => {
      sessionHuds.ingestProvider(sessionId, payload)
      for (const server of servers) void server.refreshSessionHud(sessionId)
    },
    onIdentityRecorded: ({ sessionId, runId }) => {
      sessionHuds.markResumable(sessionId)
      for (const server of servers) {
        server.providerIdentityRecorded(sessionId, runId)
        void server.refreshSessionHud(sessionId)
      }
    }
  })
  new DetachedSessionService(database, transactions)
    .normalizeOnStartup(Date.now())
  const recovery = await new RuntimeRecoveryService(runtimeDataRoot, database).recoverAll()
  for (const failure of recovery.failed) {
    console.error(`[runtime.recovery] ${failure.sessionId} ${failure.code}: ${failure.message}`)
  }
  telemetry.purgeStaleGenerations()
  await hostControl.start()
  await providerHooks.start()
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

let shutdownStarted = false
function shutdown(): void {
  if (shutdownStarted) return
  shutdownStarted = true
  for (const server of servers) server.close()
  servers.clear()
  sessions.disposeAll()
  runtimeState?.providerHooks.stop()
  runtimeState?.hostControl.stop()
  runtimeState?.database.close()
}
process.once('SIGTERM', () => {
  shutdown()
  process.exit(0)
})
process.once('exit', shutdown)

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
    if (shutdownStarted) {
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
