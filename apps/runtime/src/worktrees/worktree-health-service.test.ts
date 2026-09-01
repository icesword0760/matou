import { execFile } from 'node:child_process'
import { mkdtemp, realpath, rename, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { WorktreeHealthService } from './worktree-health-service'

const exec = promisify(execFile)

let root: string
let repositoryRoot: string
let worktreePath: string
let service: WorktreeHealthService

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-worktree-health-'))
  repositoryRoot = join(root, 'repository')
  worktreePath = join(root, 'worktrees', 'feature')
  await initializeRepository(repositoryRoot)
  await exec('git', [
    '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/health', worktreePath, 'HEAD'
  ])
  service = new WorktreeHealthService()
})

afterEach(async () => {
  await rm(root, { recursive: true, force: true })
})

describe('WorktreeHealthService', () => {
  it('returns the canonical path, branch and clean status for a registered worktree', async () => {
    await expect(service.check({
      repositoryRoot,
      path: worktreePath,
      expectedBranch: 'feature/health'
    })).resolves.toEqual({
      kind: 'ready',
      canonicalPath: await realpath(worktreePath),
      branch: 'feature/health',
      dirty: false
    })
  })

  it('reports a deleted directory as path-missing', async () => {
    await rm(worktreePath, { recursive: true, force: true })

    await expect(service.check({
      repositoryRoot,
      path: worktreePath,
      expectedBranch: 'feature/health'
    })).resolves.toEqual({ kind: 'missing', reason: 'path-missing' })
  })

  it('reports a moved worktree that Git has not registered at its new path', async () => {
    const movedPath = join(root, 'moved-feature')
    await rename(worktreePath, movedPath)

    await expect(service.check({
      repositoryRoot,
      path: movedPath,
      expectedBranch: 'feature/health'
    })).resolves.toEqual({ kind: 'missing', reason: 'not-listed-by-git' })
  })

  it('reports a path belonging to another repository as wrong-repository', async () => {
    const otherRepository = join(root, 'other-repository')
    await initializeRepository(otherRepository)

    await expect(service.check({
      repositoryRoot,
      path: otherRepository,
      expectedBranch: 'feature/health'
    })).resolves.toEqual({ kind: 'mismatch', reason: 'wrong-repository' })
  })

  it('reports a missing registered repository root as wrong-repository', async () => {
    await expect(service.check({
      repositoryRoot: join(root, 'repository-that-moved'),
      path: worktreePath,
      expectedBranch: 'feature/health'
    })).resolves.toEqual({ kind: 'mismatch', reason: 'wrong-repository' })
  })

  it('reports a different symbolic branch as wrong-branch', async () => {
    await exec('git', ['-C', worktreePath, 'switch', '-c', 'feature/other'])

    await expect(service.check({
      repositoryRoot,
      path: worktreePath,
      expectedBranch: 'feature/health'
    })).resolves.toEqual({ kind: 'mismatch', reason: 'wrong-branch' })
  })

  it('reports a detached HEAD as a healthy detached identity', async () => {
    const head = (await exec('git', ['-C', worktreePath, 'rev-parse', 'HEAD'])).stdout.trim()
    await exec('git', ['-C', worktreePath, 'checkout', '--detach', head])

    await expect(service.check({
      repositoryRoot,
      path: worktreePath
    })).resolves.toEqual({
      kind: 'ready',
      canonicalPath: await realpath(worktreePath),
      detachedHead: head,
      dirty: false
    })
  })

  it('rejects detached HEAD when the managed identity requires its recorded branch', async () => {
    const head = (await exec('git', ['-C', worktreePath, 'rev-parse', 'HEAD'])).stdout.trim()
    await exec('git', ['-C', worktreePath, 'checkout', '--detach', head])

    await expect(service.check({
      repositoryRoot,
      path: worktreePath,
      expectedBranch: 'feature/health'
    })).resolves.toEqual({ kind: 'mismatch', reason: 'wrong-head' })
  })

  it('reports an unexpected detached HEAD as wrong-head when a detached identity is required', async () => {
    const expectedHead = (await exec('git', ['-C', worktreePath, 'rev-parse', 'HEAD'])).stdout.trim()
    await writeFile(join(worktreePath, 'second.txt'), 'second\n')
    await exec('git', ['-C', worktreePath, 'add', 'second.txt'])
    await exec('git', ['-C', worktreePath, 'commit', '-m', 'second'])
    await exec('git', ['-C', worktreePath, 'checkout', '--detach', 'HEAD'])

    await expect(service.check({
      repositoryRoot,
      path: worktreePath,
      expectedDetachedHead: expectedHead
    })).resolves.toEqual({ kind: 'mismatch', reason: 'wrong-head' })
  })

  it('reports tracked and untracked changes through the dirty flag', async () => {
    await writeFile(join(worktreePath, 'uncommitted.txt'), 'keep me\n')

    await expect(service.check({
      repositoryRoot,
      path: worktreePath,
      expectedBranch: 'feature/health'
    })).resolves.toMatchObject({ kind: 'ready', dirty: true })
  })
})

async function initializeRepository(path: string): Promise<void> {
  await exec('git', ['init', path])
  await exec('git', ['-C', path, 'config', 'user.email', 'matou@example.test'])
  await exec('git', ['-C', path, 'config', 'user.name', 'Matou Test'])
  await writeFile(join(path, 'README.md'), 'root\n')
  await exec('git', ['-C', path, 'add', 'README.md'])
  await exec('git', ['-C', path, 'commit', '-m', 'initial'])
}
