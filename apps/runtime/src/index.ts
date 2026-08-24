import type { MessagePortMain, ParentPort } from 'electron'
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
import { RuntimeDatabase } from './storage/database'
import { RuntimeRecoveryService } from './recovery/runtime-recovery-service'
import { RuntimeSessionRegistry } from './session/runtime-session-registry'
import { MigrationRunner } from './storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from './storage/migrations'
import { DetachedSessionService } from './hierarchy/detached-session-service'
import { DomainTransactionManager } from './storage/domain-transaction'

type UtilityProcess = NodeJS.Process & { parentPort?: ParentPort }

const parentPort = (process as UtilityProcess).parentPort

if (!parentPort) {
  throw new Error('Matou Runtime must be launched as an Electron UtilityProcess')
}

const servers = new Set<RuntimeServer>()
const sessions = new RuntimeSessionRegistry()
const dataRoot = resolve(process.env.MATOU_DATA_DIR ?? resolve(os.homedir(), '.matou'))
const database = RuntimeDatabase.open(resolve(dataRoot, 'matou.sqlite'))
const migrationReady = new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
const telemetry = new TaskTelemetryRepository(database, database.runtimeGeneration)
const controlTokens = new CapabilityTokenService(database.runtimeGeneration)
const controlBackend = new RuntimeControlBackend(database, dataRoot, telemetry)
const controlEndpoint = controlEndpointForPlatform(dataRoot)
const hostControl = new HostControlServer({
  socketPath: controlEndpoint,
  tokenService: controlTokens,
  backend: controlBackend
})
const runtimeReady = migrationReady.then(async () => {
  new DetachedSessionService(database, new DomainTransactionManager(database))
    .normalizeOnStartup(Date.now())
  const recovery = await new RuntimeRecoveryService(dataRoot, database).recoverAll()
  for (const failure of recovery.failed) {
    console.error(`[runtime.recovery] ${failure.sessionId} ${failure.code}: ${failure.message}`)
  }
  telemetry.purgeStaleGenerations()
  await hostControl.start()
})

process.once('exit', () => {
  sessions.disposeAll()
  database.close()
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
    await runtimeReady
  } catch (error) {
    console.error('Matou Runtime schema migration failed', error)
    port.close()
    process.exitCode = 1
    return
  }

  const server = new RuntimeServer(port, dataRoot, database, undefined, {
    backend: controlBackend,
    tokens: controlTokens,
    endpoint: controlEndpoint
  }, sessions)
  servers.add(server)
  port.once('close', () => servers.delete(server))
})
