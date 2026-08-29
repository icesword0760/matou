import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { SessionGraphRepository } from './session-graph-repository'

let database: RuntimeDatabase
let graphs: SessionGraphRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-session-graph-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  graphs = new SessionGraphRepository(database, new DomainTransactionManager(database))

  database.run(
    `INSERT INTO workspaces (
       id, name, root_directory, created_at, updated_at, last_opened_at
     ) VALUES ('workspace', 'Workspace', '/tmp/workspace', 1, 1, 1)`
  )
  database.run(
    `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
     VALUES ('context', 'workspace', 'plain-directory', '/tmp/workspace', 1)`
  )
  database.run(
    `INSERT INTO tasks (
       id, workspace_id, execution_context_id, title, status, created_at, updated_at,
       sort_key, last_opened_at
     ) VALUES ('task', 'workspace', 'context', 'Task', 'active', 1, 1, 'a', 1)`
  )
  database.run(
    `INSERT INTO scenes (
       id, task_id, name, mode, created_at, updated_at, title_pinned, sort_key, layout_revision
     ) VALUES ('scene', 'task', 'Scene', 'tile', 1, 1, 0, 'a', 1)`
  )

  const sessions = [
    ['parent', 'shell', 'running', null, 1],
    ['shell-child', 'shell', 'running', null, 2],
    ['claude-child', 'claude-code', 'waiting', null, 3],
    ['history-child', 'shell', 'archived', 9, 4],
    ['new-session', 'shell', 'created', null, 5]
  ] as const
  for (const [id, kind, status, archivedAt, createdAt] of sessions) {
    database.run(
      `INSERT INTO sessions (
         id, task_id, execution_context_id, kind, status, title, cwd,
         created_at, updated_at, last_activity_at, archived_at
       ) VALUES (?, 'task', 'context', ?, ?, ?, '/tmp/workspace', ?, ?, ?, ?)`,
      id, kind, status, id, createdAt, createdAt, createdAt, archivedAt
    )
  }
  for (const [sessionId, createdSeq, interactionSeq] of [
    ['parent', 1, 8],
    ['shell-child', 2, 0],
    ['claude-child', 3, 12],
    ['history-child', 4, 0]
  ] as const) {
    database.run(
      `INSERT INTO session_canvas_memberships (
         session_id, scene_id, sibling_created_seq, last_user_interaction_seq, created_at, updated_at
       ) VALUES (?, 'scene', ?, ?, 2, 2)`,
      sessionId, createdSeq, interactionSeq
    )
  }
  insertRelation('derived', 'shell-child', 'parent', 'derived-from', 4)
  insertRelation('fork', 'claude-child', 'parent', 'forked-from', 5)
  insertRelation('history', 'history-child', 'parent', 'derived-from', 6)
})

afterEach(() => database.close())

describe('SessionGraphRepository', () => {
  it('projects active and historical mixed-mode children with stable structural edges', () => {
    database.run(
      `INSERT INTO session_graph_summaries (session_id, latest_lines_json, updated_at)
       VALUES ('claude-child', '["line one","line two"]', 20)`
    )
    const graph = graphs.projectSceneGraph('scene')

    expect(graph.nodes).toHaveLength(4)
    expect(graph.edges).toEqual(expect.arrayContaining([
      expect.objectContaining({ parentSessionId: 'parent', childSessionId: 'shell-child', relationKind: 'derived-from' }),
      expect.objectContaining({ parentSessionId: 'parent', childSessionId: 'claude-child', relationKind: 'forked-from' }),
      expect.objectContaining({ parentSessionId: 'parent', childSessionId: 'history-child', relationKind: 'derived-from' })
    ]))
    expect(graph.nodes.find(({ sessionId }) => sessionId === 'parent')).toMatchObject({
      activeChildCount: 2,
      historicalChildCount: 1,
      childModeCounts: { shell: 1, claudeCode: 1 }
    })
    expect(graph.nodes.find(({ sessionId }) => sessionId === 'claude-child')).toMatchObject({
      parentSessionId: 'parent',
      relationKind: 'forked-from',
      currentMode: 'claude-code',
      workStatus: 'needs-input',
      siblingCreatedSeq: 3,
      latestLines: ['line one', 'line two'],
      lastUserInteractionSeq: 12
    })
  })

  it('allocates persistent sequences and creates one membership with its Outbox event', () => {
    const first = graphs.nextSequence('session-sibling-created')
    const second = graphs.nextSequence('session-sibling-created')

    const membership = graphs.createMembership(command('membership'), {
      sessionId: 'new-session',
      sceneId: 'scene',
      siblingCreatedSeq: second,
      lastUserInteractionSeq: 0,
      now: 20
    }).result

    expect(second).toBe(first + 1)
    expect(graphs.getMembership('new-session')).toEqual(membership)
    expect(database.get(
      "SELECT event_type FROM domain_events WHERE event_type = 'session.canvas-membership-created'"
    )).toEqual({ event_type: 'session.canvas-membership-created' })
  })
})

function insertRelation(
  id: string,
  childSessionId: string,
  parentSessionId: string,
  kind: 'derived-from' | 'forked-from',
  now: number
): void {
  const event = database.run(
    `INSERT INTO session_relation_events (
       event_id, relation_id, operation, task_id, from_session_id, to_session_id,
       relation_kind, metadata_json, command_id, occurred_at
     ) VALUES (?, ?, 'created', 'task', ?, ?, ?, '{}', ?, ?)`,
    `event-${id}`, `relation-${id}`, childSessionId, parentSessionId, kind, `command-${id}`, now
  )
  database.run(
    `INSERT INTO session_relations_current (
       relation_id, task_id, from_session_id, to_session_id, relation_kind,
       metadata_json, created_at, updated_at, source_event_sequence
     ) VALUES (?, 'task', ?, ?, ?, '{}', ?, ?, ?)`,
    `relation-${id}`, childSessionId, parentSessionId, kind, now, now, Number(event.lastInsertRowid)
  )
}

function command(commandId: string) {
  return { commandId, commandType: 'session-canvas', requestHash: `hash-${commandId}` }
}
