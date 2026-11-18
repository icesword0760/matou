import type { SessionEnvironmentBinding } from '@matou/domain'

import type { GitRepositoryStatus } from './git'

export type SessionEnvironmentTarget = 'local' | 'worktree'

export type SessionEnvironmentLocateReason =
  | 'path-missing'
  | 'not-worktree'
  | 'wrong-repository'
  | 'not-listed-by-git'
  | 'wrong-branch'
  | 'wrong-head'
  | 'path-owned-by-another-session'
  | 'path-conflict'

export type SessionEnvironmentActionResult =
  | {
      kind: 'environment'
      sessionId: string
      activeTarget: SessionEnvironmentTarget
      state: 'ready' | 'missing' | 'recovering' | 'handoff' | 'failed'
      path: string
      restartRequired: boolean
    }
  | {
      kind: 'switch-session'
      sessionId: string
    }
  | {
      kind: 'rejected'
      sessionId: string
      reason: SessionEnvironmentLocateReason
    }

export interface SessionEnvironmentOpenResult {
  sessionId: string
  kind: SessionEnvironmentTarget
  path: string
}
export type SessionEnvironmentBindingView = SessionEnvironmentBinding

export type SessionEnvironmentIsolateResult =
  | {
      kind: 'switched'
      environment: SessionEnvironmentBindingView
      status: GitRepositoryStatus
      worktree: { id: string; path: string; branch: string }
    }
  | {
      kind: 'rejected'
      reason: 'environment-not-local' | 'branch-checked-out' | 'branch-missing' |
        'not-a-repository' | 'worktree-create-failed'
      message?: string
      status?: GitRepositoryStatus
    }
