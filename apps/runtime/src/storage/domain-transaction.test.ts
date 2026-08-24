import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from './database'
import { DomainTransactionManager } from './domain-transaction'
import { MigrationRunner } from './migration-runner'
import { FOUNDATION_MIGRATIONS } from './migrations'

let database: RuntimeDatabase
let transactions: DomainTransactionManager

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-domain-tx-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  transactions = new DomainTransactionManager(database)
})

afterEach(() => database.close())

describe('DomainTransactionManager', () => {
  it('commits a mutation, its events, and command record atomically', () => {
    const commit = transactions.execute(
      { commandId: 'cmd-1', commandType: 'workspace.create', requestHash: 'hash-1' },
      ({ tx, emit }) => {
        tx.run(
          'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
          'workspace-1',
          'Workspace',
          '/tmp/workspace',
          100,
          100
        )
        emit({
          eventId: 'event-1',
          eventType: 'workspace.created',
          aggregateType: 'workspace',
          aggregateId: 'workspace-1',
          workspaceId: 'workspace-1',
          payload: { name: 'Workspace' },
          occurredAt: 100
        })
        return { workspaceId: 'workspace-1' }
      }
    )

    expect(commit).toEqual({
      result: { workspaceId: 'workspace-1' },
      firstEventSequence: 1,
      lastEventSequence: 1,
      replayed: false
    })
    expect(database.get('SELECT id FROM workspaces')).toEqual({ id: 'workspace-1' })
    expect(database.get('SELECT event_id, command_id FROM domain_events')).toEqual({
      event_id: 'event-1',
      command_id: 'cmd-1'
    })
    expect(database.get('SELECT command_id FROM command_deduplication')).toEqual({
      command_id: 'cmd-1'
    })
  })

  it('rolls back the mutation when event insertion fails', () => {
    expect(() =>
      transactions.execute(
        { commandId: 'cmd-2', commandType: 'workspace.create', requestHash: 'hash-2' },
        ({ tx, emit }) => {
          tx.run(
            'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
            'workspace-2',
            'Workspace',
            '/tmp/workspace',
            100,
            100
          )
          emit({
            eventId: 'event-2',
            eventType: 'workspace.created',
            aggregateType: 'workspace',
            aggregateId: 'workspace-2',
            workspaceId: 'missing-workspace',
            payload: {},
            occurredAt: 100
          })
          return { workspaceId: 'workspace-2' }
        }
      )
    ).toThrow()

    expect(database.get("SELECT id FROM workspaces WHERE id = 'workspace-2'")).toBeUndefined()
    expect(database.get("SELECT event_id FROM domain_events WHERE event_id = 'event-2'")).toBeUndefined()
    expect(
      database.get("SELECT command_id FROM command_deduplication WHERE command_id = 'cmd-2'")
    ).toBeUndefined()
  })

  it('replays the recorded response without running a duplicate mutation', () => {
    let calls = 0
    const command = { commandId: 'cmd-3', commandType: 'noop', requestHash: 'hash-3' }
    const invoke = () =>
      transactions.execute(command, ({ emit }) => {
        calls += 1
        emit({
          eventId: 'event-3',
          eventType: 'test.happened',
          aggregateType: 'test',
          aggregateId: 'test-1',
          payload: { call: calls },
          occurredAt: 100
        })
        return { call: calls }
      })

    expect(invoke().replayed).toBe(false)
    expect(invoke()).toEqual({
      result: { call: 1 },
      firstEventSequence: 1,
      lastEventSequence: 1,
      replayed: true
    })
    expect(calls).toBe(1)
  })

  it('rejects reuse of a command id with a different request', () => {
    transactions.execute(
      { commandId: 'cmd-4', commandType: 'noop', requestHash: 'hash-a' },
      () => 'first'
    )

    expect(() =>
      transactions.execute(
        { commandId: 'cmd-4', commandType: 'noop', requestHash: 'hash-b' },
        () => 'second'
      )
    ).toThrow('command id cmd-4 was already used for a different request')
  })

  it('propagates terminal and causal alignment metadata', () => {
    transactions.execute(
      {
        commandId: 'cmd-5',
        commandType: 'annotation.create',
        requestHash: 'hash-5',
        causationId: 'provider-event-1',
        correlationId: 'workflow-1'
      },
      ({ emit }) => {
        emit({
          eventId: 'event-5',
          eventType: 'annotation.created',
          aggregateType: 'annotation',
          aggregateId: 'annotation-1',
          requiredTerminalSequence: 42,
          payload: { text: 'todo' },
          occurredAt: 200
        })
        return null
      }
    )

    expect(
      database.get(
        'SELECT required_terminal_sequence, causation_id, correlation_id FROM domain_events'
      )
    ).toEqual({
      required_terminal_sequence: 42,
      causation_id: 'provider-event-1',
      correlation_id: 'workflow-1'
    })
  })
})
