import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { SessionEnvironmentRepository } from './session-environment-repository'

let database: RuntimeDatabase
let environments: SessionEnvironmentRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-session-environment-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  environments = new SessionEnvironmentRepository(database)

  database.run(
    `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
     VALUES ('workspace', 'Workspace', '/tmp/workspace', 1, 1)`
  )
  database.run(
    `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
     VALUES ('local', 'workspace', 'plain-directory', '/tmp/workspace', 1),
            ('worktree-context', 'workspace', 'git-worktree', '/tmp/worktree', 1),
            ('other-worktree-context', 'workspace', 'git-worktree', '/tmp/other', 1)`
  )
  for (const [id, context, path, branch] of [
    ['worktree', 'worktree-context', '/tmp/worktree', 'codex/task-6'],
    ['other-worktree', 'other-worktree-context', '/tmp/other', 'codex/other']
  ] as const) {
    database.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         state, created_at, updated_at
       ) VALUES (?, ?, '/tmp/workspace', ?, ?, 'ready', 1, 1)`,
      id, context, path, branch
    )
  }
  database.run(
    `INSERT INTO tasks (
       id, workspace_id, execution_context_id, title, status, created_at, updated_at
     ) VALUES ('task', 'workspace', 'local', 'Task', 'active', 1, 1)`
  )
  for (const id of ['first', 'second']) {
    database.run(
      `INSERT INTO sessions (
         id, task_id, execution_context_id, kind, status, title, cwd,
         created_at, updated_at, last_activity_at
       ) VALUES (?, 'task', 'local', 'shell', 'running', ?, '/tmp/workspace', 1, 1, 1)`,
      id, id
    )
  }
})

afterEach(() => database.close())

describe('SessionEnvironmentRepository', () => {
  it('creates a local binding for an ordinary Session and projects its path', () => {
    expect(environments.get('first')).toEqual({
      sessionId: 'first',
      localExecutionContextId: 'local',
      activeTarget: 'local',
      state: 'ready',
      updatedAt: 1,
      environment: {
        kind: 'local', state: 'ready', path: '/tmp/workspace',
        localExecutionContextId: 'local'
      }
    })
  })

  it('keeps the owned Worktree identity after completing a Handoff to Local', () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'worktree', activate: true, now: 10
    })
    environments.beginTransition({
      sessionId: 'first', target: 'local', state: 'handoff', now: 11
    })
    environments.completeTransition({ sessionId: 'first', target: 'local', now: 12 })

    expect(environments.get('first')).toEqual({
      sessionId: 'first',
      localExecutionContextId: 'local',
      managedWorktreeId: 'worktree',
      activeTarget: 'local',
      state: 'ready',
      updatedAt: 12,
      environment: {
        kind: 'local', state: 'ready', path: '/tmp/workspace',
        localExecutionContextId: 'local'
      }
    })
    expect(environments.findOwningSession('worktree')).toBe('first')
    expect(database.get(
      "SELECT execution_context_id, cwd, status, archived_at FROM sessions WHERE id = 'first'"
    )).toEqual({
      execution_context_id: 'local', cwd: '/tmp/workspace',
      status: 'running', archived_at: null
    })
  })

  it('enforces exclusive Worktree ownership without stealing another Session binding', () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'worktree', activate: true, now: 10
    })

    expect(() => environments.bindOwnedWorktree({
      sessionId: 'second', worktreeId: 'worktree', activate: true, now: 11
    })).toThrow('Worktree worktree is already owned by Session first')
    expect(environments.get('second')).toMatchObject({
      activeTarget: 'local', state: 'ready'
    })
  })

  it('changes environment health without changing Session lifecycle or relationships', () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'worktree', activate: true, now: 10
    })
    database.run(
      `INSERT INTO session_relation_events (
         event_id, relation_id, operation, task_id, from_session_id, to_session_id,
         relation_kind, metadata_json, command_id, occurred_at
       ) VALUES ('event', 'relation', 'created', 'task', 'second', 'first',
                 'derived-from', '{}', 'command', 10)`
    )
    const eventSequence = Number(database.get<{ sequence: number }>(
      "SELECT sequence FROM session_relation_events WHERE event_id = 'event'"
    )!.sequence)
    database.run(
      `INSERT INTO session_relations_current (
         relation_id, task_id, from_session_id, to_session_id, relation_kind,
         metadata_json, created_at, updated_at, source_event_sequence
       ) VALUES ('relation', 'task', 'second', 'first', 'derived-from', '{}', 10, 10, ?)`,
      eventSequence
    )

    environments.markMissing('first', 'path-missing', 20)
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'missing',
      managedWorktreeId: 'worktree',
      environment: {
        kind: 'worktree', state: 'missing', path: '/tmp/worktree',
        worktreeId: 'worktree', worktreeExecutionContextId: 'worktree-context',
        error: 'path-missing'
      }
    })
    environments.markFailed('first', 'identity-mismatch', 21)

    expect(database.get(
      "SELECT status, archived_at FROM sessions WHERE id = 'first'"
    )).toEqual({ status: 'running', archived_at: null })
    expect(database.get(
      "SELECT relation_id FROM session_relations_current WHERE relation_id = 'relation'"
    )).toEqual({ relation_id: 'relation' })
    expect(environments.get('first')).toMatchObject({
      state: 'failed', environment: { state: 'failed', error: 'identity-mismatch' }
    })
  })
})
