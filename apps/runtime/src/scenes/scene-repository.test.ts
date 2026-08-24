import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { SessionRepository } from '../domain/session-repository'
import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { GeometryRepository, GeometryWriteBuffer } from './geometry-repository'
import { SceneRepository } from './scene-repository'

let database: RuntimeDatabase
let scenes: SceneRepository
let geometry: GeometryRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-scene-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const workspaces = new WorkspaceTaskRepository(database, transactions)
  workspaces.createWorkspace(command('workspace'), { id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1 })
  workspaces.createPlainExecutionContext(command('context'), { id: 'context-1', workspaceId: 'workspace-1', cwd: '/tmp/workspace', now: 1 })
  workspaces.createTask(command('task'), { id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1', title: 'Task', status: 'active', sortKey: 'a', now: 1 })
  new SessionRepository(database, transactions).createSession(command('session'), {
    id: 'session-1', taskId: 'task-1', executionContextId: 'context-1', kind: 'shell', title: 'Shell', now: 2
  })
  scenes = new SceneRepository(database, transactions)
  geometry = new GeometryRepository(database)
})

afterEach(() => database.close())

describe('SceneRepository', () => {
  it('persists Scene structure and Session mounts as domain events', () => {
    scenes.createScene(command('scene'), {
      id: 'scene-1', rootNodeId: 'node-root', taskId: 'task-1', name: 'Main', mode: 'tile', now: 3
    })
    scenes.attachWindow(command('window'), {
      id: 'window-1', sceneId: 'scene-1', nativeWindowKey: 'main', state: 'attached', now: 4
    })
    scenes.addNode(command('split'), {
      id: 'node-split', sceneId: 'scene-1', parentNodeId: 'node-root',
      kind: 'split', direction: 'horizontal', ordinal: 0, now: 5
    })
    scenes.mountSession(command('mount'), {
      id: 'mount-1', sceneId: 'scene-1', sceneNodeId: 'node-split',
      sceneWindowId: 'window-1', sessionId: 'session-1', now: 6
    })
    scenes.setMode(command('mode'), 'scene-1', 'dag', 7)

    expect(scenes.snapshot('scene-1')).toMatchObject({
      scene: { id: 'scene-1', mode: 'dag', rootNodeId: 'node-root' },
      nodes: [{ id: 'node-root' }, { id: 'node-split' }],
      mounts: [{ id: 'mount-1', sessionId: 'session-1', sceneWindowId: 'window-1' }],
      windows: [{ id: 'window-1', state: 'attached' }]
    })
    expect(database.all<{ event_type: string }>(
      "SELECT event_type FROM domain_events WHERE aggregate_type = 'scene' ORDER BY seq"
    ).map(({ event_type }) => event_type)).toEqual([
      'scene.created', 'scene.window-attached', 'scene.node-added', 'scene.session-mounted', 'scene.mode-changed'
    ])
  })

  it('does not allow mounting a Session from another Task', () => {
    scenes.createScene(command('scene'), {
      id: 'scene-1', rootNodeId: 'root', taskId: 'task-1', name: 'Main', mode: 'tile', now: 3
    })
    database.run("UPDATE sessions SET task_id = 'task-1' WHERE id = 'session-1'")

    expect(() => scenes.mountSession(command('bad-mount'), {
      id: 'mount-1', sceneId: 'scene-1', sceneNodeId: 'root', sessionId: 'missing', now: 4
    })).toThrow('mounted Session must belong to the Scene Task')
  })
})

describe('GeometryRepository', () => {
  beforeEach(() => {
    scenes.createScene(command('scene'), {
      id: 'scene-1', rootNodeId: 'root', taskId: 'task-1', name: 'Main', mode: 'card', now: 3
    })
  })

  it('writes geometry directly without adding Outbox noise and rejects stale revisions', () => {
    const before = database.get<{ count: number }>('SELECT COUNT(*) AS count FROM domain_events')!.count
    geometry.put({ sceneId: 'scene-1', ownerKey: 'node:root', layoutRevision: 2, geometry: { x: 20 }, now: 5 })

    expect(() => geometry.put({
      sceneId: 'scene-1', ownerKey: 'node:root', layoutRevision: 1, geometry: { x: 10 }, now: 6
    })).toThrow('stale layout revision 1; current revision is 2')
    expect(geometry.get('scene-1', 'node:root')).toMatchObject({ layoutRevision: 2, geometry: { x: 20 } })
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM domain_events')!.count).toBe(before)
  })

  it('coalesces high-frequency updates to the latest revision', async () => {
    vi.useFakeTimers()
    const buffer = new GeometryWriteBuffer(geometry, 50)
    buffer.schedule({ sceneId: 'scene-1', ownerKey: 'node:root', layoutRevision: 1, geometry: { x: 1 }, now: 4 })
    buffer.schedule({ sceneId: 'scene-1', ownerKey: 'node:root', layoutRevision: 2, geometry: { x: 2 }, now: 5 })

    await vi.advanceTimersByTimeAsync(50)
    expect(geometry.get('scene-1', 'node:root')).toMatchObject({ layoutRevision: 2, geometry: { x: 2 } })
    vi.useRealTimers()
  })

  it('discards geometry whose structural owner no longer exists', () => {
    geometry.put({ sceneId: 'scene-1', ownerKey: 'node:root', layoutRevision: 1, geometry: {}, now: 4 })
    database.run("DELETE FROM scene_nodes WHERE id = 'root'")

    expect(geometry.discardInvalid('scene-1')).toBe(1)
    expect(geometry.get('scene-1', 'node:root')).toBeUndefined()
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'scene', requestHash: `hash-${commandId}` }
}
