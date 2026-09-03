import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { SessionRepository } from '../domain/session-repository'
import { SessionForkIntentRepository } from './session-fork-intent-repository'

let database: RuntimeDatabase
let intents: SessionForkIntentRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-fork-intent-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const workspaces = new WorkspaceTaskRepository(database, transactions)
  workspaces.createWorkspace(command('workspace'), {
    id: 'workspace-1', name: 'Workspace', rootDirectory: root, now: 1
  })
  workspaces.createPlainExecutionContext(command('context'), {
    id: 'context-1', workspaceId: 'workspace-1', cwd: root, now: 1
  })
  workspaces.createTask(command('task'), {
    id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1',
    title: 'Task', status: 'active', sortKey: 'a', now: 1
  })
  const sessions = new SessionRepository(database, transactions)
  sessions.createSession(command('source'), {
    id: 'source-1', taskId: 'task-1', executionContextId: 'context-1',
    kind: 'claude-code', title: 'Source', now: 1
  })
  sessions.createSession(command('child'), {
    id: 'child-1', taskId: 'task-1', executionContextId: 'context-1',
    kind: 'claude-code', title: 'Child', now: 1
  })
  intents = new SessionForkIntentRepository(database)
  database.run(
    `INSERT INTO session_fork_intents (
       session_id, source_session_id, source_provider, source_provider_session_id,
       permission_mode, state, created_at, started_at, attempt_count, updated_at
     ) VALUES ('child-1', 'source-1', 'claude-code', 'provider-source',
               'bypassPermissions', 'starting', 1, 2, 1, 2)`
  )
})

function command(commandId: string) {
  return { commandId, commandType: 'fork-intent', requestHash: `hash-${commandId}` }
}

afterEach(() => database.close())

describe('SessionForkIntentRepository', () => {
  it('relaunches an interrupted provisional Fork from the original source conversation', () => {
    expect(intents.claimForLaunch('child-1', 10)).toEqual({
      kind: 'launch', sourceSessionId: 'source-1',
      sourceProviderSessionId: 'provider-source', permissionMode: 'bypassPermissions'
    })
    expect(database.get(
      `SELECT state, started_at, attempt_count, updated_at, error_message
       FROM session_fork_intents WHERE session_id = 'child-1'`
    )).toEqual({
      state: 'starting', started_at: 10, attempt_count: 2,
      updated_at: 10, error_message: null
    })
  })

  it('accepts one durable identity per submission key and returns it unchanged on retry', () => {
    database.run("DELETE FROM session_fork_intents WHERE session_id = 'child-1'")
    database.run(
      `INSERT INTO sessions (
         id, task_id, execution_context_id, kind, status, title,
         created_at, updated_at, last_activity_at
       ) VALUES ('child-2', 'task-1', 'context-1', 'claude-code', 'starting',
                 'Child 2', 2, 2, 2)`
    )
    const first = intents.accept(acceptInput())
    const duplicate = intents.accept({
      ...acceptInput(), operationId: 'operation-other', sessionId: 'child-2',
      worktreeId: 'worktree-other', executionContextId: 'worktree-context-other'
    })
    const distinct = intents.accept({
      ...acceptInput(), operationId: 'operation-2', submissionKey: 'submission-2',
      sessionId: 'child-2', worktreeId: 'worktree-2',
      executionContextId: 'worktree-context-2'
    })

    expect(first.created).toBe(true)
    expect(duplicate).toEqual({ ...first, created: false })
    expect(duplicate.identity).toEqual({
      operationId: 'operation-1', submissionKey: 'submission-1', sessionId: 'child-1',
      worktreeId: 'worktree-1', executionContextId: 'worktree-context-1',
      worktreePath: '/tmp/worktree-1', branchName: 'feature/child-1'
    })
    expect(database.get<{ count: number }>(
      "SELECT COUNT(*) AS count FROM session_fork_intents WHERE submission_key = 'submission-1'"
    )?.count).toBe(1)
    expect(distinct).toMatchObject({
      created: true,
      identity: {
        operationId: 'operation-2', submissionKey: 'submission-2', sessionId: 'child-2',
        worktreeId: 'worktree-2', executionContextId: 'worktree-context-2'
      }
    })
  })

  it('fences every late stage, complete and fail write after an expired lease is taken over', () => {
    database.run("DELETE FROM session_fork_intents WHERE session_id = 'child-1'")
    intents.accept(acceptInput())
    const first = intents.acquireLease({ operationId: 'operation-1', owner: 'runtime-a', now: 10, ttlMs: 5 })
    expect(first.kind).toBe('acquired')
    if (first.kind !== 'acquired') throw new Error('lease A missing')
    expect(intents.advanceStage({
      operationId: 'operation-1', lease: first.lease,
      stage: 'creating-worktree', now: 11
    })).toMatchObject({ kind: 'applied', progress: { stage: 'creating-worktree' } })

    expect(intents.heartbeat({
      operationId: 'operation-1', lease: first.lease, now: 15, ttlMs: 5
    })).toEqual({ kind: 'stale' })
    expect(intents.advanceStage({
      operationId: 'operation-1', lease: first.lease,
      stage: 'applying-setup', now: 15
    })).toEqual({ kind: 'stale' })
    expect(intents.complete('operation-1', first.lease, 15)).toEqual({ kind: 'stale' })
    expect(intents.failOperation({
      operationId: 'operation-1', lease: first.lease, error: 'expired failure', now: 15
    })).toEqual({ kind: 'stale' })

    const second = intents.acquireLease({ operationId: 'operation-1', owner: 'runtime-b', now: 16, ttlMs: 5 })
    expect(second.kind).toBe('acquired')
    if (second.kind !== 'acquired') throw new Error('lease B missing')
    expect(second.lease.fence).toBeGreaterThan(first.lease.fence)
    expect(intents.advanceStage({
      operationId: 'operation-1', lease: first.lease,
      stage: 'applying-setup', now: 17
    })).toEqual({ kind: 'stale' })
    expect(intents.complete('operation-1', first.lease, 17)).toEqual({ kind: 'stale' })
    expect(intents.failOperation({
      operationId: 'operation-1', lease: first.lease, error: 'late failure', now: 17
    })).toEqual({ kind: 'stale' })
    expect(intents.advanceStage({
      operationId: 'operation-1', lease: second.lease,
      stage: 'applying-setup', now: 18
    })).toMatchObject({ kind: 'applied', progress: { stage: 'applying-setup' } })
  })

  it('keeps unfenced launch and failure compatibility limited to inactive legacy operations', () => {
    database.run(
      `UPDATE session_fork_intents
       SET operation_id = 'legacy-operation:child-1', submission_key = 'legacy-submission:child-1',
           lease_owner = 'runtime-a', lease_token = 'legacy-token', lease_expires_at = 20,
           lease_fence = 1 WHERE session_id = 'child-1'`
    )

    expect(intents.claimForLaunch('child-1', 10)).toBeUndefined()
    intents.fail('child-1', 'must not bypass active lease', 10)
    expect(database.get(
      'SELECT state, error_message FROM session_fork_intents WHERE session_id = ?', 'child-1'
    )).toEqual({ state: 'starting', error_message: null })

    expect(intents.claimForLaunch('child-1', 20)).toMatchObject({ kind: 'launch' })
    intents.fail('child-1', 'legacy failure', 21)
    expect(database.get(
      'SELECT state, stage, error_message FROM session_fork_intents WHERE session_id = ?', 'child-1'
    )).toEqual({ state: 'failed', stage: 'failed', error_message: 'legacy failure' })

    database.run("DELETE FROM session_fork_intents WHERE session_id = 'child-1'")
    intents.accept(acceptInput())
    expect(intents.claimForLaunch('child-1', 30)).toBeUndefined()
    intents.fail('child-1', 'durable failure without fence', 30)
    expect(database.get(
      'SELECT state, stage, error_message FROM session_fork_intents WHERE session_id = ?', 'child-1'
    )).toEqual({ state: 'pending', stage: 'queued', error_message: null })
  })

  it('allows only ordered stages or failed and retries from the failed resumable stage', () => {
    database.run("DELETE FROM session_fork_intents WHERE session_id = 'child-1'")
    intents.accept(acceptInput())
    const decision = intents.acquireLease({
      operationId: 'operation-1', owner: 'runtime-a', now: 10, ttlMs: 100
    })
    if (decision.kind !== 'acquired') throw new Error('lease missing')
    expect(() => intents.advanceStage({
      operationId: 'operation-1', lease: decision.lease,
      stage: 'binding-session', now: 11
    })).toThrow('must advance')
    expect(intents.advanceStage({
      operationId: 'operation-1', lease: decision.lease,
      stage: 'creating-worktree', now: 12
    })).toMatchObject({ kind: 'applied', progress: { completedSteps: 0 } })
    expect(intents.advanceStage({
      operationId: 'operation-1', lease: decision.lease,
      stage: 'applying-setup', now: 13
    })).toMatchObject({ kind: 'applied', progress: { completedSteps: 1 } })
    expect(intents.failOperation({
      operationId: 'operation-1', lease: decision.lease,
      error: 'setup failed', now: 14
    })).toMatchObject({ kind: 'applied', progress: { stage: 'failed', error: 'setup failed' } })

    expect(intents.retry('operation-1', 15)).toMatchObject({
      stage: 'applying-setup', completedSteps: 1, attempt: 1
    })
  })
})

function acceptInput() {
  return {
    operationId: 'operation-1', submissionKey: 'submission-1',
    sessionId: 'child-1', sourceSessionId: 'source-1',
    sourceProviderSessionId: 'provider-source', displayName: 'Child',
    permissionMode: 'bypassPermissions' as const,
    worktreeMode: 'new' as const, worktreeId: 'worktree-1',
    executionContextId: 'worktree-context-1', worktreePath: '/tmp/worktree-1',
    branchName: 'feature/child-1', totalSteps: 5, now: 3
  }
}
