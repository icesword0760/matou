import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { SegmentJournal } from '../journal/segment-journal'
import { JournalEventCoordinator } from './journal-event-alignment'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let root: string
let database: RuntimeDatabase
let journal: SegmentJournal
let coordinator: JournalEventCoordinator

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-alignment-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  seedSession(database)
  journal = await SegmentJournal.open(root, 'session-1')
  coordinator = new JournalEventCoordinator(database, new DomainTransactionManager(database))
})

afterEach(async () => {
  await journal.close()
  database.close()
})

describe('JournalEventCoordinator', () => {
  it('flushes a domain cursor after the domain transaction commits', async () => {
    await journal.appendOutput(1, Uint8Array.from([65]))

    const commit = await coordinator.execute(
      'session-1',
      journal,
      { commandId: 'cmd-1', commandType: 'todo.create', requestHash: 'hash-1' },
      ({ emit }) => {
        emit({
          eventId: 'event-1',
          eventType: 'agent.todo',
          aggregateType: 'session',
          aggregateId: 'session-1',
          sessionId: 'session-1',
          taskId: 'task-1',
          workspaceId: 'workspace-1',
          payload: { text: 'ship' },
          requiredTerminalSequence: 1,
          occurredAt: 100
        })
        return 'ok'
      }
    )

    expect(commit.lastEventSequence).toBe(1)
    expect(await journal.readFrames()).toEqual([
      { kind: 'output', sequence: 1, data: Uint8Array.from([65]) },
      { kind: 'domain-cursor', sequence: 2, domainEventSequence: 1 }
    ])
    expect(await coordinator.recover('session-1', journal)).toEqual({
      terminalSequence: 2,
      domainEventSequence: 1,
      repaired: false,
      pendingDomainEventSequence: undefined
    })
  })

  it('repairs the crash window after SQLite commit and before marker append', async () => {
    await journal.appendOutput(1, Uint8Array.from([65]))
    await expect(
      coordinator.execute(
        'session-1',
        journal,
        { commandId: 'cmd-2', commandType: 'todo.create', requestHash: 'hash-2' },
        ({ emit }) => {
          emit({
            eventId: 'event-2',
            eventType: 'agent.todo',
            aggregateType: 'session',
            aggregateId: 'session-1',
            sessionId: 'session-1',
            payload: {},
            requiredTerminalSequence: 1,
            occurredAt: 100
          })
          return null
        },
        (phase) => {
          if (phase === 'after-domain-commit') throw new Error('simulated crash')
        }
      )
    ).rejects.toThrow('simulated crash')

    expect(database.get('SELECT event_id FROM domain_events')).toEqual({ event_id: 'event-2' })
    expect((await journal.readFrames()).some(({ kind }) => kind === 'domain-cursor')).toBe(false)
    expect(await coordinator.recover('session-1', journal)).toEqual({
      terminalSequence: 2,
      domainEventSequence: 1,
      repaired: true,
      pendingDomainEventSequence: undefined
    })
  })

  it('keeps an event pending when its required terminal bytes are missing', async () => {
    await journal.appendOutput(1, Uint8Array.from([65]))
    new DomainTransactionManager(database).execute(
      { commandId: 'cmd-3', commandType: 'todo.create', requestHash: 'hash-3' },
      ({ emit }) => {
        emit({
          eventId: 'event-3',
          eventType: 'agent.todo',
          aggregateType: 'session',
          aggregateId: 'session-1',
          sessionId: 'session-1',
          payload: {},
          requiredTerminalSequence: 5,
          occurredAt: 100
        })
        return null
      }
    )

    expect(await coordinator.recover('session-1', journal)).toEqual({
      terminalSequence: 1,
      domainEventSequence: 0,
      repaired: false,
      pendingDomainEventSequence: 1
    })
  })

  it('detects a journal cursor that points beyond committed domain state', async () => {
    await journal.appendDomainCursor(1, 99)

    await expect(coordinator.recover('session-1', journal)).rejects.toThrow(
      'journal domain cursor 99 is ahead of committed sequence 0'
    )
  })
})

function seedSession(db: RuntimeDatabase): void {
  db.transaction((tx) => {
    tx.run(
      'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
      'workspace-1', 'Workspace', '/tmp/workspace', 1, 1
    )
    tx.run(
      'INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)',
      'context-1', 'workspace-1', 'plain-directory', '/tmp/workspace', 1
    )
    tx.run(
      'INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
      'task-1', 'workspace-1', 'context-1', 'Task', 'active', 1, 1
    )
    tx.run(
      'INSERT INTO sessions (id, task_id, execution_context_id, kind, status, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)',
      'session-1', 'task-1', 'context-1', 'shell', 'running', 1, 1, 1
    )
  })
}
