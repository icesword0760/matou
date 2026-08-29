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

  it('keeps the home Workspace as the protected system default', async () => {
    const initial = await service.bootstrapWindow(command('bootstrap-1'), {
      windowId: 'window-1',
      defaultRootDirectory: workspaceRoot,
      defaultName: 'matou_workspace',
      now: 10
    })
    const workspaceId = initial.workspace!.id

    expect(initial.workspace).toMatchObject({
      name: 'matou_workspace', isDefault: true, isPinned: true, lastOpenedAt: 10
    })
    expect(() => service.renameWorkspace(command('rename-default'), {
      workspaceId, name: 'Renamed', now: 19
    })).toThrow('工作空间名称跟随目录名称')
    expect(() => service.removeWorkspace(command('remove-1'), {
      windowId: 'window-1',
      workspaceId,
      confirmedIntent: `remove-workspace:${workspaceId}`,
      now: 20
    })).toThrow('默认工作空间会保留在侧栏中')
    expect(() => service.relinkWorkspace(command('relink-default'), {
      workspaceId, rootDirectory: join(testRoot, 'other'), now: 21
    })).toThrow('默认工作空间始终指向 macOS 用户目录')
    await expect(access(workspaceRoot)).resolves.toBeUndefined()
  })

  it('adds the home default even when custom Workspaces already exist', async () => {
    const customRoot = join(testRoot, 'custom')
    await mkdir(customRoot)
    const custom = await service.createWorkspace(command('custom-first'), {
      windowId: 'window-1', name: 'Custom', rootDirectory: customRoot, now: 5
    })

    const bootstrapped = await service.bootstrapWindow(command('bootstrap-home'), {
      windowId: 'window-1', defaultRootDirectory: workspaceRoot,
      defaultName: 'icesword', now: 10
    })

    const rows = database.all<{ root_directory: string; is_default: number }>(
      'SELECT root_directory, is_default FROM workspaces ORDER BY created_at'
    )
    expect(rows).toEqual([
      { root_directory: custom.workspace!.rootDirectory, is_default: 0 },
      { root_directory: workspaceRoot, is_default: 1 }
    ])
    expect(bootstrapped.navigation.activeWorkspaceId).toBe(custom.workspace!.id)
  })

  it('keeps Workspace names bound to their directory instead of offering a separate rename', async () => {
    const first = await service.createWorkspace(command('create-1'), {
      windowId: 'window-1', name: 'First', rootDirectory: workspaceRoot, now: 10
    })
    expect(() => service.renameWorkspace(command('rename-1'), {
      workspaceId: first.workspace!.id, name: 'Renamed', now: 11
    })).toThrow('工作空间名称跟随目录名称')

    const activated = await service.activateWorkspace({
      windowId: 'window-2', workspaceId: first.workspace!.id, now: 12
    })
    expect(activated.navigation.activeWorkspaceId).toBe(first.workspace!.id)
    expect(activated.task?.id).toBe(first.task?.id)
    expect(activated.workspace?.name).toBe('First')
  })

  it('relinks an invalid Workspace while keeping its Tasks and sessions', async () => {
    const initial = await service.createWorkspace(command('relink-create'), {
      windowId: 'window-1', name: 'Moved', rootDirectory: workspaceRoot, now: 10
    })
    const replacement = join(testRoot, 'replacement')
    await mkdir(replacement)
    const relinked = await service.relinkWorkspace(command('relink-workspace'), {
      workspaceId: initial.workspace!.id, rootDirectory: replacement, now: 20
    })

    expect(relinked.rootDirectory).toBe(replacement)
    expect(database.get<{ cwd: string }>(
      'SELECT cwd FROM execution_contexts WHERE id = ?', initial.executionContext!.id
    )?.cwd).toBe(replacement)
    expect(database.get<{ cwd: string }>(
      'SELECT cwd FROM sessions WHERE id = ?', initial.session!.id
    )?.cwd).toBe(replacement)
    expect(database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM tasks WHERE workspace_id = ? AND archived_at IS NULL',
      initial.workspace!.id
    )?.count).toBe(1)
  })

  it('pins Workspaces and reorders only the pinned group', async () => {
    const roots = await Promise.all(['one', 'two', 'three'].map(async (name) => {
      const root = join(testRoot, name); await mkdir(root); return root
    }))
    const one = await service.createWorkspace(command('create-one'), {
      windowId: 'window-1', name: 'One', rootDirectory: roots[0]!, now: 10
    })
    const two = await service.createWorkspace(command('create-two'), {
      windowId: 'window-1', name: 'Two', rootDirectory: roots[1]!, now: 20
    })
    const three = await service.createWorkspace(command('create-three'), {
      windowId: 'window-1', name: 'Three', rootDirectory: roots[2]!, now: 30
    })
    await service.setWorkspacePinned(command('pin-one'), {
      workspaceId: one.workspace!.id, pinned: true, now: 40
    })
    await service.setWorkspacePinned(command('pin-three'), {
      workspaceId: three.workspace!.id, pinned: true, now: 41
    })
    await service.reorderPinnedWorkspace(command('reorder-pinned-workspace'), {
      workspaceId: three.workspace!.id, beforeWorkspaceId: one.workspace!.id, now: 42
    })

    const pinned = database.all<{ id: string }>(
      `SELECT id FROM workspaces WHERE is_pinned = 1 ORDER BY pin_sort_key, id`
    ).map(({ id }) => id)
    expect(pinned).toEqual([three.workspace!.id, one.workspace!.id])
    expect(database.get<{ is_pinned: number }>(
      'SELECT is_pinned FROM workspaces WHERE id = ?', two.workspace!.id
    )?.is_pinned).toBe(0)
  })
})

describe('HierarchyApplicationService Task workflows', () => {
  it('updates navigation recency only after terminal input is submitted', async () => {
    const initial = await bootstrap('recency-bootstrap')
    markPathValid(initial.workspace!.id)
    const other = await service.createTask(command('recency-other'), {
      windowId: 'window-1', workspaceId: initial.workspace!.id, now: 20
    })
    await service.activateTask({ windowId: 'window-1', taskId: initial.task!.id, now: 30 })

    expect(database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', initial.task!.id
    )?.last_opened_at).toBe(10)
    expect(database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', other.task!.id
    )?.last_opened_at).toBe(0)
    expect(database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM workspaces WHERE id = ?', initial.workspace!.id
    )?.last_opened_at).toBe(10)

    service.recordSessionInteraction(command('recency-submit'), {
      sessionId: initial.session!.id, now: 40
    })

    expect(database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', initial.task!.id
    )?.last_opened_at).toBe(40)
    expect(database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM workspaces WHERE id = ?', initial.workspace!.id
    )?.last_opened_at).toBe(40)
  })

  it('pins Tasks and reorders only pinned Tasks inside their Workspace', async () => {
    const initial = await bootstrap('pin-task-bootstrap')
    markPathValid(initial.workspace!.id)
    const second = await service.createTask(command('pin-task-second'), {
      windowId: 'window-1', workspaceId: initial.workspace!.id, now: 20
    })
    const third = await service.createTask(command('pin-task-third'), {
      windowId: 'window-1', workspaceId: initial.workspace!.id, now: 30
    })
    expect(new Set([initial.task!.id, second.task!.id, third.task!.id]).size).toBe(3)
    await service.setTaskPinned(command('pin-task-first'), {
      taskId: initial.task!.id, pinned: true, now: 40
    })
    await service.setTaskPinned(command('pin-task-third-action'), {
      taskId: third.task!.id, pinned: true, now: 41
    })
    expect(database.all<{ id: string; is_pinned: number }>(
      'SELECT id, is_pinned FROM tasks WHERE workspace_id = ? ORDER BY created_at',
      initial.workspace!.id
    )).toEqual(expect.arrayContaining([
      { id: initial.task!.id, is_pinned: 1 },
      { id: third.task!.id, is_pinned: 1 }
    ]))
    await service.reorderPinnedTask(command('reorder-pinned-task'), {
      workspaceId: initial.workspace!.id, taskId: third.task!.id,
      beforeTaskId: initial.task!.id, now: 42
    })

    const pinned = database.all<{ id: string }>(
      `SELECT id FROM tasks WHERE workspace_id = ? AND is_pinned = 1 ORDER BY pin_sort_key, id`,
      initial.workspace!.id
    ).map(({ id }) => id)
    expect(pinned).toEqual([third.task!.id, initial.task!.id])
    expect(database.get<{ is_pinned: number }>(
      'SELECT is_pinned FROM tasks WHERE id = ?', second.task!.id
    )?.is_pinned).toBe(0)
  })
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
      taskId: first.task!.id, beforeTaskId: created.task!.id, now: 24
    })
    expect(reordered.taskOrder).toEqual([
      initial.task!.id, second.task!.id, created.task!.id, first.task!.id
    ])
    expect(reordered.navigation.taskByWorkspace[initial.workspace!.id]).toBe(created.task!.id)
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

  it('keeps an idle Task discoverable after archiving its final Scene', () => {
    const initial = bootstrap('scene-task-cascade-bootstrap')
    markPathValid(initial.workspace!.id)
    const otherTask = service.createTask(command('scene-other-task'), {
      windowId: 'window-1', workspaceId: initial.workspace!.id, now: 20
    })
    service.activateScene({
      windowId: 'window-1', sceneId: initial.scene!.id, now: 21
    })

    const closed = service.closeScene(command('scene-idle-close'), {
      windowId: 'window-1', sceneId: initial.scene!.id, now: 23
    })

    expect(closed.action).toBe('closed')
    expect(closed.task?.id).toBe(initial.task!.id)
    expect(closed.scene).toBeNull()
    expect(database.get<{ archived_at: number | null }>(
      'SELECT archived_at FROM tasks WHERE id = ?', initial.task!.id
    )?.archived_at).toBeNull()
  })

  it('requires fresh confirmation before closing a Scene with running work', () => {
    const initial = bootstrap('scene-busy-close-bootstrap')
    markPathValid(initial.workspace!.id)
    service.createScene(command('scene-busy-close-second'), {
      windowId: 'window-1', taskId: initial.task!.id, now: 20
    })
    database.run("UPDATE sessions SET work_status = 'running' WHERE id = ?", initial.session!.id)

    expect(() => service.closeScene(command('scene-busy-stale-close'), {
      windowId: 'window-1', sceneId: initial.scene!.id, now: 21
    })).toThrow('Scene close intent is stale')
    const closed = service.closeScene(command('scene-busy-confirmed-close'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      confirmedIntent: `close-scene:${initial.scene!.id}`, now: 22
    })
    expect(closed.action).toBe('closed')
  })

  it('reopens a closed Scene with its historical graph and focuses it again', () => {
    const initial = bootstrap('scene-reopen-bootstrap')
    markPathValid(initial.workspace!.id)
    const second = service.createScene(command('scene-reopen-second'), {
      windowId: 'window-1', taskId: initial.task!.id, now: 20
    })
    service.closeScene(command('scene-close-for-reopen'), {
      windowId: 'window-1', sceneId: initial.scene!.id, now: 21
    })

    const reopened = service.reopenScene(command('scene-reopen'), {
      windowId: 'window-1', sceneId: initial.scene!.id, now: 22
    })

    expect(reopened.scene?.id).toBe(initial.scene!.id)
    expect(reopened.scene?.archivedAt).toBeUndefined()
    expect(reopened.session).toBeNull()
    expect(database.all<{ id: string }>(
      `SELECT id FROM scenes WHERE task_id = ? AND archived_at IS NULL
       ORDER BY sort_key, created_at, id`, initial.task!.id
    ).map(({ id }) => id)).toEqual([initial.scene!.id, second.scene!.id])
    expect(database.get<{ active_scene_id: string | null }>(
      'SELECT active_scene_id FROM window_task_focus WHERE window_id = ? AND task_id = ?',
      'window-1', initial.task!.id
    )?.active_scene_id).toBe(initial.scene!.id)
  })
})

describe('HierarchyApplicationService Session workflows', () => {
  it('creates a focused Claude fork immediately to the source right and persists its parent edge', () => {
    const initial = bootstrap('session-fork-bootstrap')
    markPathValid(initial.workspace!.id)
    database.run(
      "UPDATE sessions SET kind = 'claude-code', title = 'Claude', cwd = '/tmp/source-subdir' WHERE id = ?",
      initial.session!.id
    )
    database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state, metadata_json,
         created_at, updated_at, validated_at, invalidated_at
       ) VALUES (?, ?, 'claude-code', ?, 'available', '{}', 11, 11, 11, NULL)`,
      'binding-source', initial.session!.id, 'provider-source'
    )

    const forked = service.forkSession(command('session-fork'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })

    expect(forked.session).toMatchObject({
      taskId: initial.task!.id,
      executionContextId: initial.executionContext!.id,
      kind: 'claude-code', title: 'Claude', cwd: '/tmp/source-subdir'
    })
    expect(forked.navigation.sessionByScene[initial.scene!.id]).toBe(forked.session!.id)
    expect(database.get(
      `SELECT direction FROM scene_nodes
       WHERE scene_id = ? AND kind = 'split' AND parent_node_id IS NULL`,
      initial.scene!.id
    )).toEqual({ direction: 'horizontal' })
    expect(database.get(
      `SELECT ordinal FROM scene_nodes
       JOIN session_mounts ON session_mounts.scene_node_id = scene_nodes.id
       WHERE session_mounts.session_id = ?`, forked.session!.id
    )).toEqual({ ordinal: 1 })
    expect(database.get(
      `SELECT source_session_id, source_provider_session_id, state
       FROM session_fork_intents WHERE session_id = ?`, forked.session!.id
    )).toEqual({
      source_session_id: initial.session!.id,
      source_provider_session_id: 'provider-source',
      state: 'pending'
    })
    expect(database.get(
      `SELECT from_session_id, to_session_id, relation_kind
       FROM session_relations_current WHERE from_session_id = ?`, forked.session!.id
    )).toEqual({
      from_session_id: forked.session!.id,
      to_session_id: initial.session!.id,
      relation_kind: 'forked-from'
    })
  })

  it('rejects Shell, detached, team-member, and identity-less fork sources without changing layout', () => {
    const initial = bootstrap('session-fork-ineligible-bootstrap')
    markPathValid(initial.workspace!.id)
    const beforeNodes = database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM scene_nodes WHERE scene_id = ?', initial.scene!.id
    )!.count

    expect(() => service.forkSession(command('fork-shell'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })).toThrow('only resumable Claude Sessions can be forked')
    expect(database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM scene_nodes WHERE scene_id = ?', initial.scene!.id
    )!.count).toBe(beforeNodes)

    database.run(
      "UPDATE sessions SET kind = 'claude-code', title = 'Claude' WHERE id = ?",
      initial.session!.id
    )
    expect(() => service.forkSession(command('fork-identity-less'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 21
    })).toThrow('only resumable Claude Sessions can be forked')

    database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state, metadata_json,
         created_at, updated_at, validated_at, invalidated_at
       ) VALUES (?, ?, 'claude-code', ?, 'available', '{}', 22, 22, 22, NULL)`,
      'binding-ineligible-source', initial.session!.id, 'provider-ineligible-source'
    )
    database.run(
      `INSERT INTO scene_windows (
         id, scene_id, native_window_key, state, created_at, updated_at
       ) VALUES ('detached-window', ?, 'detached-native-window', 'detached', 23, 23)`,
      initial.scene!.id
    )
    database.run(
      `UPDATE session_mounts SET scene_window_id = 'detached-window'
       WHERE scene_id = ? AND session_id = ?`,
      initial.scene!.id, initial.session!.id
    )
    expect(() => service.forkSession(command('fork-detached'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 23
    })).toThrow('only resumable Claude Sessions can be forked')

    database.run(
      'UPDATE session_mounts SET scene_window_id = NULL WHERE scene_id = ? AND session_id = ?',
      initial.scene!.id, initial.session!.id
    )
    database.run(
      "UPDATE sessions SET kind = 'agent-team-member' WHERE id = ?",
      initial.session!.id
    )
    expect(() => service.forkSession(command('fork-team-member'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 24
    })).toThrow('only resumable Claude Sessions can be forked')

    expect(database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM scene_nodes WHERE scene_id = ?', initial.scene!.id
    )!.count).toBe(beforeNodes)
    expect(database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM session_fork_intents'
    )!.count).toBe(0)
  })

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

  it('requires confirmation before ending a parent Session and retains its child relation', () => {
    const initial = bootstrap('session-parent-close-bootstrap')
    markPathValid(initial.workspace!.id)
    const child = service.splitSession(command('session-parent-child'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, direction: 'horizontal', now: 20
    })
    const relationInsert = database.run(
      `INSERT INTO session_relation_events (
         event_id, relation_id, operation, task_id, from_session_id, to_session_id,
         relation_kind, metadata_json, command_id, occurred_at
       ) VALUES ('parent-close-event', 'parent-close-relation', 'created', ?, ?, ?,
                 'derived-from', '{}', 'parent-close-command', 21)`,
      initial.task!.id, child.session!.id, initial.session!.id
    )
    database.run(
      `INSERT INTO session_relations_current (
         relation_id, task_id, from_session_id, to_session_id, relation_kind,
         metadata_json, created_at, updated_at, source_event_sequence
       ) VALUES ('parent-close-relation', ?, ?, ?, 'derived-from', '{}', 21, 21, ?)`,
      initial.task!.id, child.session!.id, initial.session!.id, Number(relationInsert.lastInsertRowid)
    )

    expect(() => service.deleteSession(command('session-parent-close-stale'), {
      windowId: 'window-1', sessionId: initial.session!.id, now: 22
    })).toThrow('Session deletion intent is stale')
    service.deleteSession(command('session-parent-close-confirmed'), {
      windowId: 'window-1', sessionId: initial.session!.id,
      confirmedIntent: `delete-session:${initial.session!.id}`, now: 23
    })

    expect(database.get(
      `SELECT from_session_id, to_session_id FROM session_relations_current
       WHERE relation_id = 'parent-close-relation'`
    )).toEqual({ from_session_id: child.session!.id, to_session_id: initial.session!.id })
  })

  it('requires confirmation before ending a running Session even when it is a leaf', () => {
    const initial = bootstrap('session-running-close-bootstrap')
    markPathValid(initial.workspace!.id)
    service.splitSession(command('session-running-sibling'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, direction: 'horizontal', now: 20
    })
    database.run("UPDATE sessions SET work_status = 'running' WHERE id = ?", initial.session!.id)

    expect(() => service.deleteSession(command('session-running-close-stale'), {
      windowId: 'window-1', sessionId: initial.session!.id, now: 21
    })).toThrow('Session deletion intent is stale')
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
