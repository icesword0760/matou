export type ClaudeSessionPermissionMode =
  | 'default'
  | 'auto'
  | 'acceptEdits'
  | 'plan'
  | 'bypassPermissions'

export type ClaudeSessionEventKind = 'user' | 'assistant' | 'tool' | 'system'

export type ClaudeSessionAvailability =
  | 'available'
  | 'loaded-here'
  | 'loaded-elsewhere'

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
  availability: ClaudeSessionAvailability
  loadedSessionId?: string
  loadedSessionTitle?: string
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
  page: ClaudeSessionEventPageInfo
}

export interface ClaudeSessionEventPageInfo {
  startEventIndex: number
  endEventIndex: number
  total: number
  hasEarlier: boolean
  hasLater: boolean
}

export interface ClaudeSessionListResult {
  sessions: ClaudeSessionSummary[]
  total: number
  offset: number
  limit: number
  nextOffset: number
  hasMore: boolean
}

export interface ClaudeSessionSearchResult {
  query: string
  hits: ClaudeSessionSearchHit[]
  total: number
  offset: number
  limit: number
  nextOffset: number
  hasMore: boolean
}

export interface ClaudeSessionLoadResult {
  sessionId: string
  providerSessionId: string
  permissionMode: ClaudeSessionPermissionMode
}
