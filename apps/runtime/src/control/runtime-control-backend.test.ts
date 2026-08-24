import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeControlBackend } from './runtime-control-backend'
import { TaskTelemetryRepository } from '../domain/product-foundation-repository'
import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { NotificationProjection } from '../product/experience-foundation'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let root: string
let database: RuntimeDatabase

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-control-backend-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
})

afterEach(() => database.close())

describe('RuntimeControlBackend Task information channel', () => {
  it('turns an external error log into Task unread feedback without changing focus', async () => {
    const hierarchy = new HierarchyApplicationService(database, new DomainTransactionManager(database))
    const initial = hierarchy.bootstrapWindow(command('bootstrap'), {
      windowId: 'window-1', defaultRootDirectory: root, defaultName: 'Workspace', now: 1
    })
    const notifications = new NotificationProjection({ cooldownMs: 0 })
    const backend = new RuntimeControlBackend(
      database, root,
      new TaskTelemetryRepository(database, database.runtimeGeneration),
      notifications
    )

    await backend.appendTaskLog(initial.task!.id, 'error', 'claude-code', 'Build failed')

    expect(notifications.list()).toEqual([
      expect.objectContaining({
        type: 'error', taskId: initial.task!.id, sessionId: initial.session!.id,
        mountId: initial.mount!.id, subtitle: 'claude-code', body: 'Build failed', read: false
      })
    ])
  })

  it('rejects writes to a deleted Task without affecting live Task information', async () => {
    const hierarchy = new HierarchyApplicationService(database, new DomainTransactionManager(database))
    const initial = hierarchy.bootstrapWindow(command('bootstrap-delete'), {
      windowId: 'window-1', defaultRootDirectory: root, defaultName: 'Workspace', now: 1
    })
    const telemetry = new TaskTelemetryRepository(database, database.runtimeGeneration)
    const backend = new RuntimeControlBackend(database, root, telemetry, new NotificationProjection())
    await backend.writeTaskStatus(initial.task!.id, 'phase', 'running')
    await backend.writeTaskProgress(initial.task!.id, 50, 'half')
    await backend.appendTaskLog(initial.task!.id, 'info', 'agent', 'working')
    hierarchy.deleteTask(command('delete'), {
      windowId: 'window-1', taskId: initial.task!.id,
      confirmedIntent: `delete-task:${initial.task!.id}`, now: 2
    })

    await expect(backend.writeTaskStatus(initial.task!.id, 'phase', 'late')).rejects.toThrow('does not exist')
    expect(telemetry.snapshot(initial.task!.id)).toEqual({ status: {}, progress: undefined, logs: [] })
  })
})

function command(commandId: string) {
  return { commandId, commandType: commandId, requestHash: `hash-${commandId}` }
}
