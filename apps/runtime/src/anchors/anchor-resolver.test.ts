import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import type { CommandOutputAnchor, ScreenCaptureAnchor, SemanticAnchor } from '@matou/domain'

import { AnchorResolver, CommandBoundaryRepository, Osc133Tracker } from './anchor-resolver'
import { SegmentJournal } from '../journal/segment-journal'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let root: string
let database: RuntimeDatabase
let journal: SegmentJournal
let boundaries: CommandBoundaryRepository
let resolver: AnchorResolver

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-anchor-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  seedSession(database)
  journal = await SegmentJournal.open(root, 'session-1')
  boundaries = new CommandBoundaryRepository(database)
  resolver = new AnchorResolver(database, async () => journal.readFrames(), boundaries)
})

afterEach(async () => {
  await journal.close()
  database.close()
})

describe('AnchorResolver', () => {
  it('resolves semantic anchors by stable event id instead of repeated text', () => {
    new DomainTransactionManager(database).execute(
      { commandId: 'cmd-domain', commandType: 'agent', requestHash: 'hash-domain' },
      ({ emit }) => {
        emit({
          eventId: 'semantic-1', eventType: 'agent.todo', aggregateType: 'session',
          aggregateId: 'session-1', sessionId: 'session-1', payload: { text: '✓ Done' }, occurredAt: 10
        })
        return null
      }
    )
    const anchor: SemanticAnchor = {
      kind: 'semantic-event', sessionId: 'session-1', eventId: 'semantic-1',
      sourceRef: { provider: 'claude-code', providerEventId: 'tool-use-1' }
    }

    expect(resolver.resolveSemantic(anchor)).toEqual({
      status: 'resolved', eventSequence: 1, payload: { text: '✓ Done' }
    })
  })

  it('uses OSC 133 command identity to distinguish identical output', async () => {
    await journal.appendOutput(1, new TextEncoder().encode('same\n'))
    await journal.appendResize(2, 80, 24)
    await journal.appendOutput(3, new TextEncoder().encode('same\n'))
    await journal.appendResize(4, 80, 24)
    boundaries.start({ commandId: 'command-1', sessionId: 'session-1', sequence: 1, commandText: 'first', now: 1 })
    boundaries.finish('command-1', 2, 0, 2)
    boundaries.start({ commandId: 'command-2', sessionId: 'session-1', sequence: 3, commandText: 'second', now: 3 })
    boundaries.finish('command-2', 4, 0, 4)
    const anchor: CommandOutputAnchor = {
      kind: 'command-output', sessionId: 'session-1', commandId: 'command-2',
      startSequence: 3, endSequence: 4
    }

    await expect(resolver.resolveCommandOutput(anchor)).resolves.toEqual({
      status: 'resolved', text: 'same\n', startSequence: 3, endSequence: 4
    })
  })

  it('degrades retained command anchors when their journal prefix is gone', async () => {
    await journal.appendOutput(5, new TextEncoder().encode('tail'))
    boundaries.start({ commandId: 'command-old', sessionId: 'session-1', sequence: 1, now: 1 })
    boundaries.finish('command-old', 5, 0, 5)
    const anchor: CommandOutputAnchor = {
      kind: 'command-output', sessionId: 'session-1', commandId: 'command-old',
      startSequence: 1, endSequence: 5
    }

    await expect(resolver.resolveCommandOutput(anchor)).resolves.toMatchObject({
      status: 'degraded', reason: 'journal-retention', text: 'tail'
    })
  })

  it('uses screen epochs to reject an alternate-screen mismatch while retaining captured text', async () => {
    await journal.appendOutput(1, new TextEncoder().encode('before'))
    await journal.appendReset(2, 1)
    await journal.appendOutput(3, new TextEncoder().encode('after'))
    const anchor: ScreenCaptureAnchor = {
      kind: 'screen-capture', sessionId: 'session-1', screenEpoch: 0, sequence: 3,
      geometry: { cols: 80, rows: 24 }, range: { startX: 0, startY: 0, endX: 5, endY: 0 },
      capturedText: 'before'
    }

    await expect(resolver.resolveScreenCapture(anchor)).resolves.toEqual({
      status: 'degraded', reason: 'screen-epoch-mismatch', text: 'before'
    })
  })
})

describe('Osc133Tracker', () => {
  it('turns shell integration markers into a stable command boundary', () => {
    const tracker = new Osc133Tracker('session-1', boundaries)
    tracker.ingest('\u001b]133;B\u0007', 10, 10)
    tracker.ingest('\u001b]133;C\u0007', 11, 11)
    tracker.ingest('\u001b]133;D;0\u0007', 12, 12)

    expect(boundaries.list('session-1')[0]).toMatchObject({
      startedSequence: 10, executedSequence: 11, endedSequence: 12, exitCode: 0
    })
  })
})

function seedSession(db: RuntimeDatabase): void {
  db.transaction((tx) => {
    tx.run('INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)', 'workspace-1', 'Workspace', '/tmp/workspace', 1, 1)
    tx.run('INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)', 'context-1', 'workspace-1', 'plain-directory', '/tmp/workspace', 1)
    tx.run('INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, sort_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)', 'task-1', 'workspace-1', 'context-1', 'Task', 'active', 'a', 1, 1)
    tx.run('INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)', 'session-1', 'task-1', 'context-1', 'shell', 'running', 'Shell', 1, 1, 1)
  })
}
