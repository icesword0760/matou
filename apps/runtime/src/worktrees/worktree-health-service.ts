import { execFile } from 'node:child_process'
import { realpath } from 'node:fs/promises'
import { isAbsolute, resolve } from 'node:path'
import { promisify } from 'node:util'

const exec = promisify(execFile)

export type WorktreeHealth =
  | {
      kind: 'ready'
      canonicalPath: string
      branch?: string
      detachedHead?: string
      dirty: boolean
    }
  | { kind: 'missing'; reason: 'path-missing' | 'not-listed-by-git' }
  | { kind: 'mismatch'; reason: 'wrong-repository' | 'wrong-branch' | 'wrong-head' }

export interface WorktreeIdentityExpectation {
  repositoryRoot: string
  path: string
  expectedBranch?: string
  expectedDetachedHead?: string
}

export function managedWorktreeIdentityExpectation(input: {
  repositoryRoot: string
  path: string
  branch: string
  baseRevision?: string | null
}): WorktreeIdentityExpectation {
  if (input.branch === '(detached)') {
    if (!input.baseRevision) throw new Error('detached Worktree identity requires a base revision')
    return {
      repositoryRoot: input.repositoryRoot,
      path: input.path,
      expectedDetachedHead: input.baseRevision
    }
  }
  return {
    repositoryRoot: input.repositoryRoot,
    path: input.path,
    expectedBranch: input.branch
  }
}

export class WorktreeHealthService {
  async check(expectation: WorktreeIdentityExpectation): Promise<WorktreeHealth> {
    const canonicalPath = await realpath(expectation.path).catch(() => undefined)
    if (!canonicalPath) return { kind: 'missing', reason: 'path-missing' }

    const expectedRepository = await gitRepositoryIdentity(expectation.repositoryRoot).catch(() => undefined)
    if (!expectedRepository) return { kind: 'mismatch', reason: 'wrong-repository' }
    const actualRepository = await gitRepositoryIdentity(canonicalPath).catch(() => undefined)
    if (!actualRepository || actualRepository.commonDirectory !== expectedRepository.commonDirectory) {
      return { kind: 'mismatch', reason: 'wrong-repository' }
    }

    const listedPaths = await registeredWorktreePaths(expectedRepository.root)
    if (!listedPaths.has(canonicalPath)) {
      return { kind: 'missing', reason: 'not-listed-by-git' }
    }

    const branch = await symbolicBranch(canonicalPath)
    if (branch !== undefined) {
      if (expectation.expectedDetachedHead !== undefined) {
        return { kind: 'mismatch', reason: 'wrong-head' }
      }
      if (
        expectation.expectedBranch !== undefined &&
        branch !== expectation.expectedBranch
      ) {
        return { kind: 'mismatch', reason: 'wrong-branch' }
      }
      return {
        kind: 'ready',
        canonicalPath,
        branch,
        dirty: await isDirty(canonicalPath)
      }
    }

    const detachedHead = (await git(canonicalPath, ['rev-parse', 'HEAD'])).trim()
    if (expectation.expectedBranch !== undefined) {
      return { kind: 'mismatch', reason: 'wrong-head' }
    }
    if (
      expectation.expectedDetachedHead !== undefined &&
      detachedHead !== expectation.expectedDetachedHead
    ) {
      return { kind: 'mismatch', reason: 'wrong-head' }
    }
    return {
      kind: 'ready',
      canonicalPath,
      detachedHead,
      dirty: await isDirty(canonicalPath)
    }
  }
}

async function gitRepositoryIdentity(path: string): Promise<{
  root: string
  commonDirectory: string
}> {
  const rootOutput = (await git(path, ['rev-parse', '--show-toplevel'])).trim()
  const root = await realpath(rootOutput)
  const commonOutput = (await git(path, ['rev-parse', '--git-common-dir'])).trim()
  const commonPath = isAbsolute(commonOutput) ? commonOutput : resolve(path, commonOutput)
  return { root, commonDirectory: await realpath(commonPath) }
}

async function registeredWorktreePaths(repositoryRoot: string): Promise<Set<string>> {
  const output = await git(repositoryRoot, ['worktree', 'list', '--porcelain'])
  const paths = output
    .split('\n')
    .filter((line) => line.startsWith('worktree '))
    .map((line) => line.slice('worktree '.length))
  const canonical = await Promise.all(paths.map((path) => realpath(path).catch(() => undefined)))
  return new Set(canonical.filter((path): path is string => path !== undefined))
}

async function symbolicBranch(path: string): Promise<string | undefined> {
  try {
    return (await git(path, ['symbolic-ref', '--quiet', '--short', 'HEAD'])).trim()
  } catch (error) {
    if (commandExitCode(error) === 1) return undefined
    throw error
  }
}

async function isDirty(path: string): Promise<boolean> {
  return (await git(path, ['status', '--porcelain'])).trim() !== ''
}

async function git(path: string, args: string[]): Promise<string> {
  return (await exec('git', ['-c', 'core.quotePath=false', '-C', path, ...args])).stdout
}

function commandExitCode(error: unknown): number | undefined {
  if (typeof error !== 'object' || error === null || !('code' in error)) return undefined
  return typeof error.code === 'number' ? error.code : undefined
}
