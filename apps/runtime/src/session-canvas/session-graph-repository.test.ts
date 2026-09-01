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
      sharedWorkingDirectory: true,
      activeChildCount: 2,
      stoppedChildCount: 1,
      childModeCounts: { shell: 1, claudeCode: 1 }
    })
    expect(graph.nodes.find(({ sessionId }) => sessionId === 'claude-child')).toMatchObject({
      parentSessionId: 'parent',
      relationKind: 'forked-from',
      currentMode: 'claude-code',
      workStatus: 'idle',
      siblingCreatedSeq: 3,
      latestLines: ['line one', 'line two'],
      lastUserInteractionSeq: 12
    })
  })

  it('projects the durable work status independently from terminal process lifetime', () => {
    database.run("UPDATE sessions SET work_status = 'running' WHERE id = 'shell-child'")
    database.run("UPDATE sessions SET work_status = 'needs-input' WHERE id = 'claude-child'")

    const graph = graphs.projectSceneGraph('scene')

    expect(graph.nodes.find(({ sessionId }) => sessionId === 'shell-child'))
      .toMatchObject({ currentMode: 'shell', workStatus: 'running' })
    expect(graph.nodes.find(({ sessionId }) => sessionId === 'claude-child'))
      .toMatchObject({ currentMode: 'claude-code', workStatus: 'needs-input' })
  })

  it('keeps the node, history, and DAG edge when its owned Worktree is missing', () => {
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('missing-context', 'workspace', 'git-worktree', '/tmp/missing-worktree', 10)`
    )
    database.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         state, created_at, updated_at
       ) VALUES ('missing-worktree', 'missing-context', '/tmp/workspace',
                 '/tmp/missing-worktree', 'codex/missing', 'failed', 10, 10)`
    )
    database.run(
      `UPDATE session_environment_bindings
       SET managed_worktree_id = 'missing-worktree', active_target = 'worktree',
           state = 'missing', error_message = 'path-missing', updated_at = 11
       WHERE session_id = 'claude-child'`
    )
    database.run(
      `INSERT INTO session_graph_summaries (session_id, latest_lines_json, updated_at)
       VALUES ('claude-child', '["persisted line"]', 11)`
    )

    const graph = graphs.projectSceneGraph('scene')
    const node = graph.nodes.find(({ sessionId }) => sessionId === 'claude-child')

    expect(node).toMatchObject({
      parentSessionId: 'parent',
      relationKind: 'forked-from',
      latestLines: ['persisted line'],
      environment: {
        kind: 'worktree', state: 'missing', path: '/tmp/missing-worktree',
        localExecutionContextId: 'context', worktreeId: 'missing-worktree',
        worktreeExecutionContextId: 'missing-context', error: 'path-missing'
      },
      git: { state: 'unavailable', dirty: false }
    })
    expect(graph.edges).toContainEqual(expect.objectContaining({
      parentSessionId: 'parent', childSessionId: 'claude-child'
    }))
    expect(graph.nodes).toHaveLength(4)
  })

  it('keeps a read-only-compatible legacy v21 graph browsable before binding migration', async () => {
    database.close()
    const root = await mkdtemp(join(tmpdir(), 'matou-session-graph-v21-'))
    const path = join(root, 'matou.sqlite')
    database = RuntimeDatabase.open(path)
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS.slice(0, 21)).migrate()
    graphs = new SessionGraphRepository(database, new DomainTransactionManager(database))
    database.run(
      `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
       VALUES ('workspace', 'Workspace', '/tmp/workspace', 1, 1)`
    )
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('context', 'workspace', 'plain-directory', '/tmp/workspace', 1)`
    )
    database.run(
      `INSERT INTO tasks (
         id, workspace_id, execution_context_id, title, status, created_at, updated_at
       ) VALUES ('task', 'workspace', 'context', 'Task', 'active', 1, 1)`
    )
    database.run(
      `INSERT INTO scenes (
         id, task_id, name, mode, created_at, updated_at, title_pinned, sort_key,
         layout_revision
       ) VALUES ('scene', 'task', 'Scene', 'tile', 1, 1, 0, 'a', 1)`
    )
    for (const [id, createdAt] of [['parent', 1], ['child', 2]] as const) {
      database.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at
         ) VALUES (?, 'task', 'context', 'shell', 'created', ?, '/tmp/workspace', ?, ?, ?)`,
        id, id, createdAt, createdAt, createdAt
      )
      database.run(
        `INSERT INTO session_canvas_memberships (
           session_id, scene_id, sibling_created_seq, last_user_interaction_seq,
           created_at, updated_at
         ) VALUES (?, 'scene', ?, 0, ?, ?)`,
        id, createdAt, createdAt, createdAt
      )
    }
    insertRelation('legacy', 'child', 'parent', 'derived-from', 3)
    database.run(
      `INSERT INTO session_graph_summaries (session_id, latest_lines_json, updated_at)
       VALUES ('child', '["legacy history"]', 3)`
    )
    database.close()
    database = RuntimeDatabase.openReadOnly(path)
    graphs = new SessionGraphRepository(database, new DomainTransactionManager(database))

    const graph = graphs.projectSceneGraph('scene')

    expect(graph.nodes).toHaveLength(2)
    expect(graph.nodes.find(({ sessionId }) => sessionId === 'child')).toMatchObject({
      parentSessionId: 'parent', latestLines: ['legacy history']
    })
    expect(graph.nodes.find(({ sessionId }) => sessionId === 'child'))
      .not.toHaveProperty('environment')
    expect(graph.edges).toContainEqual(expect.objectContaining({
      parentSessionId: 'parent', childSessionId: 'child'
    }))
  })

  it('marks every live node that actually shares the same working directory', () => {
    for (const id of ['parent', 'shell-child', 'claude-child'] as const) {
      database.run(
        `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
         VALUES (?, 'workspace', 'plain-directory', '/tmp/workspace', 10)`,
        `context-${id}`
      )
      database.run('UPDATE sessions SET execution_context_id = ? WHERE id = ?', `context-${id}`, id)
    }

    const graph = graphs.projectSceneGraph('scene')

    for (const id of ['parent', 'shell-child', 'claude-child']) {
      expect(graph.nodes.find(({ sessionId }) => sessionId === id))
        .toMatchObject({ sharedWorkingDirectory: true })
    }
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
