import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { DomainEventStore } from './domain-event-store'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let store: DomainEventStore
let transactions: DomainTransactionManager

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-outbox-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  store = new DomainEventStore(database)
  transactions = new DomainTransactionManager(database)
})

afterEach(() => database.close())

describe('DomainEventStore', () => {
  it('replays ordered pages after a durable cursor', () => {
    appendEvents(3)

    expect(store.readAfter(0, 2).map(({ sequence }) => sequence)).toEqual([1, 2])
    store.acknowledge('renderer-1', 2, 100)
    expect(store.cursor('renderer-1')).toBe(2)
    expect(store.readForConsumer('renderer-1', 10).map(({ sequence }) => sequence)).toEqual([3])
  })

  it('keeps independent monotonic cursors per consumer', () => {
    appendEvents(2)
    store.acknowledge('renderer-a', 2, 100)
    store.acknowledge('plugin-b', 1, 100)
    store.acknowledge('renderer-a', 1, 200)

    expect(store.cursor('renderer-a')).toBe(2)
    expect(store.cursor('plugin-b')).toBe(1)
    expect(store.lag('renderer-a')).toBe(0)
    expect(store.lag('plugin-b')).toBe(1)
  })

  it('decodes the complete stable event envelope', () => {
    appendEvents(1)

    expect(store.readAfter(0, 1)[0]).toEqual({
      sequence: 1,
      eventId: 'event-1',
      eventType: 'test.happened',
      aggregateType: 'test',
      aggregateId: 'aggregate-1',
      workspaceId: undefined,
      taskId: undefined,
      sessionId: undefined,
      payload: { ordinal: 1 },
      schemaVersion: 1,
      requiredTerminalSequence: undefined,
      commandId: 'cmd-1',
      causationId: undefined,
      correlationId: undefined,
      occurredAt: 101
    })
  })

  function appendEvents(count: number): void {
    for (let ordinal = 1; ordinal <= count; ordinal += 1) {
      transactions.execute(
        {
          commandId: `cmd-${ordinal}`,
          commandType: 'test',
          requestHash: `hash-${ordinal}`
        },
        ({ emit }) => {
          emit({
            eventId: `event-${ordinal}`,
            eventType: 'test.happened',
            aggregateType: 'test',
            aggregateId: `aggregate-${ordinal}`,
            payload: { ordinal },
            occurredAt: 100 + ordinal
          })
          return null
        }
      )
    }
  }
})
