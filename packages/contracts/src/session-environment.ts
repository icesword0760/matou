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
