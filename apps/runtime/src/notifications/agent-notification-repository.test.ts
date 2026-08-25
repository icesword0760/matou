import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { SessionRepository } from '../domain/session-repository'
import { DomainEventStore } from '../events/domain-event-store'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { AgentNotificationRepository } from './agent-notification-repository'

let database: RuntimeDatabase
let repository: AgentNotificationRepository
let events: DomainEventStore

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-agent-notification-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const hierarchy = new WorkspaceTaskRepository(database, transactions)
  hierarchy.createWorkspace(command('workspace'), { id: 'workspace-1', name: 'Workspace', rootDirectory: root, now: 1 })
  hierarchy.createPlainExecutionContext(command('context'), { id: 'context-1', workspaceId: 'workspace-1', cwd: root, now: 1 })
  hierarchy.createTask(command('task'), {
    id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1',
    title: 'Task', status: 'active', sortKey: 'a', now: 1
  })
  new SessionRepository(database, transactions).createSession(command('session'), {
    id: 'session-1', taskId: 'task-1', executionContextId: 'context-1', kind: 'claude-code', title: 'Claude', now: 2
  })
  repository = new AgentNotificationRepository(database, transactions)
  events = new DomainEventStore(database)
})

afterEach(() => database.close())

describe('AgentNotificationRepository', () => {
  it('emits a hierarchy-addressable semantic event without creating notification persistence', () => {
    repository.publish(command('notification'), {
      eventId: 'provider-event-1', runId: 'run-1', sessionId: 'session-1', provider: 'claude-code',
      event: {
        eventType: 'completed', title: 'Claude Code', subtitle: 'Completed', body: '任务完成', sound: true,
        cooldownKey: 'Stop'
      },
      now: 10
    })

    expect(events.readAfter(0, 20).at(-1)).toMatchObject({
      eventId: 'provider-event-1', eventType: 'agent.notification', aggregateType: 'session',
      aggregateId: 'session-1', workspaceId: 'workspace-1', taskId: 'task-1', sessionId: 'session-1',
      payload: {
        runId: 'run-1', provider: 'claude-code',
        event: { eventType: 'completed', title: 'Claude Code', body: '任务完成' }
      },
      occurredAt: 10
    })
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM annotations')?.count).toBe(0)
  })

  it('keeps a damaged Session event visible with the known Session identity when hierarchy lookup fails', () => {
    repository.publish(command('missing'), {
      eventId: 'provider-event-missing', runId: 'run-missing', sessionId: 'missing-session', provider: 'claude-code',
      event: { eventType: 'attention', title: 'Claude Code', subtitle: 'Attention', body: '需要处理', sound: true, cooldownKey: 'Notification' },
      now: 11
    })

    expect(events.readAfter(0, 20).at(-1)).toMatchObject({
      eventType: 'agent.notification', aggregateId: 'missing-session',
      payload: { targetSessionId: 'missing-session' }
    })
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'agent.notification.publish', requestHash: `hash-${commandId}` }
}
