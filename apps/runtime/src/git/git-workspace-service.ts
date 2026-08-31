import { execFile } from 'node:child_process'
import { realpath } from 'node:fs/promises'
import { promisify } from 'node:util'

import type {
  GitBranchSummary,
  GitCheckoutResult,
  GitRepositoryStatus,
  GitWorktreeSummary
} from '@matou/contracts'

const exec = promisify(execFile)
const MAX_GIT_OUTPUT = 8 * 1024 * 1024

export class GitWorkspaceService {
  async status(cwd: string): Promise<GitRepositoryStatus> {
    const repositoryRoot = (await git(cwd, ['rev-parse', '--show-toplevel'])).trim()
    const actualCwd = await realpath(cwd).catch(() => cwd)
    const [branchOutput, head, porcelain, numstat, remotes, worktreeOutput] = await Promise.all([
      git(repositoryRoot, ['branch', '--show-current']),
      git(repositoryRoot, ['rev-parse', '--short=12', 'HEAD']),
      git(repositoryRoot, ['status', '--porcelain=v1', '--untracked-files=normal']),
      git(repositoryRoot, ['diff', '--numstat', 'HEAD']),
      git(repositoryRoot, ['remote']),
      git(repositoryRoot, ['worktree', 'list', '--porcelain'])
    ])
    const currentBranch = branchOutput.trim() || undefined
    const files = parsePorcelain(porcelain)
    const lineStats = parseNumstat(numstat)
    const upstream = await optionalGit(repositoryRoot, [
      'rev-parse', '--abbrev-ref', '--symbolic-full-name', '@{upstream}'
    ])
    const sync = upstream
      ? parseAheadBehind(await git(repositoryRoot, [
          'rev-list', '--left-right', '--count', `${upstream.trim()}...HEAD`
        ]))
      : { ahead: 0, behind: 0 }
    const worktrees = await this.#worktrees(repositoryRoot, actualCwd, worktreeOutput)
    const branchRows = await this.#branches(repositoryRoot, currentBranch, worktrees)
    const defaultBranch = await findDefaultBranch(repositoryRoot, branchRows)
    const branches = sortBranches(branchRows, defaultBranch, currentBranch)
    const hasRemote = remotes.trim().length > 0
    return {
      repositoryRoot,
      cwd: actualCwd,
      ...(currentBranch ? { currentBranch } : { detachedHead: head.trim() }),
      ...(defaultBranch ? { defaultBranch } : {}),
      ...(upstream ? { upstream: upstream.trim() } : {}),
      dirty: files.stagedCount + files.unstagedCount + files.untrackedCount > 0,
      ...files,
      ...lineStats,
      ...sync,
      hasRemote,
      canPush: Boolean(hasRemote && currentBranch && (upstream === undefined || sync.ahead > 0)),
      branches,
      worktrees
    }
  }

  async checkout(cwd: string, branch: string): Promise<GitCheckoutResult> {
    const targetBranch = requiredBranch(branch)
    try {
      await git(cwd, ['checkout', targetBranch])
      return { kind: 'switched', status: await this.status(cwd) }
    } catch (error) {
      const output = gitErrorOutput(error)
      if (!/would be overwritten by checkout/i.test(output)) throw gitError(error)
      return {
        kind: 'blocked-by-working-tree-changes',
        targetBranch,
        conflictingPaths: checkoutConflictPaths(output),
        status: await this.status(cwd)
      }
    }
  }

  async createBranch(cwd: string, branch: string): Promise<GitRepositoryStatus> {
    await git(cwd, ['checkout', '-b', requiredBranch(branch)])
    return this.status(cwd)
  }

  async commit(
    cwd: string,
    input: { message: string; includeUnstaged: boolean }
  ): Promise<GitRepositoryStatus> {
    const message = input.message.trim()
    if (!message) throw new Error('请输入提交信息')
    if (input.includeUnstaged) await git(cwd, ['add', '-A'])
    await git(cwd, ['commit', '-m', message])
    return this.status(cwd)
  }

  async push(cwd: string): Promise<GitRepositoryStatus> {
    const status = await this.status(cwd)
    if (!status.currentBranch) throw new Error('当前处于 detached HEAD，请先创建分支')
    if (status.upstream) {
      await git(cwd, ['push'])
    } else {
      const remotes = (await git(cwd, ['remote'])).split('\n').map((item) => item.trim()).filter(Boolean)
      const remote = remotes.includes('origin') ? 'origin' : remotes[0]
      if (!remote) throw new Error('仓库尚未配置远端')
      await git(cwd, [
        'push', '--set-upstream', remote, `HEAD:refs/heads/${status.currentBranch}`
      ])
    }
    return this.status(cwd)
  }

  async #branches(
    repositoryRoot: string,
    currentBranch: string | undefined,
    worktrees: GitWorktreeSummary[]
  ): Promise<GitBranchSummary[]> {
    const output = await git(repositoryRoot, [
      'for-each-ref', '--format=%(refname:short)%00%(committerdate:unix)', 'refs/heads'
    ])
    const checkoutByBranch = new Map(worktrees
      .filter(({ branch }) => branch !== '(detached)')
      .map(({ branch, path }) => [branch, path] as const))
    return output.split('\n').filter(Boolean).map((line) => {
      const [name = '', timestamp = '0'] = line.split('\0')
      const checkedOutPath = checkoutByBranch.get(name)
      return {
        name,
        current: name === currentBranch,
        commitTimestamp: Number(timestamp) || 0,
        ...(checkedOutPath ? { checkedOutPath } : {})
      }
    })
  }

  async #worktrees(
    repositoryRoot: string,
    actualCwd: string,
    output: string
  ): Promise<GitWorktreeSummary[]> {
    const parsed = parseWorktreePorcelain(output)
    return Promise.all(parsed.map(async (entry, index) => {
      const path = await realpath(entry.path).catch(() => entry.path)
      const dirty = (await optionalGit(path, [
        'status', '--porcelain=v1', '--untracked-files=normal'
      ]))?.trim().length !== 0
      return {
        path,
        branch: entry.branch,
        head: entry.head,
        current: actualCwd === path || actualCwd.startsWith(`${path}/`),
        main: index === 0 || path === repositoryRoot,
        dirty,
        managed: false,
        sessionCount: 0
      }
    }))
  }
}

function requiredBranch(value: string): string {
  const branch = value.trim()
  if (!branch) throw new Error('请输入分支名称')
  if (branch.startsWith('-')) throw new Error('分支名称格式不正确')
  return branch
}

async function git(cwd: string, args: string[]): Promise<string> {
  try {
    const result = await exec('git', ['-C', cwd, ...args], {
      encoding: 'utf8', maxBuffer: MAX_GIT_OUTPUT
    })
    return result.stdout
  } catch (error) {
    throw gitError(error)
  }
}

async function optionalGit(cwd: string, args: string[]): Promise<string | undefined> {
  try {
    return await git(cwd, args)
  } catch {
    return undefined
  }
}

function gitError(error: unknown): Error {
  if (error instanceof GitExecutionError) return error
  return new GitExecutionError(gitErrorOutput(error))
}

class GitExecutionError extends Error {
  constructor(message: string) {
    super(message.trim().slice(-16_384) || 'Git 操作失败')
    this.name = 'GitExecutionError'
  }
}

function gitErrorOutput(error: unknown): string {
  if (error instanceof GitExecutionError) return error.message
  if (typeof error !== 'object' || error === null) return String(error)
  const candidate = error as { stderr?: unknown; stdout?: unknown; message?: unknown }
  const parts = [candidate.stderr, candidate.stdout, candidate.message]
    .filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
  return parts.join('\n').trim() || 'Git 操作失败'
}

function parsePorcelain(output: string): {
  stagedCount: number; unstagedCount: number; untrackedCount: number
} {
  let stagedCount = 0
  let unstagedCount = 0
  let untrackedCount = 0
  for (const line of output.split('\n')) {
    if (line.length < 3) continue
    if (line.startsWith('??')) {
      untrackedCount += 1
      continue
    }
    if (line[0] !== ' ') stagedCount += 1
    if (line[1] !== ' ') unstagedCount += 1
  }
  return { stagedCount, unstagedCount, untrackedCount }
}

function parseNumstat(output: string): { additions: number; deletions: number } {
  let additions = 0
  let deletions = 0
  for (const line of output.split('\n')) {
    const [added, deleted] = line.split('\t')
    if (added && added !== '-') additions += Number(added) || 0
    if (deleted && deleted !== '-') deletions += Number(deleted) || 0
  }
  return { additions, deletions }
}

function parseAheadBehind(output: string): { ahead: number; behind: number } {
  const [behind = '0', ahead = '0'] = output.trim().split(/\s+/)
  return { ahead: Number(ahead) || 0, behind: Number(behind) || 0 }
}

function parseWorktreePorcelain(output: string): Array<{ path: string; head: string; branch: string }> {
  return output.trim().split(/\n\s*\n/).filter(Boolean).flatMap((block) => {
    const fields = new Map(block.split('\n').map((line) => {
      const separator = line.indexOf(' ')
      return separator < 0 ? [line, ''] : [line.slice(0, separator), line.slice(separator + 1)]
    }))
    const path = fields.get('worktree')
    const head = fields.get('HEAD')
    if (!path || !head) return []
    const ref = fields.get('branch')
    return [{ path, head, branch: ref?.replace(/^refs\/heads\//, '') ?? '(detached)' }]
  })
}

async function findDefaultBranch(
  repositoryRoot: string,
  branches: GitBranchSummary[]
): Promise<string | undefined> {
  const symbolic = await optionalGit(repositoryRoot, [
    'symbolic-ref', '--quiet', '--short', 'refs/remotes/origin/HEAD'
  ])
  const remoteDefault = symbolic?.trim().replace(/^origin\//, '')
  if (remoteDefault && branches.some(({ name }) => name === remoteDefault)) return remoteDefault
  if (branches.some(({ name }) => name === 'main')) return 'main'
  if (branches.some(({ name }) => name === 'master')) return 'master'
  return branches[0]?.name
}

function sortBranches(
  branches: GitBranchSummary[],
  defaultBranch: string | undefined,
  currentBranch: string | undefined
): GitBranchSummary[] {
  return [...branches].sort((left, right) => {
    const priority = (value: GitBranchSummary) => value.name === defaultBranch
      ? 0 : value.name === currentBranch ? 1 : 2
    return priority(left) - priority(right) ||
      right.commitTimestamp - left.commitTimestamp || left.name.localeCompare(right.name)
  })
}

function checkoutConflictPaths(output: string): string[] {
  const match = output.match(/would be overwritten by checkout:\s*([\s\S]*?)(?:Please commit|Please move|Aborting|$)/i)
  if (!match?.[1]) return []
  return match[1].split('\n').map((line) => line.trim()).filter(Boolean)
}
