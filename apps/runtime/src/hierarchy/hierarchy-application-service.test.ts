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

describe('HierarchyApplicationService Task workflows', () => {
  it('chooses the lowest available user Task name and preserves explicit order', async () => {
    const initial = await bootstrap('task-bootstrap')
    markPathValid(initial.workspace!.id)
    const first = await service.createTask(command('task-new-1'), {
      windowId: 'window-1', workspaceId: initial.workspace!.id, now: 20
    })
    const second = await service.createTask(command('task-new-2'), {
      windowId: 'window-1', workspaceId: initial.workspace!.id, now: 21
    })
    await service.renameTask(command('task-rename'), {
      taskId: second.task!.id, title: '新事项 3', now: 22
    })
    const created = await service.createTask(command('task-new-3'), {
      windowId: 'window-1', workspaceId: initial.workspace!.id, now: 23
    })

    expect(first.task?.title).toBe('新事项')
    expect(created.task?.title).toBe('新事项 2')
    const reordered = await service.reorderTask(command('task-order'), {
      windowId: 'window-1', workspaceId: initial.workspace!.id,
      taskId: created.task!.id, beforeTaskId: first.task!.id, now: 24
    })
    expect(reordered.taskOrder).toEqual([
      initial.task!.id, created.task!.id, first.task!.id, second.task!.id
    ])
  })

  it('deletes a confirmed final Task and atomically replaces it with 默认', async () => {
    const initial = await bootstrap('delete-bootstrap')
    markPathValid(initial.workspace!.id)

    const result = await service.deleteTask(command('delete-final'), {
      windowId: 'window-1',
      taskId: initial.task!.id,
      confirmedIntent: `delete-task:${initial.task!.id}`,
      now: 40
    })

    expect(result.disposedSessionIds).toContain(initial.session!.id)
    expect(result.task).toMatchObject({ title: '默认' })
    expect(result.task?.id).not.toBe(initial.task!.id)
    expect(database.all<{ title: string }>(
      `SELECT title FROM tasks
       WHERE workspace_id = ? AND archived_at IS NULL`,
      initial.workspace!.id
    )).toEqual([{ title: '默认' }])
  })

  it('blocks new Task hierarchy creation when the Workspace path is invalid', async () => {
    const initial = await bootstrap('invalid-bootstrap')
    database.run(
      `INSERT INTO workspace_path_state (
         workspace_id, status, reason, checked_at, validation_generation
       ) VALUES (?, 'invalid', 'missing', 20, 1)`,
      initial.workspace!.id
    )

    expect(() => service.createTask(command('invalid-task'), {
      windowId: 'window-1', workspaceId: initial.workspace!.id, now: 21
    })).toThrow('工作区目录不可用，请先在本地恢复原路径，或移出该工作区')
  })
})

describe('HierarchyApplicationService Scene workflows', () => {
  it('protects the last Scene of the last Task by returning hide-window', () => {
    const initial = bootstrap('scene-protected-bootstrap')
    markPathValid(initial.workspace!.id)

    const result = service.closeScene(command('close-protected'), {
      windowId: 'window-1', sceneId: initial.scene!.id, now: 30
    })

    expect(result.action).toBe('hide-window')
    expect(database.get<{ archived_at: number | null }>(
      'SELECT archived_at FROM scenes WHERE id = ?', initial.scene!.id
    )?.archived_at).toBeNull()
    expect(result.disposedSessionIds).toEqual([])
  })

  it('closes a non-last Scene, disposes its Session, and focuses its successor', () => {
    const initial = bootstrap('scene-close-bootstrap')
    markPathValid(initial.workspace!.id)
    const second = service.createScene(command('scene-new'), {
      windowId: 'window-1', taskId: initial.task!.id, now: 20
    })
    service.activateScene({
      windowId: 'window-1', sceneId: initial.scene!.id, now: 21
    })

    const result = service.closeScene(command('scene-close'), {
      windowId: 'window-1', sceneId: initial.scene!.id, now: 22
    })

    expect(result.action).toBe('closed')
    expect(result.disposedSessionIds).toContain(initial.session!.id)
    expect(result.scene?.id).toBe(second.scene?.id)
  })

  it('pins manual Scene titles and enforces uniqueness within one Task', () => {
    const initial = bootstrap('scene-title-bootstrap')
    markPathValid(initial.workspace!.id)
    const second = service.createScene(command('scene-new'), {
      windowId: 'window-1', taskId: initial.task!.id, now: 20
    })
    service.renameScene(command('scene-title-1'), {
      sceneId: initial.scene!.id, name: '发布检查', now: 21
    })

    expect(() => service.renameScene(command('scene-title-2'), {
      sceneId: second.scene!.id, name: '发布检查', now: 22
    })).toThrow('当前事项下已存在同名页签')
  })

  it('requires the Scene close intent before removing a final Scene when another Task exists', () => {
    const initial = bootstrap('scene-task-cascade-bootstrap')
    markPathValid(initial.workspace!.id)
    const otherTask = service.createTask(command('scene-other-task'), {
      windowId: 'window-1', workspaceId: initial.workspace!.id, now: 20
    })
    service.activateScene({
      windowId: 'window-1', sceneId: initial.scene!.id, now: 21
    })

    expect(() => service.closeScene(command('scene-stale-close'), {
      windowId: 'window-1', sceneId: initial.scene!.id, now: 22
    })).toThrow('Scene close intent is stale')
    const closed = service.closeScene(command('scene-confirmed-close'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      confirmedIntent: `close-scene:${initial.scene!.id}`, now: 23
    })

    expect(closed.action).toBe('closed')
    expect(closed.task?.id).toBe(otherTask.task?.id)
    expect(database.get<{ archived_at: number | null }>(
      'SELECT archived_at FROM tasks WHERE id = ?', initial.task!.id
    )?.archived_at).toBe(23)
  })
})

describe('HierarchyApplicationService Session workflows', () => {
  it('deletes a sibling mount while preserving the Scene', () => {
    const initial = bootstrap('session-sibling-bootstrap')
    markPathValid(initial.workspace!.id)
    const split = service.splitSession(command('session-split'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, direction: 'horizontal', now: 20
    })
    expect(database.get(
      "SELECT kind, direction FROM scene_nodes WHERE scene_id = ? AND kind = 'split'",
      initial.scene!.id
    )).toEqual({ kind: 'split', direction: 'horizontal' })

    const deleted = service.deleteSession(command('session-delete-sibling'), {
      windowId: 'window-1', sessionId: split.session!.id, now: 21
    })

    expect(deleted.outcome).toBe('scene-remains')
    expect(deleted.scene?.id).toBe(initial.scene!.id)
    expect(deleted.disposedSessionIds).toEqual([split.session!.id])
  })

  it('deletes a final Session and creates a fresh default Task hierarchy', () => {
    const initial = bootstrap('session-final-bootstrap')
    markPathValid(initial.workspace!.id)

    const deleted = service.deleteSession(command('session-delete-final'), {
      windowId: 'window-1', sessionId: initial.session!.id,
      confirmedIntent: `delete-session:${initial.session!.id}`, now: 30
    })

    expect(deleted.outcome).toBe('default-task-created')
    expect(deleted.task).toMatchObject({ title: '默认' })
    expect(deleted.task?.id).not.toBe(initial.task!.id)
    expect(deleted.disposedSessionIds).toEqual([initial.session!.id])
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

function bootstrap(commandId: string) {
  return service.bootstrapWindow(command(commandId), {
    windowId: 'window-1',
    defaultRootDirectory: workspaceRoot,
    defaultName: 'matou_workspace',
    now: 10
  })
}

function markPathValid(workspaceId: string): void {
  database.run(
    `INSERT INTO workspace_path_state (
       workspace_id, status, reason, checked_at, validation_generation
     ) VALUES (?, 'valid', '', 11, 1)`,
    workspaceId
  )
}
