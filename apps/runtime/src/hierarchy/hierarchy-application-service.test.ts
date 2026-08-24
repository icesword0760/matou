import { access, mkdir, mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { HierarchyApplicationService } from './hierarchy-application-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let service: HierarchyApplicationService
let testRoot: string
let workspaceRoot: string

beforeEach(async () => {
  testRoot = await mkdtemp(join(tmpdir(), 'matou-hierarchy-'))
  workspaceRoot = join(testRoot, 'matou_workspace')
  await mkdir(workspaceRoot)
  database = RuntimeDatabase.open(join(testRoot, 'data', 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  service = new HierarchyApplicationService(
    database,
    new DomainTransactionManager(database)
  )
})

afterEach(() => database.close())

describe('HierarchyApplicationService Workspace workflows', () => {
  it('creates one complete default hierarchy in one idempotent command', async () => {
    const first = await service.bootstrapWindow(command('bootstrap-1'), {
      windowId: 'window-1',
      defaultRootDirectory: workspaceRoot,
      defaultName: 'matou_workspace',
      now: 10
    })

    expect(first.navigation.activeWorkspaceId).toBe(first.workspace?.id)
    expect(first.task?.title).toBe('默认')
    expect(first.scene?.taskId).toBe(first.task?.id)
    expect(first.session?.executionContextId).toBe(first.executionContext?.id)
    expect(first.mount?.sessionId).toBe(first.session?.id)
    expect(eventTypes()).toEqual([
      'workspace.created',
      'task.created',
      'scene.created',
      'session.created',
      'scene.session-mounted'
    ])

    const replay = await service.bootstrapWindow(command('bootstrap-1'), {
      windowId: 'window-1',
      defaultRootDirectory: workspaceRoot,
      defaultName: 'matou_workspace',
      now: 10
    })
    expect(replay).toEqual(first)
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM workspaces')?.count).toBe(1)
  })

  it('reuses a normalized active Workspace path and activates it in the requesting window', async () => {
    const created = await service.createWorkspace(command('create-1'), {
      windowId: 'window-1',
      name: 'Product',
      rootDirectory: `${workspaceRoot}/.`,
      now: 10
    })
    const reused = await service.createWorkspace(command('create-2'), {
      windowId: 'window-2',
      name: 'Ignored duplicate name',
      rootDirectory: workspaceRoot,
      now: 11
    })

    expect(reused.workspace?.id).toBe(created.workspace?.id)
    expect(reused.navigation).toMatchObject({
      windowId: 'window-2',
      activeWorkspaceId: created.workspace?.id
    })
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM workspaces')?.count).toBe(1)
  })

  it('records explicit default removal, preserves the disk directory, and does not recreate it', async () => {
    const initial = await service.bootstrapWindow(command('bootstrap-1'), {
      windowId: 'window-1',
      defaultRootDirectory: workspaceRoot,
      defaultName: 'matou_workspace',
      now: 10
    })
    const workspaceId = initial.workspace!.id

    const removed = await service.removeWorkspace(command('remove-1'), {
      windowId: 'window-1',
      workspaceId,
      confirmedIntent: `remove-workspace:${workspaceId}`,
      now: 20
    })

    expect(removed.workspace).toBeNull()
    await expect(access(workspaceRoot)).resolves.toBeUndefined()
    expect(readBootstrapFlag('default-workspace-removed')).toBe(true)

    const next = await service.bootstrapWindow(command('bootstrap-2'), {
      windowId: 'window-1',
      defaultRootDirectory: workspaceRoot,
      defaultName: 'matou_workspace',
      now: 30
    })
    expect(next.workspace).toBeNull()
  })

  it('renames and activates an existing Workspace without changing its hierarchy', async () => {
    const first = await service.createWorkspace(command('create-1'), {
      windowId: 'window-1', name: 'First', rootDirectory: workspaceRoot, now: 10
    })
    const renamed = await service.renameWorkspace(command('rename-1'), {
      workspaceId: first.workspace!.id, name: '  Renamed  ', now: 11
    })
    const activated = await service.activateWorkspace({
      windowId: 'window-2', workspaceId: renamed.id, now: 12
    })

    expect(renamed.name).toBe('Renamed')
    expect(activated.navigation.activeWorkspaceId).toBe(renamed.id)
    expect(activated.task?.id).toBe(first.task?.id)
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'test', requestHash: `hash-${commandId}` }
}

function eventTypes(): string[] {
  return database
    .all<{ event_type: string }>('SELECT event_type FROM domain_events ORDER BY seq')
    .map(({ event_type }) => event_type)
}

function readBootstrapFlag(key: string): unknown {
  const row = database.get<{ value_json: string }>(
    'SELECT value_json FROM bootstrap_state WHERE key = ?', key
  )
  return row === undefined ? undefined : JSON.parse(row.value_json)
}
