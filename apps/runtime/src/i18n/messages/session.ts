import type { CatalogShape } from '../catalog'

export const sessionZhCN = {
  // claude-session-catalog.ts
  claudeSessionNotInWorkspace: 'Claude Code 会话不存在或不属于当前工作空间',
  claudeSessionIdInvalid: 'Claude Code 会话标识格式错误',
  /** Default transcript title, read when a Claude Code session is catalogued. */
  untitledClaudeSession: '未命名 Claude 会话',
  modelRouterRecoveryFailed: '本地模型路由未运行，自动恢复失败。请重启码头后重试。',

  // antigravity-session-catalog.ts
  antigravitySessionNotInWorkspace: 'Antigravity 会话不存在或不属于当前工作空间',
  antigravitySessionIdInvalid: 'Antigravity 会话标识格式错误',
  untitledAntigravitySession: '未命名 Antigravity 会话',

  codexSessionNotInWorkspace: 'Codex 会话不存在或不属于当前工作目录',
  codexSessionIdInvalid: 'Codex 会话标识必须是完整 UUID',
  codexSessionCorrupt: 'Codex 会话文件已损坏，无法读取',
  codexHistoryMissing: '祖先历史不完整：祖先文件缺失或不可读，仅显示当前会话。',
  codexHistoryCycle: '祖先历史存在循环，已停止回溯并显示可读取的历史。',
  codexHistoryDepthLimit: '祖先历史超过 5 层，仅显示已读取的历史。',
  codexSessionPermissionDenied: '没有权限读取 Codex 会话目录或文件，请检查访问权限后重试',

  dshSessionNotInWorkspace: 'DSH 会话不存在或不属于当前工作目录',
  dshSessionIdInvalid: 'DSH 会话标识格式错误',
  dshSessionCorrupt: 'DSH 会话文件已损坏或包含不支持的记录，无法预览',
  dshSessionReadLimit: 'DSH 会话超过预览读取上限，原文件未被修改',
  dshSessionUnsupportedEvent: 'DSH 会话包含暂不支持的必要事件，已停止预览',
  dshPreviewShortened: '预览内容已截短',

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
  modelRouterRecoveryFailed: 'The local model router is not running and could not be restarted. Restart Matou and try again.',

  antigravitySessionNotInWorkspace:
    'That Antigravity session does not exist, or it does not belong to this workspace',
  antigravitySessionIdInvalid: 'The Antigravity session id has an invalid format',
  untitledAntigravitySession: 'Untitled Antigravity session',

  codexSessionNotInWorkspace: 'That Codex session does not exist or does not belong to this working directory',
  codexSessionIdInvalid: 'A Codex session id must be a full UUID',
  codexSessionCorrupt: 'The Codex session file is damaged and cannot be read',
  codexHistoryMissing: 'Ancestor history is incomplete: an ancestor file is missing or unreadable. Showing this session only.',
  codexHistoryCycle: 'Ancestor history contains a cycle. Showing the readable history without following the cycle.',
  codexHistoryDepthLimit: 'Ancestor history exceeds 5 levels. Showing the history read within that limit.',
  codexSessionPermissionDenied: 'Cannot read the Codex session directory or file. Check access permissions and retry',

  dshSessionNotInWorkspace: 'That DSH session does not exist or does not belong to this working directory',
  dshSessionIdInvalid: 'The DSH session id has an invalid format',
  dshSessionCorrupt: 'The DSH session file is damaged or contains unsupported records and cannot be previewed',
  dshSessionReadLimit: 'The DSH session exceeds the preview read limit; the original file was not changed',
  dshSessionUnsupportedEvent: 'The DSH session contains an unsupported required event; preview was stopped',
  dshPreviewShortened: 'Preview shortened',

  hookStatusMessage: 'Confirming the session',
  teammateFinished: 'A teammate finished the current task',

  forkStartFailed: 'The fork session failed to start'
}
