import type { CatalogShape } from '../catalog'

export const sessionZhCN = {
  // claude-session-catalog.ts
  claudeSessionNotInWorkspace: 'Claude Code 会话不存在或不属于当前工作空间',
  claudeSessionIdInvalid: 'Claude Code 会话标识格式错误',
  /** Default transcript title, read when a Claude Code session is catalogued. */
  untitledClaudeSession: '未命名 Claude 会话',

  // provider-hook-server.ts
  hookStatusMessage: '正在确认会话',
  teammateFinished: '队友已完成当前任务',

  // session-fork-intent-repository.ts
  forkStartFailed: 'Fork 会话启动失败'
}

export const sessionEn: CatalogShape<typeof sessionZhCN> = {
  claudeSessionNotInWorkspace:
    'That Claude Code session does not exist, or it does not belong to this workspace',
  claudeSessionIdInvalid: 'The Claude Code session id has an invalid format',
  untitledClaudeSession: 'Untitled Claude session',

  hookStatusMessage: 'Confirming the session',
  teammateFinished: 'A teammate finished the current task',

  forkStartFailed: 'The fork session failed to start'
}
