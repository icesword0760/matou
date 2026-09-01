import { execFile } from 'node:child_process'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { WorktreeHealthService } from './worktree-health-service'
import { WorktreeReconciler } from './worktree-reconciler'
import { WorktreeService } from './worktree-service'

const exec = promisify(execFile)

let root: string
let repositoryRoot: string
let database: RuntimeDatabase
let transactions: DomainTransactionManager
let worktrees: WorktreeService
let reconciler: WorktreeReconciler

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-worktree-reconciler-'))
  repositoryRoot = join(root, 'repository')
  await initializeRepository(repositoryRoot)
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  transactions = new DomainTransactionManager(database)
  const workspaces = new WorkspaceTaskRepository(database, transactions)
  workspaces.createWorkspace(command('workspace'), {
    id: 'workspace-1', name: 'Workspace', rootDirectory: repositoryRoot, now: 1
  })
  workspaces.createPlainExecutionContext(command('local-context'), {
    id: 'workspace-1:local', workspaceId: 'workspace-1', cwd: repositoryRoot, now: 1
  })
  worktrees = new WorktreeService(database, transactions, {
    stopRuns: vi.fn(async () => undefined)
  })
  reconciler = new WorktreeReconciler(database, transactions, worktrees, new WorktreeHealthService())
})

afterEach(async () => {
  database.close()
  await rm(root, { recursive: true, force: true })
})

describe('WorktreeReconciler', () => {
  it('continues a creating operation when only its branch exists', async () => {
    const path = join(root, 'worktrees', 'branch-only')
    await exec('git', ['-C', repositoryRoot, 'branch', 'feature/branch-only', 'HEAD'])
    seedWorktree({ id: 'branch-only', path, branch: 'feature/branch-only', state: 'creating' })

    await expect(reconciler.reconcileAll(100)).resolves.toEqual({
      checked: 1, repaired: 1, degraded: 0
    })
    expect(worktrees.get('branch-only')).toMatchObject({ state: 'ready', path })
    await expect(exec('git', ['-C', path, 'branch', '--show-current'])).resolves.toMatchObject({
      stdout: 'feature/branch-only\n'
    })
  })

  it('adopts a fully-created directory when the ready database write was interrupted', async () => {
    const path = join(root, 'worktrees', 'directory-ready')
    await exec('git', [
      '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/directory-ready', path, 'HEAD'
    ])
    seedWorktree({ id: 'directory-ready', path, branch: 'feature/directory-ready', state: 'creating' })

    const result = await reconciler.reconcileAll(100)

    expect(result).toEqual({ checked: 1, repaired: 1, degraded: 0 })
    expect(worktrees.get('directory-ready')?.state).toBe('ready')
  })

  it('restarts a fully-missing creating operation from its persisted base ref', async () => {
    const path = join(root, 'worktrees', 'fully-missing')
    seedWorktree({ id: 'fully-missing', path, branch: 'feature/fully-missing', state: 'creating' })

    const result = await reconciler.reconcileAll(100)

    expect(result).toEqual({ checked: 1, repaired: 1, degraded: 0 })
    expect(worktrees.get('fully-missing')?.state).toBe('ready')
    await expect(exec('git', ['-C', path, 'rev-parse', '--verify', 'HEAD'])).resolves.toBeDefined()
  })

  it('finishes a removing operation when the directory has already gone', async () => {
    const path = join(root, 'worktrees', 'already-removed')
    await exec('git', [
      '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/already-removed', path, 'HEAD'
    ])
    seedWorktree({ id: 'already-removed', path, branch: 'feature/already-removed', state: 'removing' })
    seedBoundSession('already-removed', 'context-already-removed')
    const sessionBefore = database.get('SELECT * FROM sessions WHERE id = ?', 'session-1')
    await rm(path, { recursive: true, force: true })

    const result = await reconciler.reconcileAll(100)

    expect(result).toEqual({ checked: 1, repaired: 1, degraded: 0 })
    expect(worktrees.get('already-removed')?.state).toBe('removed')
    expect(database.get(
      'SELECT archived_at FROM execution_contexts WHERE id = ?', 'context-already-removed'
    )).toEqual({ archived_at: 100 })
    expect((await exec('git', ['-C', repositoryRoot, 'worktree', 'list', '--porcelain'])).stdout)
      .not.toContain(path)
    expect(database.get(
      `SELECT active_target, state FROM session_environment_bindings
       WHERE session_id = 'session-1'`
    )).toEqual({ active_target: 'worktree', state: 'missing' })
    expect(database.get('SELECT * FROM sessions WHERE id = ?', 'session-1')).toEqual(sessionBefore)
  })

  it('retains a dirty worktree whose removing operation was interrupted', async () => {
    const path = join(root, 'worktrees', 'dirty-removing')
    await exec('git', [
      '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/dirty-removing', path, 'HEAD'
    ])
    await writeFile(join(path, 'keep.txt'), 'keep me\n')
    seedWorktree({ id: 'dirty-removing', path, branch: 'feature/dirty-removing', state: 'removing' })

    const result = await reconciler.reconcileAll(100)

    expect(result).toEqual({ checked: 1, repaired: 1, degraded: 0 })
    expect(worktrees.get('dirty-removing')?.state).toBe('retained')
    await expect(exec('git', ['-C', path, 'status', '--porcelain'])).resolves.toMatchObject({
      stdout: expect.stringContaining('keep.txt')
    })
  })

  it('marks only the active environment missing while preserving Session history and graph rows', async () => {
    const path = join(root, 'worktrees', 'missing-active')
    seedWorktree({ id: 'missing-active', path, branch: 'feature/missing-active', state: 'ready' })
    seedBoundSession('missing-active', 'context-missing-active')
    const sessionBefore = database.get('SELECT * FROM sessions WHERE id = ?', 'session-1')
    const eventsBefore = database.all('SELECT * FROM session_relation_events')

    const result = await reconciler.reconcileAll(100)

    expect(result).toEqual({ checked: 1, repaired: 0, degraded: 1 })
    expect(database.get(
      'SELECT active_target, state FROM session_environment_bindings WHERE session_id = ?',
      'session-1'
    )).toEqual({ active_target: 'worktree', state: 'missing' })
    expect(database.get('SELECT * FROM sessions WHERE id = ?', 'session-1')).toEqual(sessionBefore)
    expect(database.all('SELECT * FROM session_relation_events')).toEqual(eventsBefore)
  })

  it('isolates a failed setup, marks its active environment failed, and continues other worktrees', async () => {
    const failedPath = join(root, 'worktrees', 'setup-fails')
    const healthyPath = join(root, 'worktrees', 'continues')
    seedWorktree({
      id: 'setup-fails', path: failedPath, branch: 'feature/setup-fails', state: 'creating',
      setupPolicy: [{ command: '/usr/bin/false', args: [] }]
    })
    seedBoundSession('setup-fails', 'context-setup-fails')
    seedWorktree({
      id: 'continues', path: healthyPath, branch: 'feature/continues', state: 'creating'
    })

    const result = await reconciler.reconcileAll(100)

    expect(result).toEqual({ checked: 2, repaired: 1, degraded: 1 })
    expect(worktrees.get('continues')?.state).toBe('ready')
    expect(worktrees.get('setup-fails')?.state).toBe('failed')
    expect(database.get<{ state: string }>(
      `SELECT state FROM session_environment_bindings WHERE session_id = 'session-1'`
    )).toEqual({ state: 'failed' })
  })

  it('isolates a real Git index failure and continues reconciling other worktrees', async () => {
    const brokenPath = join(root, 'worktrees', 'broken-index')
    const healthyPath = join(root, 'worktrees', 'healthy-after-broken')
    await exec('git', [
      '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/broken-index', brokenPath, 'HEAD'
    ])
    await exec('git', [
      '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/healthy-after-broken', healthyPath, 'HEAD'
    ])
    seedWorktree({ id: 'broken-index', path: brokenPath, branch: 'feature/broken-index', state: 'ready' })
    seedWorktree({
      id: 'healthy-after-broken', path: healthyPath,
      branch: 'feature/healthy-after-broken', state: 'ready'
    })
    const brokenGitDirectory = (await exec(
      'git', ['-C', brokenPath, 'rev-parse', '--git-dir']
    )).stdout.trim()
    await rm(join(brokenGitDirectory, 'index'), { force: true })
    await mkdir(join(brokenGitDirectory, 'index'))

    const result = await reconciler.reconcileAll(100)

    expect(result).toEqual({ checked: 2, repaired: 0, degraded: 1 })
    expect(worktrees.get('broken-index')?.state).toBe('failed')
    expect(worktrees.get('healthy-after-broken')?.state).toBe('ready')
  })

  it('keeps a retained dirty Worktree visible and degrades it only when its path disappears', async () => {
    const path = join(root, 'worktrees', 'retained')
    await exec('git', [
      '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/retained', path, 'HEAD'
    ])
    await writeFile(join(path, 'keep.txt'), 'keep me\n')
    seedWorktree({ id: 'retained', path, branch: 'feature/retained', state: 'retained' })
    seedBoundSession('retained', 'context-retained')

    await expect(reconciler.reconcileAll(100)).resolves.toEqual({
      checked: 1, repaired: 0, degraded: 0
    })
    expect(worktrees.get('retained')?.state).toBe('retained')

    await rm(path, { recursive: true, force: true })
    await expect(reconciler.reconcileAll(101)).resolves.toEqual({
      checked: 1, repaired: 0, degraded: 1
    })
    expect(database.get(
      `SELECT state FROM session_environment_bindings WHERE session_id = 'session-1'`
    )).toEqual({ state: 'missing' })
  })

  it('accepts a registered detached Worktree only while its recorded HEAD still matches', async () => {
    const path = join(root, 'worktrees', 'detached')
    await exec('git', ['-C', repositoryRoot, 'worktree', 'add', '--detach', path, 'HEAD'])
    const head = (await exec('git', ['-C', path, 'rev-parse', 'HEAD'])).stdout.trim()
    seedWorktree({
      id: 'detached', path, branch: '(detached)', baseRevision: head, state: 'ready'
    })

    await expect(reconciler.reconcileAll(100)).resolves.toEqual({
      checked: 1, repaired: 0, degraded: 0
    })
    await writeFile(join(path, 'next.txt'), 'next\n')
    await exec('git', ['-C', path, 'add', 'next.txt'])
    await exec('git', ['-C', path, 'commit', '-m', 'next'])

    await expect(reconciler.reconcileAll(101)).resolves.toEqual({
      checked: 1, repaired: 0, degraded: 1
    })
    expect(worktrees.get('detached')?.state).toBe('failed')
  })

  it('marks the active environment failed when Git refuses to remove a locked Worktree', async () => {
    const path = join(root, 'worktrees', 'locked')
    await exec('git', [
      '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/locked', path, 'HEAD'
    ])
    await exec('git', ['-C', repositoryRoot, 'worktree', 'lock', path])
    seedWorktree({ id: 'locked', path, branch: 'feature/locked', state: 'removing' })
    seedBoundSession('locked', 'context-locked')
    const sessionBefore = database.get('SELECT * FROM sessions WHERE id = ?', 'session-1')

    await expect(reconciler.reconcileAll(100)).resolves.toEqual({
      checked: 1, repaired: 0, degraded: 1
    })
    expect(worktrees.get('locked')?.state).toBe('failed')
    expect(database.get(
      `SELECT active_target, state FROM session_environment_bindings
       WHERE session_id = 'session-1'`
    )).toEqual({ active_target: 'worktree', state: 'failed' })
    expect(database.get('SELECT * FROM sessions WHERE id = ?', 'session-1')).toEqual(sessionBefore)
  })
})

function seedWorktree(input: {
  id: string
  path: string
  branch: string
  baseRevision?: string
  state: 'creating' | 'ready' | 'retained' | 'removing'
  setupPolicy?: Array<{ command: string; args: string[] }>
}): void {
  database.transaction((tx) => {
    tx.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES (?, 'workspace-1', 'git-worktree', ?, 1)`,
      `context-${input.id}`, input.path
    )
    tx.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         base_ref, base_revision, state, setup_policy_json, setup_result_json,
         cleanup_policy, created_at, updated_at
       ) VALUES (?, ?, ?, ?, ?, 'HEAD', ?, ?, ?, '[]', 'retain-dirty', 1, 1)`,
      input.id, `context-${input.id}`, repositoryRoot, input.path, input.branch,
      input.baseRevision ?? null, input.state,
      JSON.stringify(input.setupPolicy ?? [])
    )
  })
}

function seedBoundSession(worktreeId: string, worktreeContextId: string): void {
  database.transaction((tx) => {
    tx.run(
      `INSERT INTO tasks (
         id, workspace_id, execution_context_id, title, status, sort_key, created_at, updated_at
       ) VALUES ('task-1', 'workspace-1', 'workspace-1:local', 'Task', 'active', 'a', 1, 1)`
    )
    tx.run(
      `INSERT INTO sessions (
         id, task_id, execution_context_id, kind, status, title, cwd,
         created_at, updated_at, last_activity_at
       ) VALUES ('session-1', 'task-1', ?, 'shell', 'exited', 'Shell', ?, 1, 1, 1)`,
      worktreeContextId,
      worktrees.get(worktreeId)!.path
    )
    tx.run(
      `UPDATE session_environment_bindings
       SET managed_worktree_id = ?, active_target = 'worktree', state = 'ready', updated_at = 1
       WHERE session_id = 'session-1'`,
      worktreeId
    )
  })
}

function command(commandId: string) {
  return { commandId, commandType: 'worktree.reconcile', requestHash: `hash-${commandId}` }
}

async function initializeRepository(path: string): Promise<void> {
  await exec('git', ['init', path])
  await exec('git', ['-C', path, 'config', 'user.email', 'matou@example.test'])
  await exec('git', ['-C', path, 'config', 'user.name', 'Matou Test'])
  await writeFile(join(path, 'README.md'), 'root\n')
  await exec('git', ['-C', path, 'add', 'README.md'])
  await exec('git', ['-C', path, 'commit', '-m', 'initial'])
}
