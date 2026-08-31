import { execFile } from 'node:child_process'
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { GitWorkspaceService } from './git-workspace-service'

const exec = promisify(execFile)
let root: string
let repository: string
let service: GitWorkspaceService

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-git-workspace-'))
  repository = join(root, 'repository')
  await exec('git', ['init', '-b', 'main', repository])
  await exec('git', ['-C', repository, 'config', 'user.email', 'matou@example.test'])
  await exec('git', ['-C', repository, 'config', 'user.name', 'Matou Test'])
  await writeFile(join(repository, 'tracked.txt'), 'line one\nline two\n')
  await exec('git', ['-C', repository, 'add', 'tracked.txt'])
  await exec('git', ['-C', repository, 'commit', '-m', 'initial'])
  service = new GitWorkspaceService()
})

afterEach(async () => rm(root, { recursive: true, force: true }))

describe('GitWorkspaceService repository state', () => {
  it('reports branches, staged, unstaged, untracked and line summary from the real repository', async () => {
    await exec('git', ['-C', repository, 'branch', 'feature/demo'])
    await writeFile(join(repository, 'tracked.txt'), 'line one changed\nline two\nline three\n')
    await exec('git', ['-C', repository, 'add', 'tracked.txt'])
    await writeFile(join(repository, 'tracked.txt'), 'line one changed again\nline two\nline three\n')
    await writeFile(join(repository, 'untracked.txt'), 'new\n')

    const status = await service.status(repository)

    expect(status).toMatchObject({
      repositoryRoot: await realpath(repository),
      currentBranch: 'main',
      defaultBranch: 'main',
      dirty: true,
      stagedCount: 1,
      unstagedCount: 1,
      untrackedCount: 1,
      hasRemote: false,
      ahead: 0,
      behind: 0
    })
    expect(status.additions).toBeGreaterThan(0)
    expect(status.deletions).toBeGreaterThan(0)
    expect(status.branches.map(({ name }) => name)).toEqual(['main', 'feature/demo'])
    expect(status.branches.find(({ name }) => name === 'main')).toMatchObject({ current: true })
  })

  it('lets Git carry safe local edits across a branch checkout', async () => {
    await exec('git', ['-C', repository, 'branch', 'feature/safe'])
    await writeFile(join(repository, 'local-only.txt'), 'keep\n')

    const result = await service.checkout(repository, 'feature/safe')

    expect(result.kind).toBe('switched')
    expect(result.status.currentBranch).toBe('feature/safe')
    expect(await readFile(join(repository, 'local-only.txt'), 'utf8')).toBe('keep\n')
  })

  it('returns a product conflict with paths when checkout would overwrite local edits', async () => {
    await exec('git', ['-C', repository, 'checkout', '-b', 'feature/conflict'])
    await writeFile(join(repository, 'tracked.txt'), 'branch version\n')
    await exec('git', ['-C', repository, 'commit', '-am', 'branch edit'])
    await exec('git', ['-C', repository, 'checkout', 'main'])
    await writeFile(join(repository, 'tracked.txt'), 'local version\n')

    const result = await service.checkout(repository, 'feature/conflict')

    expect(result).toMatchObject({
      kind: 'blocked-by-working-tree-changes',
      targetBranch: 'feature/conflict',
      conflictingPaths: ['tracked.txt']
    })
    expect(result.status.currentBranch).toBe('main')
  })

  it('creates and checks out a new branch from HEAD', async () => {
    const status = await service.createBranch(repository, 'feature/new-work')

    expect(status.currentBranch).toBe('feature/new-work')
    await expect(exec('git', [
      '-C', repository, 'show-ref', '--verify', 'refs/heads/feature/new-work'
    ])).resolves.toBeTruthy()
  })
})

describe('GitWorkspaceService commit and push', () => {
  it('commits only staged changes when includeUnstaged is off', async () => {
    await writeFile(join(repository, 'staged.txt'), 'staged\n')
    await writeFile(join(repository, 'untracked.txt'), 'untracked\n')
    await exec('git', ['-C', repository, 'add', 'staged.txt'])

    const status = await service.commit(repository, {
      message: 'feat: staged only', includeUnstaged: false
    })

    expect(status.untrackedCount).toBe(1)
    expect((await exec('git', ['-C', repository, 'show', '--format=', '--name-only', 'HEAD'])).stdout)
      .toContain('staged.txt')
    expect((await exec('git', ['-C', repository, 'show', '--format=', '--name-only', 'HEAD'])).stdout)
      .not.toContain('untracked.txt')
  })

  it('includes staged, unstaged and untracked changes when requested', async () => {
    await writeFile(join(repository, 'tracked.txt'), 'updated\n')
    await writeFile(join(repository, 'new.txt'), 'new\n')

    const status = await service.commit(repository, {
      message: 'feat: include all', includeUnstaged: true
    })

    expect(status.dirty).toBe(false)
    expect((await exec('git', ['-C', repository, 'show', '--format=', '--name-only', 'HEAD'])).stdout)
      .toContain('new.txt')
  })

  it('rejects a blank commit message before mutating the index', async () => {
    await writeFile(join(repository, 'new.txt'), 'new\n')

    await expect(service.commit(repository, { message: '   ', includeUnstaged: true }))
      .rejects.toThrow('请输入提交信息')
    expect((await exec('git', ['-C', repository, 'status', '--porcelain'])).stdout)
      .toContain('?? new.txt')
  })

  it('sets upstream on first push and uses it on later pushes', async () => {
    const remote = join(root, 'remote.git')
    await exec('git', ['init', '--bare', remote])
    await exec('git', ['-C', repository, 'remote', 'add', 'origin', remote])

    let status = await service.push(repository)
    expect(status.upstream).toBe('origin/main')
    expect(status.ahead).toBe(0)

    await writeFile(join(repository, 'second.txt'), 'second\n')
    await service.commit(repository, { message: 'feat: second', includeUnstaged: true })
    status = await service.push(repository)

    expect(status.ahead).toBe(0)
    expect((await exec('git', ['--git-dir', remote, 'rev-parse', 'refs/heads/main'])).stdout.trim())
      .toBe((await exec('git', ['-C', repository, 'rev-parse', 'HEAD'])).stdout.trim())
  })
})

describe('GitWorkspaceService worktrees', () => {
  it('creates a managed Worktree and merges its Matou session count into Git discovery', async () => {
    const database = await openDatabase()
    try {
      const managed = new GitWorkspaceService({ database, dataRoot: root })

      const status = await managed.createWorktree(command('create-worktree'), {
        workspaceId: 'workspace-1', repositoryRoot: repository,
        branch: 'feature/managed', baseRef: 'HEAD', now: 10
      })
      const created = status.worktrees.find(({ branch }) => branch === 'feature/managed')
      expect(created).toMatchObject({ managed: true, dirty: false, sessionCount: 0 })
      expect(created?.path).toContain(join(root, 'worktrees', 'workspace-1'))

      seedSession(database, created!.worktreeId!, created!.path)
      const refreshed = await managed.status(repository)
      expect(refreshed.worktrees.find(({ branch }) => branch === 'feature/managed'))
        .toMatchObject({ managed: true, sessionCount: 1 })
    } finally {
      database.close()
    }
  })

  it('registers an existing external Worktree for a session without making it Matou-removable', async () => {
    const database = await openDatabase()
    try {
      const externalPath = join(root, 'external-worktree')
      await exec('git', [
        '-C', repository, 'worktree', 'add', '-b', 'feature/external', externalPath, 'HEAD'
      ])
      const managed = new GitWorkspaceService({ database, dataRoot: root })

      const context = await managed.ensureWorktreeContext(command('register-external'), {
        workspaceId: 'workspace-1', repositoryRoot: repository,
        path: externalPath, branch: 'feature/external', now: 20
      })
      const second = await managed.ensureWorktreeContext(command('register-external-again'), {
        workspaceId: 'workspace-1', repositoryRoot: repository,
        path: externalPath, branch: 'feature/external', now: 21
      })

      expect(second).toEqual(context)
      expect(database.get('SELECT kind, cwd FROM execution_contexts WHERE id = ?', context.executionContextId))
        .toEqual({ kind: 'git-worktree', cwd: await realpath(externalPath) })
      expect((await managed.status(repository)).worktrees.find(({ branch }) => branch === 'feature/external'))
        .toMatchObject({ managed: false })
    } finally {
      database.close()
    }
  })

  it('keeps a managed Worktree when it is dirty or still has sessions', async () => {
    const database = await openDatabase()
    try {
      const managed = new GitWorkspaceService({ database, dataRoot: root })
      let status = await managed.createWorktree(command('create-retained'), {
        workspaceId: 'workspace-1', repositoryRoot: repository,
        branch: 'feature/retained', baseRef: 'HEAD', now: 10
      })
      const created = status.worktrees.find(({ branch }) => branch === 'feature/retained')!
      seedSession(database, created.worktreeId!, created.path)

      await expect(managed.removeWorktree(command('remove-with-session'), {
        worktreeId: created.worktreeId!, now: 30
      })).rejects.toThrow('仍有关联会话')

      database.run('UPDATE sessions SET archived_at = 31, status = ? WHERE id = ?', 'archived', 'session-worktree')
      await writeFile(join(created.path, 'local.txt'), 'keep\n')
      status = await managed.removeWorktree(command('remove-dirty'), {
        worktreeId: created.worktreeId!, now: 32
      })

      expect(status.worktrees.find(({ branch }) => branch === 'feature/retained')).toMatchObject({
        managed: true, dirty: true
      })
      expect(database.get('SELECT state FROM worktrees WHERE id = ?', created.worktreeId!))
        .toEqual({ state: 'retained' })
    } finally {
      database.close()
    }
  })
})

async function openDatabase(): Promise<RuntimeDatabase> {
  const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  new WorkspaceTaskRepository(database, transactions).createWorkspace(command('workspace'), {
    id: 'workspace-1', name: 'Workspace', rootDirectory: repository, now: 1
  })
  return database
}

function seedSession(database: RuntimeDatabase, worktreeId: string, path: string): void {
  const worktree = database.get<{ execution_context_id: string }>(
    'SELECT execution_context_id FROM worktrees WHERE id = ?', worktreeId
  )!
  database.transaction((tx) => {
    tx.run(
      `INSERT INTO tasks (
         id, workspace_id, execution_context_id, title, status, sort_key,
         created_at, updated_at, last_opened_at, version
       ) VALUES (?, ?, ?, ?, 'active', 'a', 1, 1, 1, 1)`,
      'task-worktree', 'workspace-1', worktree.execution_context_id, 'Worktree'
    )
    tx.run(
      `INSERT INTO sessions (
         id, task_id, execution_context_id, kind, status, title, cwd,
         created_at, updated_at, last_activity_at, version
       ) VALUES (?, ?, ?, 'shell', 'running', 'Shell', ?, 1, 1, 1, 1)`,
      'session-worktree', 'task-worktree', worktree.execution_context_id, path
    )
  })
}

function command(commandId: string) {
  return { commandId, commandType: 'git-workspace', requestHash: `hash-${commandId}` }
}
