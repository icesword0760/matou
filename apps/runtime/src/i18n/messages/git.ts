import type { CatalogShape } from '../catalog'

export const gitZhCN = {
  // git-workspace-service.ts
  commitMessageRequired: '请输入提交信息',
  detachedHead: '当前处于 detached HEAD，请先创建分支',
  noRemote: '仓库尚未配置远端',
  worktreeStillLinked: (sessionCount: number) => `该 Worktree 仍有关联会话（${sessionCount}）`,
  branchNameRequired: '请输入分支名称',
  branchNameInvalid: '分支名称格式不正确',
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
  operationFailed: 'The Git operation failed'
}
