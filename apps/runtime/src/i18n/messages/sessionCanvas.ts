import type { CatalogShape } from '../catalog'

export const sessionCanvasZhCN = {
  // session-canvas-service.ts
  detachedSessionMustReturn: '独立窗口中的会话需要先回到原会话列表',
  sessionNotInCanvas: '会话不在当前画布中',
  /** Default canvas name, read when a canvas is created. */
  newCanvas: '新画布',
  newCanvasNumbered: (suffix: number) => `新画布 ${suffix}`,

  // session-interaction-service.ts
  interactionKindNotOrdering: '用户交互类型不参与会话排序',

  // provider-mode-service.ts
  retryRestoreOnlyFailed: '只有恢复失败的 Claude Code 会话需要重试',
  startFreshOnlyFailed: '只有恢复失败的 Claude Code 会话可新开对话',
  recoveryFailedTitle: 'Claude Code 恢复失败',
  recoveryIncomplete: 'Claude Code 会话恢复未完成',

  // branch-name.ts — the fork name typed in the branch dialog
  branchName: {
    required: '请输入分支名称',
    tooLong: '分支名称最多 64 个字符',
    duplicate: (displayName: string) => `同一层已存在“${displayName}”`
  },

  // fork-workflow-service.ts
  fork: {
    notFailedRetry: '当前分支无需重试',
    notFailedRemove: '当前分支无需移除',
    detachedSource: '请先把会话返回当前画布',
    rootHasNoForkParent: '根层会话可创建子分支',
    sourceNotReady: '完成首轮 AI 对话后可创建分支',
    currentEnvironmentUnavailable: '当前执行环境已不可用',
    worktreeUnavailable: '指定的 Worktree 已不可用',
    worktreeBranchMismatch: (branch: string) => `Worktree 当前分支与提交的 ${branch} 不一致`,
    gitRepositoryRequired: '新工作树需要 Git 仓库',
    invalidBranch: '分支名称无效',
    branchExists: (branch: string) => `分支 ${branch} 已存在`,
    branchReserved: (branch: string) => `分支 ${branch} 已被其他 Fork 预留`,
    /** fork-operation-coordinator.ts; kept in step with `server.forkIdentityTimeout`. */
    identityTimeout: 'Fork 会话身份确认超时，请重试'
  }
}

export const sessionCanvasEn: CatalogShape<typeof sessionCanvasZhCN> = {
  detachedSessionMustReturn: 'A session in a detached window has to return to its session list first',
  sessionNotInCanvas: 'That session is not in this canvas',
  newCanvas: 'New canvas',
  newCanvasNumbered: (suffix) => `New canvas ${suffix}`,

  interactionKindNotOrdering: 'This user interaction kind does not take part in session ordering',

  retryRestoreOnlyFailed: 'Only a Claude Code session whose restore failed needs a retry',
  startFreshOnlyFailed: 'Only a Claude Code session whose restore failed can start a fresh conversation',
  recoveryFailedTitle: 'Claude Code recovery failed',
  recoveryIncomplete: 'The Claude Code session was not fully restored',

  branchName: {
    required: 'Enter a fork name',
    tooLong: 'The fork name can be at most 64 characters',
    duplicate: (displayName) => `"${displayName}" already exists at this level`
  },

  fork: {
    notFailedRetry: 'This fork does not need a retry',
    notFailedRemove: 'This fork does not need removing',
    detachedSource: 'Return the session to its canvas first',
    rootHasNoForkParent: 'A root session can only create child forks',
    sourceNotReady: 'Forking becomes available after the first AI exchange',
    currentEnvironmentUnavailable: 'The current execution environment is no longer available',
    worktreeUnavailable: 'The requested worktree is no longer available',
    worktreeBranchMismatch: (branch) => `The worktree is not on the submitted branch ${branch}`,
    gitRepositoryRequired: 'A new worktree needs a Git repository',
    invalidBranch: 'The branch name is invalid',
    branchExists: (branch) => `Branch ${branch} already exists`,
    branchReserved: (branch) => `Branch ${branch} is already reserved by another fork`,
    identityTimeout: 'Fork session identity confirmation timed out; try again'
  }
}
