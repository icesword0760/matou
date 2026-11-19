import type { CatalogShape } from '../catalog'

export const gitZhCN = {
  // git-workspace-service.ts
  commitMessageRequired: '请输入提交信息',
  detachedHead: '当前处于 detached HEAD，请先创建分支',
  noRemote: '仓库尚未配置远端',
  worktreeStillLinked: (sessionCount: number) => `该 Worktree 仍有关联会话（${sessionCount}）`,
  branchNameRequired: '请输入分支名称',
  branchNameInvalid: '分支名称格式不正确',
  branchMissing: (branch: string) => `分支「${branch}」已不存在`,
  branchCheckedOut: (path: string) => `目标分支已在 ${path} 检出`,
  notARepository: '当前目录不是 Git 仓库',
  environmentNotLocal: '当前会话无法从本地目录创建独立 Worktree',
  worktreeRestartFailed: '会话未能在独立 Worktree 中重新启动',
  operationFailed: 'Git 操作失败'
}

export const gitEn: CatalogShape<typeof gitZhCN> = {
  commitMessageRequired: 'Enter a commit message',
  detachedHead: 'HEAD is detached; create a branch first',
  noRemote: 'The repository has no remote configured yet',
  worktreeStillLinked: (sessionCount) => sessionCount === 1
    ? 'This worktree still has 1 session attached'
    : `This worktree still has ${sessionCount} sessions attached`,
  branchNameRequired: 'Enter a branch name',
  branchNameInvalid: 'The branch name format is invalid',
  branchMissing: (branch) => `Branch “${branch}” no longer exists`,
  branchCheckedOut: (path) => `The target branch is already checked out at ${path}`,
  notARepository: 'The current directory is not a Git repository',
  environmentNotLocal: 'This session cannot create a separate worktree from its current environment',
  worktreeRestartFailed: 'The session couldn’t restart in the separate worktree',
  operationFailed: 'The Git operation failed'
}
