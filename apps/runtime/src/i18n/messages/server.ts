import type { CatalogShape } from '../catalog'

export const serverZhCN = {
  // index.ts — fork bootstrap and fork notifications
  sessionNotReady: (sessionId: string) => `会话 ${sessionId} 尚未准备完成`,
  branchReady: '分支已就绪',
  branchFailed: '分支创建失败',
  branchReadyBody: '新的分支会话已经可以继续工作',
  branchIncomplete: '分支创建未完成',
  runtimeShuttingDown: 'Runtime 正在关闭',

  // index.ts — deterministic startup failures and database recovery
  migrationHistoryMismatch: '工作区升级记录与当前版本不一致，原数据保持原样。',
  databaseSchemaUnsupported: '工作区数据来自较新的 Matou 版本，请更新应用后重新检查。',
  databaseNotReadOnly: '当前数据库不在只读恢复模式',
  noPendingRecovery: '当前没有待处理的数据库恢复操作',
  recoveryCycleChanged: '数据库恢复周期已更新，本次操作已停止',
  recoveryStillRequired: '重新检查后数据库仍需要恢复',

  // runtime-server.ts — terminal sessions
  noRetryableInput: '当前会话没有可重试的上一轮输入',
  forkProcessExited: (exitCode: number | string) => `Fork 会话进程已退出，代码：${exitCode}`,
  forkProcessStartFailed: (reason: string) => `Fork 会话进程启动失败：${reason}`,
  /** Written into the terminal after a fork session gave up. */
  forkIncompleteBanner: '[Fork 未完成，请检查上方原因后重试]',
  defaultShell: '系统默认 Shell',
  exitCode: (exitCode: number | string) => `退出代码 ${exitCode}`,
  signal: (signal: number | string) => `信号 ${signal}`,
  shellStartFailed: (executable: string, termination: string) =>
    `Shell 进程启动失败：${executable} 未产生可用输出并退出（${termination}）`,
  providerIdentityMismatch: 'AI 会话返回的上下文与待恢复会话不一致，请重试恢复',
  forkIdentityTimeout: 'Fork 会话身份确认超时，请重试',
  environmentRecovering: '该会话的运行目录正在恢复或需要重新定位，请先处理运行环境后继续',
  worktreeUnavailable: '该会话的 Worktree 当前不可用，请恢复、重新定位或切换到 Local 后继续'
}

export const serverEn: CatalogShape<typeof serverZhCN> = {
  sessionNotReady: (sessionId) => `Session ${sessionId} is not ready yet`,
  branchReady: 'Branch ready',
  branchFailed: 'Branch creation failed',
  branchReadyBody: 'The new branch session is ready to continue',
  branchIncomplete: 'Branch creation did not finish',
  runtimeShuttingDown: 'The runtime is shutting down',

  migrationHistoryMismatch: 'The workspace upgrade history does not match this version; your data was left untouched.',
  databaseSchemaUnsupported: 'This workspace comes from a newer version of Matou; update the app and check again.',
  databaseNotReadOnly: 'The database is not in read-only recovery mode',
  noPendingRecovery: 'There is no pending database recovery operation',
  recoveryCycleChanged: 'The database recovery cycle changed; this operation was stopped',
  recoveryStillRequired: 'The database still needs recovery after the re-check',

  noRetryableInput: 'This session has no previous input to retry',
  forkProcessExited: (exitCode) => `The fork session process exited; code: ${exitCode}`,
  forkProcessStartFailed: (reason) => `The fork session process failed to start: ${reason}`,
  forkIncompleteBanner: '[Fork did not finish; check the reason above and retry]',
  defaultShell: 'the system default shell',
  exitCode: (exitCode) => `exit code ${exitCode}`,
  signal: (signal) => `signal ${signal}`,
  shellStartFailed: (executable, termination) =>
    `Shell process failed to start: ${executable} produced no usable output and exited (${termination})`,
  providerIdentityMismatch:
    'The context returned by the AI session does not match the session being restored; try restoring again',
  forkIdentityTimeout: 'Fork session identity confirmation timed out; try again',
  environmentRecovering:
    'The working directory of this session is being restored or needs relocating; sort out the environment before continuing',
  worktreeUnavailable:
    'The worktree of this session is unavailable; restore it, relocate it, or switch to Local before continuing'
}
