import { execFile } from 'node:child_process'
import { mkdtemp, realpath, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { WorktreeService } from './worktree-service'

const exec = promisify(execFile)
let root: string
let repositoryRoot: string
let database: RuntimeDatabase
let service: WorktreeService
let stopRuns: ReturnType<typeof vi.fn<(runIds: string[]) => Promise<void>>>

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-worktree-'))
  repositoryRoot = join(root, 'repository')
  await exec('git', ['init', repositoryRoot])
  await exec('git', ['-C', repositoryRoot, 'config', 'user.email', 'matou@example.test'])
  await exec('git', ['-C', repositoryRoot, 'config', 'user.name', 'Matou Test'])
  await writeFile(join(repositoryRoot, 'README.md'), 'root\n')
  await exec('git', ['-C', repositoryRoot, 'add', 'README.md'])
  await exec('git', ['-C', repositoryRoot, 'commit', '-m', 'initial'])

  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const workspaces = new WorkspaceTaskRepository(database, transactions)
  workspaces.createWorkspace(command('workspace'), {
    id: 'workspace-1', name: 'Workspace', rootDirectory: repositoryRoot, now: 1
  })
  stopRuns = vi.fn<(runIds: string[]) => Promise<void>>(async () => undefined)
  service = new WorktreeService(database, transactions, { stopRuns })
})

afterEach(() => database.close())

describe('WorktreeService', () => {
  it('creates an isolated git worktree and persists its audited setup result', async () => {
    const path = join(root, 'worktrees', 'branch-a')
    const worktree = await service.create(command('create'), {
      id: 'worktree-1', executionContextId: 'context-worktree-1', workspaceId: 'workspace-1',
      repositoryRoot, path, branch: 'branch-a', baseRef: 'HEAD', setupPolicy: [], now: 10
    })

    expect(worktree).toMatchObject({
      id: 'worktree-1', path, branch: 'branch-a', state: 'ready', cleanupPolicy: 'retain-dirty'
    })
    await expect(exec('git', ['-C', path, 'rev-parse', '--show-toplevel'])).resolves.toMatchObject({
      stdout: `${await realpath(path)}\n`
    })
    expect(database.get('SELECT kind FROM execution_contexts WHERE id = ?', 'context-worktree-1')).toEqual({
      kind: 'git-worktree'
    })
    expect(database.all<{ event_type: string }>(
      "SELECT event_type FROM domain_events WHERE aggregate_type = 'worktree' ORDER BY seq"
    )).toEqual([
      { event_type: 'worktree.creation-started' },
      { event_type: 'worktree.ready' }
    ])
  })

  it('retains a dirty worktree instead of deleting user changes', async () => {
    const path = join(root, 'worktrees', 'dirty')
    await service.create(command('create-dirty'), {
      id: 'worktree-dirty', executionContextId: 'context-dirty', workspaceId: 'workspace-1',
      repositoryRoot, path, branch: 'dirty-branch', baseRef: 'HEAD', setupPolicy: [], now: 10
    })
    await writeFile(join(path, 'uncommitted.txt'), 'keep me')

    const result = await service.remove(command('remove-dirty'), 'worktree-dirty', 20)

    expect(result.state).toBe('retained')
    expect(stopRuns).not.toHaveBeenCalled()
    await expect(exec('git', ['-C', path, 'status', '--porcelain'])).resolves.toMatchObject({
      stdout: expect.stringContaining('uncommitted.txt')
    })
  })

  it('stops bound SessionRuns before removing a clean worktree', async () => {
    const path = join(root, 'worktrees', 'clean')
    await service.create(command('create-clean'), {
      id: 'worktree-clean', executionContextId: 'context-clean', workspaceId: 'workspace-1',
      repositoryRoot, path, branch: 'clean-branch', baseRef: 'HEAD', setupPolicy: [], now: 10
    })
    seedBoundRun(database, 'context-clean')

    const result = await service.remove(command('remove-clean'), 'worktree-clean', 20)

    expect(stopRuns).toHaveBeenCalledWith(['run-1'])
    expect(result.state).toBe('removed')
    expect(database.get('SELECT archived_at FROM execution_contexts WHERE id = ?', 'context-clean')).toEqual({
      archived_at: 20
    })
  })

  it('records failed creation without losing the operation identity', async () => {
    await expect(service.create(command('create-failed'), {
      id: 'worktree-failed', executionContextId: 'context-failed', workspaceId: 'workspace-1',
      repositoryRoot: join(root, 'not-a-repository'), path: join(root, 'bad'),
      branch: 'bad', baseRef: 'HEAD', setupPolicy: [], now: 10
    })).rejects.toThrow()

    expect(database.get('SELECT state FROM worktrees WHERE id = ?', 'worktree-failed')).toEqual({
      state: 'failed'
    })
  })

  it('reuses its existing branch when a prior add failed before linking the worktree directory', async () => {
    const path = join(root, 'worktrees', 'retry-partial')
    await exec('git', ['-C', repositoryRoot, 'branch', 'retry-partial', 'HEAD'])

    const retried = await service.create(command('retry-partial'), {
      id: 'worktree-retry-partial', executionContextId: 'context-retry-partial',
      workspaceId: 'workspace-1', repositoryRoot, path,
      branch: 'retry-partial', baseRef: 'HEAD', setupPolicy: [], now: 20
    })

    expect(retried).toMatchObject({ state: 'ready', branch: 'retry-partial', path })
    await expect(exec('git', ['-C', path, 'branch', '--show-current'])).resolves.toMatchObject({
      stdout: 'retry-partial\n'
    })
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'worktree', requestHash: `hash-${commandId}` }
}

function seedBoundRun(db: RuntimeDatabase, contextId: string): void {
  db.transaction((tx) => {
    tx.run('INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, sort_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)', 'task-1', 'workspace-1', contextId, 'Task', 'active', 'a', 1, 1)
    tx.run('INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)', 'session-1', 'task-1', contextId, 'shell', 'running', 'Shell', 1, 1, 1)
    tx.run('INSERT INTO session_runs (id, session_id, ordinal, runtime_generation, status, started_at) VALUES (?, ?, ?, ?, ?, ?)', 'run-1', 'session-1', 1, 'generation', 'running', 1)
  })
}
