import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { GeometryRepository, GeometryWriteBuffer } from './geometry-repository'

let database: RuntimeDatabase
let geometry: GeometryRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-canvas-geometry-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  geometry = new GeometryRepository(database)
  database.run("INSERT INTO workspaces (id,name,root_directory,created_at,updated_at) VALUES ('w','W','/tmp',1,1)")
  database.run("INSERT INTO execution_contexts (id,workspace_id,kind,cwd,created_at) VALUES ('c','w','plain-directory','/tmp',1)")
  database.run("INSERT INTO tasks (id,workspace_id,execution_context_id,title,status,created_at,updated_at) VALUES ('t','w','c','T','active',1,1)")
  for (const sceneId of ['scene-a', 'scene-b']) {
    database.run("INSERT INTO scenes (id,task_id,name,mode,created_at,updated_at) VALUES (?,'t',?,'tile',1,1)", sceneId, sceneId)
  }
  database.run("INSERT INTO sessions (id,task_id,execution_context_id,kind,status,title,cwd,created_at,updated_at,last_activity_at) VALUES ('s','t','c','shell','created','S','/tmp',1,1,1)")
  database.run("INSERT INTO session_canvas_memberships (session_id,scene_id,sibling_created_seq,last_user_interaction_seq,created_at,updated_at) VALUES ('s','scene-a',1,0,1,1)")
})
afterEach(() => database.close())

describe('canvas geometry owner keys', () => {
  it('isolates per-level scroll/focus, DAG viewport and stable node positions by Scene', () => {
    geometry.put(update('scene-a', 'session-group:scene-a:root', { scrollLeft: 80, focusedSessionId: 's' }))
    geometry.put(update('scene-a', 'session-group:scene-a:s', { scrollLeft: 22, focusedSessionId: 's' }))
    geometry.put(update('scene-a', 'dag-viewport:scene-a', { panX: 10, panY: 20, zoom: 1.2 }))
    geometry.put(update('scene-a', 'dag-node-layout:scene-a:s', { x: 300, y: 200 }))
    geometry.put(update('scene-b', 'session-group:scene-b:root', { scrollLeft: 5 }))

    expect(geometry.list('scene-a').map(({ ownerKey }) => ownerKey)).toEqual([
      'dag-node-layout:scene-a:s', 'dag-viewport:scene-a',
      'session-group:scene-a:root', 'session-group:scene-a:s'
    ])
    expect(new GeometryRepository(database).get('scene-a', 'dag-viewport:scene-a')?.geometry)
      .toEqual({ panX: 10, panY: 20, zoom: 1.2 })
    expect(geometry.get('scene-b', 'session-group:scene-b:root')?.geometry).toEqual({ scrollLeft: 5 })
  })

  it('debounces high-frequency frames for 180ms and flushes the latest once', async () => {
    vi.useFakeTimers()
    const put = vi.spyOn(geometry, 'put')
    const buffer = new GeometryWriteBuffer(geometry)
    buffer.schedule(update('scene-a', 'dag-viewport:scene-a', { panX: 1 }))
    buffer.schedule(update('scene-a', 'dag-viewport:scene-a', { panX: 2 }))
    await vi.advanceTimersByTimeAsync(179)
    expect(put).not.toHaveBeenCalled()
    buffer.flush()
    expect(put).toHaveBeenCalledTimes(1)
    expect(geometry.get('scene-a', 'dag-viewport:scene-a')?.geometry).toEqual({ panX: 2 })
    vi.useRealTimers()
  })
})

function update(sceneId: string, ownerKey: string, value: unknown) {
  return { sceneId, ownerKey, layoutRevision: 0, geometry: value, now: 10 }
}
