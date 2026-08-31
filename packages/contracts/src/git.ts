export interface GitBranchSummary {
  name: string
  current: boolean
  commitTimestamp: number
  checkedOutPath?: string
}

export interface GitWorktreeSummary {
  path: string
  branch: string
  head: string
  current: boolean
  main: boolean
  dirty: boolean
  managed: boolean
  sessionCount: number
  worktreeId?: string
}

export interface GitRepositoryStatus {
  repositoryRoot: string
  cwd: string
  currentBranch?: string
  detachedHead?: string
  defaultBranch?: string
  upstream?: string
  dirty: boolean
  stagedCount: number
  unstagedCount: number
  untrackedCount: number
  additions: number
  deletions: number
  ahead: number
  behind: number
  hasRemote: boolean
  canPush: boolean
  branches: GitBranchSummary[]
  worktrees: GitWorktreeSummary[]
}

export type GitCheckoutResult =
  | { kind: 'switched'; status: GitRepositoryStatus }
  | {
      kind: 'blocked-by-working-tree-changes'
      targetBranch: string
      conflictingPaths: string[]
      status: GitRepositoryStatus
    }
