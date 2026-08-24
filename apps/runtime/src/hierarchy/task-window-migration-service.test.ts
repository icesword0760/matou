import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { HierarchyApplicationService } from './hierarchy-application-service'
import { TaskWindowMigrationService } from './task-window-migration-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let hierarchy: HierarchyApplicationService
let migrations: TaskWindowMigrationService

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-task-window-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  migrations = new TaskWindowMigrationService(database, transactions)
})
afterEach(() => database.close())

describe('TaskWindowMigrationService', () => {
  it('moves the complete Task placement without changing Workspace ownership', () => {
    const initial = bootstrap()
    registerWindow('window-2')
    const pending = migrations.prepare(command('move-1'), {
      migrationId: 'migration-1', taskId: initial.task!.id,
      sourceWindowId: 'window-1', targetWindowId: 'window-2', now: 80
    })
    expect(pending.state).toBe('preparing')

    migrations.acknowledgeTarget(command('move-ack'), {
      migrationId: pending.id, now: 81
    })
    expect(readPlacement(initial.task!.id)).toEqual({ window_id: 'window-2' })
    expect(database.get<{ workspace_id: string }>('SELECT workspace_id FROM tasks WHERE id = ?', initial.task!.id)?.workspace_id)
      .toBe(initial.workspace!.id)
  })

  it('restores the source placement when target closes before acknowledgement', () => {
    const initial = bootstrap()
    registerWindow('window-2')
    const pending = migrations.prepare(command('move-2'), {
      migrationId: 'migration-2', taskId: initial.task!.id,
      sourceWindowId: 'window-1', targetWindowId: 'window-2', now: 80
    })
    migrations.fail(command('move-fail'), {
      migrationId: pending.id, reason: 'target-closed', now: 82
    })

    expect(readPlacement(initial.task!.id)).toEqual({ window_id: 'window-1' })
    expect(migrations.get(pending.id)?.state).toBe('failed')
  })

  it('rejects an overlapping migration for the same Task', () => {
    const initial = bootstrap()
    registerWindow('window-2')
    migrations.prepare(command('move-1'), {
      migrationId: 'migration-1', taskId: initial.task!.id,
      sourceWindowId: 'window-1', targetWindowId: 'window-2', now: 80
    })
    expect(() => migrations.prepare(command('move-2'), {
      migrationId: 'migration-2', taskId: initial.task!.id,
      sourceWindowId: 'window-1', targetWindowId: 'window-2', now: 81
    })).toThrow(/already|overlap/i)
  })
})

function bootstrap() {
  return hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: '/tmp/task-window-workspace',
    defaultName: 'task-window-workspace', now: 1
  })
}
function registerWindow(id: string) {
  database.run(
    "INSERT INTO app_windows (id, kind, state, created_at, updated_at) VALUES (?, 'main', 'visible', 2, 2)", id
  )
}
function readPlacement(taskId: string) {
  return database.get<{ window_id: string }>(
    'SELECT window_id FROM window_task_placements WHERE task_id = ?', taskId
  )
}
function command(commandId: string) {
  return { commandId, commandType: 'test', requestHash: `hash-${commandId}` }
}
