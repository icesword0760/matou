import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeRpcRouter } from './runtime-rpc-router'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let router: RuntimeRpcRouter

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-rpc-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  router = new RuntimeRpcRouter(database)
})

afterEach(() => database.close())

describe('RuntimeRpcRouter', () => {
  it('routes atomic Workspace hierarchy workflows', async () => {
    const bootstrapped = await router.handle('hierarchy.bootstrap-window', payload('bootstrap', {
      windowId: 'window-1',
      defaultRootDirectory: '/tmp/matou_workspace',
      defaultName: 'matou_workspace',
      now: 1
    })) as { workspace: { id: string }; navigation: { activeWorkspaceId: string } }
    expect(bootstrapped.navigation.activeWorkspaceId).toBe(bootstrapped.workspace.id)

    const renamed = await router.handle('hierarchy.rename-workspace', payload('rename-workspace', {
      workspaceId: bootstrapped.workspace.id,
      name: 'Renamed',
      now: 2
    })) as { name: string }
    expect(renamed.name).toBe('Renamed')

    const activated = await router.handle('hierarchy.activate-workspace', payload('activate-workspace', {
      windowId: 'window-2',
      workspaceId: bootstrapped.workspace.id,
      now: 3
    })) as { navigation: { windowId: string; activeWorkspaceId: string } }
    expect(activated.navigation).toMatchObject({
      windowId: 'window-2',
      activeWorkspaceId: bootstrapped.workspace.id
    })

    database.run(
      `INSERT INTO workspace_path_state (
         workspace_id, status, reason, checked_at, validation_generation
       ) VALUES (?, 'valid', '', 4, 2)
       ON CONFLICT(workspace_id) DO UPDATE SET
         status = 'valid', reason = '', validation_generation = 2`,
      bootstrapped.workspace.id
    )
    const createdTask = await router.handle('hierarchy.create-task', payload('create-task', {
      windowId: 'window-2', workspaceId: bootstrapped.workspace.id, now: 4
    })) as { task: { id: string; title: string } }
    expect(createdTask.task.title).toBe('新事项')
    await router.handle('hierarchy.rename-task', payload('rename-task', {
      taskId: createdTask.task.id, title: '实施事项', now: 5
    }))
    const taskActivated = await router.handle(
      'hierarchy.activate-task',
      payload('activate-task', {
        windowId: 'window-1', taskId: createdTask.task.id, now: 6
      })
    ) as { task: { id: string } }
    expect(taskActivated.task.id).toBe(createdTask.task.id)
  })

  it('creates, modifies, archives, and projects the authoritative hierarchy', async () => {
    await router.handle('workspace.create', payload('workspace', {
      id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1
    }))
    await router.handle('workspace.update', payload('workspace-update', {
      id: 'workspace-1', name: 'Renamed', now: 2
    }))
    await router.handle('execution-context.create-plain', payload('context', {
      id: 'context-1', workspaceId: 'workspace-1', cwd: '/tmp/workspace', now: 2
    }))
    await router.handle('task.create', payload('task', {
      id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1',
      title: 'Task', status: 'active', sortKey: 'a', now: 3
    }))
    await router.handle('task.update', payload('task-update', {
      id: 'task-1', title: 'Updated Task', status: 'completed', now: 4
    }))
    await router.handle('session.create', payload('session', {
      id: 'session-1', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'shell', title: 'Shell', now: 5
    }))
    await router.handle('session.update', payload('session-update', {
      id: 'session-1', title: 'Renamed Shell', status: 'waiting', now: 6
    }))
    await router.handle('scene.create', payload('scene', {
      id: 'scene-1', rootNodeId: 'root-1', taskId: 'task-1', name: 'Main', mode: 'tile', now: 7
    }))
    await router.handle('scene.archive', payload('scene-archive', { id: 'scene-1', now: 8 }))
    await router.handle('session.archive', payload('session-archive', { id: 'session-1', now: 9 }))
    await router.handle('task.archive', payload('task-archive', { id: 'task-1', now: 10 }))
    await router.handle('workspace.archive', payload('workspace-archive', { id: 'workspace-1', now: 11 }))

    const snapshot = await router.handle('projection.snapshot', {}) as {
      runtimeGeneration: string
      eventSequence: number
      workspaces: Array<{ id: string; name: string; archivedAt?: number }>
      tasks: Array<{ id: string; title: string; status: string }>
      sessions: Array<{ id: string; title: string; status: string }>
      scenes: Array<{ id: string; archivedAt?: number }>
    }
    expect(snapshot.runtimeGeneration).toBe(database.runtimeGeneration)
    expect(snapshot.eventSequence).toBeGreaterThan(0)
    expect(snapshot.workspaces[0]).toMatchObject({ id: 'workspace-1', name: 'Renamed', archivedAt: 11 })
    expect(snapshot.tasks[0]).toMatchObject({ id: 'task-1', title: 'Updated Task', status: 'archived' })
    expect(snapshot.sessions[0]).toMatchObject({ id: 'session-1', title: 'Renamed Shell', status: 'archived' })
    expect(snapshot.scenes[0]).toMatchObject({ id: 'scene-1', archivedAt: 8 })
  })

  it('supports relation and Scene structural commands with synchronous event replay', async () => {
    await seedHierarchy()
    await router.handle('session.create', payload('parent', {
      id: 'parent', taskId: 'task-1', executionContextId: 'context-1', kind: 'shell', title: 'Parent', now: 4
    }))
    await router.handle('session.create', payload('child', {
      id: 'child', taskId: 'task-1', executionContextId: 'context-1', kind: 'shell', title: 'Child', now: 4
    }))
    await router.handle('relation.create', payload('relation', {
      id: 'relation-1', taskId: 'task-1', fromSessionId: 'child', toSessionId: 'parent',
      kind: 'forked-from', metadata: {}, now: 5
    }))
    await router.handle('scene.create', payload('scene', {
      id: 'scene-1', rootNodeId: 'root', taskId: 'task-1', name: 'Main', mode: 'tile', now: 5
    }))
    await router.handle('scene.mount-session', payload('mount', {
      id: 'mount-1', sceneId: 'scene-1', sceneNodeId: 'root', sessionId: 'child', now: 6
    }))
    await router.handle('geometry.put', {
      sceneId: 'scene-1', ownerKey: 'node:root', layoutRevision: 0,
      geometry: { ratio: 0.35 }, now: 7
    })

    const snapshot = await router.handle('projection.snapshot', {}) as {
      sceneSnapshots: Array<{ scene: { id: string }; geometry: Array<{ geometry: { ratio: number } }> }>
    }
    expect(snapshot.sceneSnapshots.find(({ scene }) => scene.id === 'scene-1')?.geometry)
      .toEqual([expect.objectContaining({ geometry: { ratio: 0.35 } })])

    const replay = await router.handle('events.replay', { afterSequence: 0, limit: 100 }) as {
      events: Array<{ eventType: string }>
    }
    expect(replay.events.some(({ eventType }) => eventType === 'session-relation.created')).toBe(true)
    expect(replay.events.some(({ eventType }) => eventType === 'scene.session-mounted')).toBe(true)
    await router.handle('events.ack', { consumerId: 'renderer-1', throughSequence: replay.events.length })
    expect(database.get('SELECT last_event_seq FROM consumer_cursors WHERE consumer_id = ?', 'renderer-1')).toEqual({
      last_event_seq: replay.events.length
    })
  })

  it('rejects malformed payloads before reaching repositories', async () => {
    await expect(router.handle('workspace.create', { command: {}, input: { name: '' } })).rejects.toMatchObject({
      code: 'INVALID_REQUEST'
    })
  })

  it('exposes prepare and target acknowledgement for whole-Task migration', async () => {
    const initial = await router.handle('hierarchy.bootstrap-window', payload('bootstrap-move', {
      windowId: 'window-1', defaultRootDirectory: '/tmp/move-workspace',
      defaultName: 'move-workspace', now: 1
    })) as { task: { id: string } }
    await router.handle('hierarchy.bootstrap-window', payload('bootstrap-target', {
      windowId: 'window-2', defaultRootDirectory: '/tmp/move-workspace',
      defaultName: 'move-workspace', now: 2
    }))
    await router.handle('hierarchy.move-task-to-window', payload('move-prepare', {
      phase: 'prepare', migrationId: 'migration-1', taskId: initial.task.id,
      sourceWindowId: 'window-1', targetWindowId: 'window-2', now: 3
    }))
    const committed = await router.handle('hierarchy.move-task-to-window', payload('move-ack', {
      phase: 'acknowledge', migrationId: 'migration-1', now: 4
    })) as { state: string }

    expect(committed.state).toBe('committed')
    expect(database.get('SELECT window_id FROM window_task_placements WHERE task_id = ?', initial.task.id))
      .toEqual({ window_id: 'window-2' })
  })

  async function seedHierarchy(): Promise<void> {
    await router.handle('workspace.create', payload('workspace', {
      id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1
    }))
    await router.handle('execution-context.create-plain', payload('context', {
      id: 'context-1', workspaceId: 'workspace-1', cwd: '/tmp/workspace', now: 2
    }))
    await router.handle('task.create', payload('task', {
      id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1',
      title: 'Task', status: 'active', sortKey: 'a', now: 3
    }))
  }
})

function payload(commandId: string, input: Record<string, unknown>) {
  return {
    command: { commandId, commandType: commandId, requestHash: `hash-${commandId}` },
    input
  }
}
