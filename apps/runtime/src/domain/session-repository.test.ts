import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { WorkspaceTaskRepository } from './workspace-task-repository'
import { SessionRepository } from './session-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let sessions: SessionRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-session-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const workspaces = new WorkspaceTaskRepository(database, transactions)
  workspaces.createWorkspace(command('workspace'), { id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1 })
  workspaces.createPlainExecutionContext(command('context'), { id: 'context-1', workspaceId: 'workspace-1', cwd: '/tmp/workspace', now: 1 })
  workspaces.createTask(command('task'), { id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1', title: 'Task', status: 'active', sortKey: 'a', now: 1 })
  sessions = new SessionRepository(database, transactions)
})

afterEach(() => database.close())

describe('SessionRepository', () => {
  it('keeps logical Session identity across multiple process runs', () => {
    sessions.createSession(command('session'), {
      id: 'session-1', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'claude-code', title: 'Claude', now: 2
    })
    const first = sessions.startRun(command('run-1'), {
      id: 'run-1', sessionId: 'session-1', runtimeGeneration: 'generation-1',
      profile: 'claude-code', pid: 101, cols: 80, rows: 24, now: 3
    }).result
    sessions.finishRun(command('finish-1'), 'run-1', { exitCode: 1, now: 4 })
    const second = sessions.startRun(command('run-2'), {
      id: 'run-2', sessionId: 'session-1', runtimeGeneration: 'generation-2',
      profile: 'claude-code', pid: 202, cols: 100, rows: 30, now: 5
    }).result

    expect(first).toMatchObject({ ordinal: 1, pid: 101 })
    expect(second).toMatchObject({ ordinal: 2, pid: 202 })
    expect(sessions.getSession('session-1')).toMatchObject({
      id: 'session-1', status: 'running', version: 4
    })
    expect(sessions.listRuns('session-1').map(({ id }) => id)).toEqual(['run-1', 'run-2'])
  })

  it('uses only validated, non-invalidated provider bindings for resume', () => {
    seedSession()
    sessions.bindProvider(command('binding-old'), {
      id: 'binding-old', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-old', metadata: {}, now: 3
    })
    sessions.bindProvider(command('binding-new'), {
      id: 'binding-new', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-new', metadata: { cwd: '/tmp/workspace' }, now: 4
    })
    sessions.validateProviderBinding(command('validate-old'), 'binding-old', 5)
    sessions.validateProviderBinding(command('validate-new'), 'binding-new', 6)
    sessions.invalidateProviderBinding(command('invalidate-new'), 'binding-new', 'resume failed', 7)

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      id: 'binding-old', providerSessionId: 'provider-old', validatedAt: 5
    })
    expect(sessions.listProviderBindings('session-1')).toHaveLength(2)
  })

  it('isolates provider resume failure to its owning Session', () => {
    seedSession()
    sessions.createSession(command('session-2'), {
      id: 'session-2', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'codex', title: 'Codex', now: 2
    })
    sessions.bindProvider(command('binding-1'), {
      id: 'binding-1', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-1', metadata: {}, now: 3
    })
    sessions.bindProvider(command('binding-2'), {
      id: 'binding-2', sessionId: 'session-2', provider: 'codex',
      providerSessionId: 'provider-2', metadata: {}, now: 3
    })
    sessions.validateProviderBinding(command('validate-1'), 'binding-1', 4)
    sessions.validateProviderBinding(command('validate-2'), 'binding-2', 4)

    sessions.invalidateProviderBinding(command('fail-1'), 'binding-1', 'provider missing', 5)

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()
    expect(sessions.getResumeBinding('session-2', 'codex')).toMatchObject({ id: 'binding-2' })
  })
})

function seedSession(): void {
  sessions.createSession(command('session-1'), {
    id: 'session-1', taskId: 'task-1', executionContextId: 'context-1',
    kind: 'claude-code', title: 'Claude', now: 2
  })
}

function command(commandId: string) {
  return { commandId, commandType: 'session', requestHash: `hash-${commandId}` }
}
