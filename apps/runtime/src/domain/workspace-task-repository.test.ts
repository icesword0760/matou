import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { WorkspaceTaskRepository } from './workspace-task-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let repository: WorkspaceTaskRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-workspace-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  repository = new WorkspaceTaskRepository(database, new DomainTransactionManager(database))
})

afterEach(() => database.close())

describe('WorkspaceTaskRepository', () => {
  it('creates Workspace, plain execution context, and ordered Task through domain events', () => {
    repository.createWorkspace(command('cmd-w'), {
      id: 'workspace-1', name: 'Product', rootDirectory: '/tmp/product', now: 10
    })
    repository.createPlainExecutionContext(command('cmd-c'), {
      id: 'context-1', workspaceId: 'workspace-1', cwd: '/tmp/product', now: 11
    })
    const commit = repository.createTask(command('cmd-t'), {
      id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1',
      title: 'New item', status: 'active', sortKey: 'a0', now: 12
    })

    expect(commit.result).toMatchObject({ id: 'task-1', title: 'New item', version: 1 })
    expect(repository.getWorkspace('workspace-1')).toMatchObject({
      taskOrder: ['task-1'], pathIdentity: 'path:/tmp/product', version: 2
    })
    expect(database.all<{ event_type: string }>('SELECT event_type FROM domain_events ORDER BY seq')).toEqual([
      { event_type: 'workspace.created' },
      { event_type: 'execution-context.created' },
      { event_type: 'task.created' },
      { event_type: 'workspace.task-order-changed' }
    ])
  })

  it('enforces trimmed unique active Task names within a Workspace', () => {
    seedWorkspace(repository)
    repository.createTask(command('cmd-a'), {
      id: 'task-a', workspaceId: 'workspace-1', executionContextId: 'context-1',
      title: 'Item', status: 'planned', sortKey: 'a', now: 10
    })

    expect(() => repository.createTask(command('cmd-b'), {
      id: 'task-b', workspaceId: 'workspace-1', executionContextId: 'context-1',
      title: ' Item ', status: 'planned', sortKey: 'b', now: 11
    })).toThrow('an active Task named "Item" already exists in this Workspace')
  })

  it('requires parent Task and execution context to belong to the same Workspace', () => {
    seedWorkspace(repository)
    repository.createWorkspace(command('cmd-w2'), {
      id: 'workspace-2', name: 'Other', rootDirectory: '/tmp/other', now: 2
    })
    repository.createPlainExecutionContext(command('cmd-c2'), {
      id: 'context-2', workspaceId: 'workspace-2', cwd: '/tmp/other', now: 2
    })
    repository.createTask(command('cmd-parent'), {
      id: 'parent', workspaceId: 'workspace-1', executionContextId: 'context-1',
      title: 'Parent', status: 'active', sortKey: 'a', now: 3
    })

    expect(() => repository.createTask(command('cmd-child'), {
      id: 'child', workspaceId: 'workspace-2', parentTaskId: 'parent',
      executionContextId: 'context-2', title: 'Child', status: 'planned', sortKey: 'a', now: 4
    })).toThrow('parent Task must belong to the same Workspace')
  })

  it('archives a Task without deleting its Session graph or journal identity', () => {
    seedWorkspace(repository)
    repository.createTask(command('cmd-task'), {
      id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1',
      title: 'Task', status: 'active', sortKey: 'a', now: 3
    })
    database.run(
      'INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)',
      'session-1', 'task-1', 'context-1', 'shell', 'running', 'Shell', 4, 4, 4
    )

    repository.archiveTask(command('cmd-archive'), 'task-1', 20)

    expect(repository.getTask('task-1')).toMatchObject({ status: 'archived', archivedAt: 20 })
    expect(database.get('SELECT id FROM sessions WHERE id = ?', 'session-1')).toEqual({ id: 'session-1' })
  })

  it('keeps a Workspace usable when only its stored Task order is malformed', () => {
    seedWorkspace(repository)
    database.run(
      'UPDATE workspaces SET task_order_json = ? WHERE id = ?',
      '{broken-json', 'workspace-1'
    )

    expect(repository.getWorkspace('workspace-1')).toMatchObject({
      id: 'workspace-1', taskOrder: []
    })
    expect(() => repository.createTask(command('task-after-repair'), {
      id: 'task-after-repair', workspaceId: 'workspace-1', executionContextId: 'context-1',
      title: 'Still usable', status: 'active', sortKey: 'a', now: 3
    })).not.toThrow()
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'test', requestHash: `hash-${commandId}` }
}

function seedWorkspace(target: WorkspaceTaskRepository): void {
  target.createWorkspace(command('cmd-workspace'), {
    id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1
  })
  target.createPlainExecutionContext(command('cmd-context'), {
    id: 'context-1', workspaceId: 'workspace-1', cwd: '/tmp/workspace', now: 2
  })
}
