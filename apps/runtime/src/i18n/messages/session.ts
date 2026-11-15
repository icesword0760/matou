import type { CatalogShape } from '../catalog'

export const sessionZhCN = {
  // claude-session-catalog.ts
  claudeSessionNotInWorkspace: 'Claude Code 会话不存在或不属于当前工作空间',
  claudeSessionIdInvalid: 'Claude Code 会话标识格式错误',
  /** Default transcript title, read when a Claude Code session is catalogued. */
  untitledClaudeSession: '未命名 Claude 会话',

  // antigravity-session-catalog.ts
  antigravitySessionNotInWorkspace: 'Antigravity 会话不存在或不属于当前工作空间',
  antigravitySessionIdInvalid: 'Antigravity 会话标识格式错误',
  untitledAntigravitySession: '未命名 Antigravity 会话',

  codexSessionNotInWorkspace: 'Codex 会话不存在或不属于当前工作目录',
  codexSessionIdInvalid: 'Codex 会话标识必须是完整 UUID',
  codexSessionCorrupt: 'Codex 会话文件已损坏，无法读取',
  codexSessionPermissionDenied: '没有权限读取 Codex 会话目录或文件，请检查访问权限后重试',

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

  antigravitySessionNotInWorkspace:
    'That Antigravity session does not exist, or it does not belong to this workspace',
  antigravitySessionIdInvalid: 'The Antigravity session id has an invalid format',
  untitledAntigravitySession: 'Untitled Antigravity session',

  codexSessionNotInWorkspace: 'That Codex session does not exist or does not belong to this working directory',
  codexSessionIdInvalid: 'A Codex session id must be a full UUID',
  codexSessionCorrupt: 'The Codex session file is damaged and cannot be read',
  codexSessionPermissionDenied: 'Cannot read the Codex session directory or file. Check access permissions and retry',

  hookStatusMessage: 'Confirming the session',
  teammateFinished: 'A teammate finished the current task',

  forkStartFailed: 'The fork session failed to start'
}
