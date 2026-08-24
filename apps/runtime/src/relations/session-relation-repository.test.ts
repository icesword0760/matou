import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { SessionRepository } from '../domain/session-repository'
import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { SessionRelationRepository } from './session-relation-repository'

let database: RuntimeDatabase
let relations: SessionRelationRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-relations-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const workspaces = new WorkspaceTaskRepository(database, transactions)
  workspaces.createWorkspace(command('workspace'), { id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1 })
  workspaces.createPlainExecutionContext(command('context'), { id: 'context-1', workspaceId: 'workspace-1', cwd: '/tmp/workspace', now: 1 })
  workspaces.createTask(command('task'), { id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1', title: 'Task', status: 'active', sortKey: 'a', now: 1 })
  const sessions = new SessionRepository(database, transactions)
  for (const id of ['parent', 'child-a', 'child-b', 'other', 'node-0', 'node-1', 'node-2', 'node-3', 'node-4', 'node-5']) {
    sessions.createSession(command(`session-${id}`), {
      id, taskId: 'task-1', executionContextId: 'context-1', kind: 'shell', title: id, now: 2
    })
  }
  relations = new SessionRelationRepository(database, transactions)
})

afterEach(() => database.close())

describe('SessionRelationRepository', () => {
  it('appends relation history and updates the current projection in the same transaction', () => {
    const relation = relations.create(command('fork-a'), {
      id: 'relation-a', taskId: 'task-1', fromSessionId: 'child-a', toSessionId: 'parent',
      kind: 'forked-from', metadata: { prompt: 'branch A' }, now: 10
    }).result

    expect(relation).toMatchObject({ id: 'relation-a', kind: 'forked-from' })
    expect(database.get(
      'SELECT operation, relation_kind FROM session_relation_events WHERE relation_id = ?', 'relation-a'
    )).toEqual({ operation: 'created', relation_kind: 'forked-from' })
    expect(database.get(
      'SELECT relation_kind, source_event_sequence FROM session_relations_current WHERE relation_id = ?', 'relation-a'
    )).toEqual({ relation_kind: 'forked-from', source_event_sequence: 1 })
    expect(database.get(
      "SELECT event_type FROM domain_events WHERE event_type = 'session-relation.created'"
    )).toEqual({ event_type: 'session-relation.created' })
  })

  it('derives siblings from their active common fork parent without storing sibling edges', () => {
    relations.create(command('fork-a'), {
      id: 'relation-a', taskId: 'task-1', fromSessionId: 'child-a', toSessionId: 'parent',
      kind: 'forked-from', metadata: {}, now: 10
    })
    relations.create(command('fork-b'), {
      id: 'relation-b', taskId: 'task-1', fromSessionId: 'child-b', toSessionId: 'parent',
      kind: 'forked-from', metadata: {}, now: 11
    })

    expect(relations.deriveSiblings('child-a').map(({ id }) => id)).toEqual(['child-b'])
    expect(database.get(
      "SELECT relation_id FROM session_relations_current WHERE relation_kind = 'sibling'"
    )).toBeUndefined()
  })

  it('enforces one active direct fork parent', () => {
    relations.create(command('fork-a'), {
      id: 'relation-a', taskId: 'task-1', fromSessionId: 'child-a', toSessionId: 'parent',
      kind: 'forked-from', metadata: {}, now: 10
    })

    expect(() => relations.create(command('fork-again'), {
      id: 'relation-again', taskId: 'task-1', fromSessionId: 'child-a', toSessionId: 'other',
      kind: 'forked-from', metadata: {}, now: 11
    })).toThrow('Session child-a already has an active fork parent')
  })

  it.each(['forked-from', 'depends-on'] as const)('rejects a %s cycle', (kind) => {
    relations.create(command(`${kind}-1`), {
      id: `${kind}-1`, taskId: 'task-1', fromSessionId: 'child-a', toSessionId: 'parent',
      kind, metadata: {}, now: 10
    })

    expect(() => relations.create(command(`${kind}-2`), {
      id: `${kind}-2`, taskId: 'task-1', fromSessionId: 'parent', toSessionId: 'child-a',
      kind, metadata: {}, now: 11
    })).toThrow(`creating ${kind} would introduce a cycle`)
  })

  it.each(['forked-from', 'depends-on'] as const)(
    'preserves the acyclic property for every back-edge across a deep %s chain',
    (kind) => {
      for (let index = 1; index < 6; index += 1) {
        relations.create(command(`${kind}-chain-${index}`), {
          id: `${kind}-chain-${index}`, taskId: 'task-1',
          fromSessionId: `node-${index}`, toSessionId: `node-${index - 1}`,
          kind, metadata: {}, now: 20 + index
        })
      }
      for (let index = 1; index < 6; index += 1) {
        expect(() => relations.create(command(`${kind}-back-${index}`), {
          id: `${kind}-back-${index}`, taskId: 'task-1',
          fromSessionId: 'node-0', toSessionId: `node-${index}`,
          kind, metadata: {}, now: 40 + index
        })).toThrow(`creating ${kind} would introduce a cycle`)
      }
      expect(database.get<{ count: number }>(
        'SELECT COUNT(*) AS count FROM session_relations_current WHERE relation_kind = ?', kind
      )?.count).toBe(5)
    }
  )

  it('revokes and restores through append-only facts while keeping current queries synchronous', () => {
    relations.create(command('supports'), {
      id: 'relation-s', taskId: 'task-1', fromSessionId: 'child-a', toSessionId: 'other',
      kind: 'supports', metadata: { reason: 'research' }, now: 10
    })
    relations.revoke(command('revoke'), 'relation-s', 11)
    expect(relations.getCurrent('relation-s')).toBeUndefined()
    relations.restore(command('restore'), 'relation-s', 12)

    expect(relations.getCurrent('relation-s')).toMatchObject({ kind: 'supports' })
    expect(relations.history('relation-s').map(({ operation }) => operation)).toEqual([
      'created', 'revoked', 'restored'
    ])
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'relation', requestHash: `hash-${commandId}` }
}
