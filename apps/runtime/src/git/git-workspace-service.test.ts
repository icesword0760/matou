import { execFile } from 'node:child_process'
import { mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

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
