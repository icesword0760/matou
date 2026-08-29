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
       state, created_at, started_at, attempt_count, updated_at
     ) VALUES ('child-1', 'source-1', 'claude-code', 'provider-source',
               'starting', 1, 2, 1, 2)`
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
      sourceProviderSessionId: 'provider-source'
    })
    expect(database.get(
      `SELECT state, started_at, attempt_count, updated_at, error_message
       FROM session_fork_intents WHERE session_id = 'child-1'`
    )).toEqual({
      state: 'starting', started_at: 10, attempt_count: 2,
      updated_at: 10, error_message: null
    })
  })
})
