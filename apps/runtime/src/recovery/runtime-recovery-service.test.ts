import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { SegmentJournal, readSessionFrames } from '../journal/segment-journal'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { RuntimeRecoveryService } from './runtime-recovery-service'

const databases: RuntimeDatabase[] = []
afterEach(() => {
  for (const database of databases.splice(0)) database.close()
})

describe('RuntimeRecoveryService', () => {
  it('repairs crash windows after restart and isolates one corrupt Session', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-runtime-recovery-'))
    const databasePath = join(root, 'matou.sqlite')
    const before = RuntimeDatabase.open(databasePath)
    databases.push(before)
    await new MigrationRunner(before, FOUNDATION_MIGRATIONS).migrate()
    seed(before, ['session-good', 'session-corrupt'])
    before.run(
      `INSERT INTO session_runs (
         id, session_id, ordinal, runtime_generation, profile, status,
         cols, rows, started_at
       ) VALUES (?, ?, 1, ?, 'shell', 'running', 80, 24, 2)`,
      'run-stale', 'session-good', before.runtimeGeneration
    )

    const good = await SegmentJournal.open(root, 'session-good')
    await good.appendOutput(1, Uint8Array.from([65]))
    await good.close()
    new DomainTransactionManager(before).execute(
      { commandId: 'event-without-marker', commandType: 'agent.todo', requestHash: 'hash' },
      ({ emit }) => {
        emit({
          eventId: 'todo-1', eventType: 'agent.todo', aggregateType: 'session',
          aggregateId: 'session-good', sessionId: 'session-good', taskId: 'task-1',
          payload: { text: 'recover me' }, requiredTerminalSequence: 1, occurredAt: 10
        })
        return null
      }
    )

    const corrupt = await SegmentJournal.open(root, 'session-corrupt')
    await corrupt.appendOutput(1, Uint8Array.from([66]))
    await corrupt.appendOutput(2, Uint8Array.from([67]))
    const corruptPath = corrupt.path
    await corrupt.close()
    const bytes = await readFile(corruptPath)
    bytes[20] = bytes[20]! ^ 0xff
    await writeFile(corruptPath, bytes)

    const priorGeneration = before.runtimeGeneration
    before.close()
    databases.splice(databases.indexOf(before), 1)
    const after = RuntimeDatabase.open(databasePath)
    databases.push(after)
    await new MigrationRunner(after, FOUNDATION_MIGRATIONS).migrate()

    const report = await new RuntimeRecoveryService(root, after).recoverAll()

    expect(after.runtimeGeneration).not.toBe(priorGeneration)
    expect(report.interruptedRuns).toEqual(['run-stale'])
    expect(after.get('SELECT status FROM session_runs WHERE id = ?', 'run-stale')).toEqual({
      status: 'interrupted'
    })
    expect(report.recovered).toEqual([
      expect.objectContaining({ sessionId: 'session-good', repairedAlignment: true })
    ])
    expect(report.failed).toEqual([
      expect.objectContaining({ sessionId: 'session-corrupt', code: 'JOURNAL_CORRUPT' })
    ])
    expect(await readSessionFrames(root, 'session-good')).toEqual([
      { kind: 'output', sequence: 1, data: Uint8Array.from([65]) },
      { kind: 'domain-cursor', sequence: 2, domainEventSequence: 2 }
    ])
  })
})

function seed(database: RuntimeDatabase, sessionIds: string[]): void {
  database.transaction((tx) => {
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
    for (const sessionId of sessionIds) {
      tx.run(
        'INSERT INTO sessions (id, task_id, execution_context_id, kind, status, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)',
        sessionId, 'task-1', 'context-1', 'shell', 'running', 1, 1, 1
      )
    }
  })
}
