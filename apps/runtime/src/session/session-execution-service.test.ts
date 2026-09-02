import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { SessionRepository } from '../domain/session-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { RuntimeSessionRegistry } from './runtime-session-registry'
import {
  SessionExecutionService,
  type SessionExecutionBackend,
  type SessionExecutionDescriptor
} from './session-execution-service'
import { SessionForkIntentRepository } from './session-fork-intent-repository'

let database: RuntimeDatabase
let registry: RuntimeSessionRegistry
let intents: SessionForkIntentRepository

const descriptor: SessionExecutionDescriptor = {
  sessionId: 'child-1', executionContextId: 'context-1', profile: 'claude-code',
  cols: 80, rows: 24
}

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-session-execution-'))
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
  registry = new RuntimeSessionRegistry()
  intents = new SessionForkIntentRepository(database)
})

afterEach(() => database.close())

describe('SessionExecutionService', () => {
  it('defers a Renderer launch while a durable Fork has no live process', async () => {
    const lease = durableForkAtProviderStage()
    const backend = backendFixture()
    const service = new SessionExecutionService(database, registry, backend)

    await expect(service.startOrResume('child-1', descriptor)).resolves.toEqual({
      kind: 'deferred', operationId: 'operation-1', stage: 'restoring-provider'
    })
    expect(backend.startOrResume).not.toHaveBeenCalled()
    expect(lease.token).not.toBe('')
  })

  it('starts the durable Fork only for its current lease and binds a run identity', async () => {
    const lease = durableForkAtProviderStage()
    const backend = backendFixture()
    const service = new SessionExecutionService(database, registry, backend, { now: () => 12 })

    await expect(service.startOrResume('child-1', descriptor, {
      operationId: 'operation-1', runId: 'run-authoritative', lease
    })).resolves.toEqual({
      kind: 'started', value: 'launched',
      authority: {
        operationId: 'operation-1', runId: 'run-authoritative', lease,
        sourceSessionId: 'source-1', sourceProviderSessionId: 'provider-source'
      }
    })
    expect(backend.startOrResume).toHaveBeenCalledWith(descriptor, {
      operationId: 'operation-1', runId: 'run-authoritative', lease,
      sourceSessionId: 'source-1', sourceProviderSessionId: 'provider-source'
    })
  })

  it('rejects an expired or replaced Fork authority before launching a process', async () => {
    const first = durableForkAtProviderStage(5)
    const second = intents.acquireLease({
      operationId: 'operation-1', owner: 'runtime-b', now: 20, ttlMs: 20
    })
    if (second.kind !== 'acquired') throw new Error('takeover lease missing')
    const backend = backendFixture()
    const service = new SessionExecutionService(database, registry, backend, { now: () => 21 })

    await expect(service.startOrResume('child-1', descriptor, {
      operationId: 'operation-1', runId: 'run-stale', lease: first
    })).resolves.toEqual({ kind: 'stale-authority', operationId: 'operation-1' })
    expect(backend.startOrResume).not.toHaveBeenCalled()
  })
})

function durableForkAtProviderStage(ttlMs = 20) {
  intents.accept({
    operationId: 'operation-1', submissionKey: 'submission-1', sessionId: 'child-1',
    sourceSessionId: 'source-1', sourceProviderSessionId: 'provider-source',
    displayName: 'Child', worktreeMode: 'current', totalSteps: 2, now: 2
  })
  const decision = intents.acquireLease({
    operationId: 'operation-1', owner: 'runtime-a', now: 10, ttlMs
  })
  if (decision.kind !== 'acquired') throw new Error('lease missing')
  const advanced = intents.advanceStage({
    operationId: 'operation-1', lease: decision.lease,
    stage: 'restoring-provider', now: 11
  })
  if (advanced.kind !== 'applied') throw new Error('provider stage missing')
  return decision.lease
}

function backendFixture(): SessionExecutionBackend<string> {
  return { startOrResume: vi.fn(async () => 'launched') }
}

function command(commandId: string) {
  return { commandId, commandType: 'session-execution', requestHash: `hash-${commandId}` }
}
