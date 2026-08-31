export type ClaudeSessionPermissionMode =
  | 'default'
  | 'auto'
  | 'acceptEdits'
  | 'plan'
  | 'bypassPermissions'

export type ClaudeSessionEventKind = 'user' | 'assistant' | 'tool' | 'system'

export interface ClaudeSessionSearchHit {
  eventIndex: number
  kind: ClaudeSessionEventKind
  excerpt: string
}

export interface ClaudeSessionSummary {
  providerSessionId: string
  title: string
  cwd: string
  updatedAt: number
  model?: string
  permissionMode: ClaudeSessionPermissionMode
  eventCount: number
  matchCount: number
  hits: ClaudeSessionSearchHit[]
}

export interface ClaudeSessionPreviewEvent {
  index: number
  kind: ClaudeSessionEventKind
  role?: 'user' | 'assistant'
  timestamp?: number
  text: string
  toolName?: string
  matched: boolean
}

export interface ClaudeSessionDetail extends ClaudeSessionSummary {
  events: ClaudeSessionPreviewEvent[]
}

export interface ClaudeSessionListResult {
  sessions: ClaudeSessionSummary[]
  total: number
}

export interface ClaudeSessionLoadResult {
  sessionId: string
  providerSessionId: string
  permissionMode: ClaudeSessionPermissionMode
}
