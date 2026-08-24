import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ProductFoundationRepository, TaskTelemetryRepository } from './product-foundation-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let foundation: ProductFoundationRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-foundation-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  seed(database)
  foundation = new ProductFoundationRepository(database, new DomainTransactionManager(database))
})

afterEach(() => database.close())

describe('ProductFoundationRepository', () => {
  it('persists annotations with semantic anchors and degradation state', () => {
    foundation.createAnnotation(command('annotation'), {
      id: 'annotation-1', taskId: 'task-1', sessionId: 'session-1', kind: 'todo',
      textSnapshot: 'Run tests',
      anchor: { kind: 'semantic-event', sessionId: 'session-1', eventId: 'event-provider-1' },
      now: 2
    })
    const degraded = foundation.updateAnnotationStatus(
      command('annotation-degraded'), 'annotation-1', 'degraded', 3
    ).result

    expect(degraded).toMatchObject({
      id: 'annotation-1', status: 'degraded', textSnapshot: 'Run tests',
      anchor: { eventId: 'event-provider-1' }
    })
  })

  it('upserts artifacts by Task path identity and records Validation lifecycle events', () => {
    foundation.observeArtifact(command('artifact-1'), {
      id: 'artifact-1', taskId: 'task-1', producerSessionId: 'session-1',
      pathIdentity: 'path:dist/app.js', mediaType: 'text/javascript', state: 'observed',
      metadata: { size: 1 }, now: 2
    })
    const artifact = foundation.observeArtifact(command('artifact-2'), {
      id: 'ignored-new-id', taskId: 'task-1', producerSessionId: 'session-1',
      pathIdentity: 'path:dist/app.js', mediaType: 'text/javascript', state: 'produced',
      metadata: { size: 2 }, now: 3
    }).result
    foundation.createValidation(command('validation'), {
      id: 'validation-1', taskId: 'task-1', sessionId: 'session-1',
      checkId: 'unit-tests', status: 'running', now: 3
    })
    const validation = foundation.updateValidation(
      command('validation-pass'), 'validation-1', 'passed', { passed: 85 }, 4
    ).result

    expect(artifact).toMatchObject({ id: 'artifact-1', state: 'produced', metadata: { size: 2 } })
    expect(validation).toMatchObject({ id: 'validation-1', status: 'passed', endedAt: 4 })
  })
})

describe('TaskTelemetryRepository', () => {
  it('keeps current-generation status/progress/log history and emits subscriptions', () => {
    const telemetry = new TaskTelemetryRepository(database, 'generation-1', { maxLogsPerTask: 2 })
    const listener = vi.fn()
    telemetry.subscribe(listener)
    telemetry.setStatus('task-1', 'phase', 'building', 2)
    telemetry.setProgress('task-1', 50, 'half', 3)
    telemetry.appendLog('task-1', 'info', 'agent', 'one', 4)
    telemetry.appendLog('task-1', 'warn', 'agent', 'two', 5)
    telemetry.appendLog('task-1', 'error', 'agent', 'three', 6)

    expect(telemetry.snapshot('task-1')).toMatchObject({
      status: { phase: 'building' }, progress: { progress: 50, label: 'half' }
    })
    expect(telemetry.snapshot('task-1').logs.map(({ message }) => message)).toEqual(['two', 'three'])
    expect(listener).toHaveBeenCalledTimes(5)
  })

  it('purges telemetry from previous Runtime generations', () => {
    const first = new TaskTelemetryRepository(database, 'generation-old')
    first.setStatus('task-1', 'phase', 'old', 2)
    first.appendLog('task-1', 'info', 'agent', 'old', 2)

    const next = new TaskTelemetryRepository(database, 'generation-new')
    expect(next.purgeStaleGenerations()).toBe(2)
    expect(next.snapshot('task-1')).toEqual({ status: {}, progress: undefined, logs: [] })
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'foundation', requestHash: `hash-${commandId}` }
}

function seed(db: RuntimeDatabase): void {
  db.transaction((tx) => {
    tx.run('INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)', 'workspace-1', 'Workspace', '/tmp/workspace', 1, 1)
    tx.run('INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)', 'context-1', 'workspace-1', 'plain-directory', '/tmp/workspace', 1)
    tx.run('INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, sort_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)', 'task-1', 'workspace-1', 'context-1', 'Task', 'active', 'a', 1, 1)
    tx.run('INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)', 'session-1', 'task-1', 'context-1', 'shell', 'running', 'Shell', 1, 1, 1)
  })
}
