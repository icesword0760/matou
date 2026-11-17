export type ContextWindowMode = 'default' | '512k' | 'maximum'
export type CompactionEffort = 'follow' | 'low' | 'medium' | 'high' | 'xhigh' | 'max'
export type ClaudeModelRole = 'opus' | 'sonnet' | 'haiku'
export type ClaudeRoleAssignments = Partial<Record<ClaudeModelRole, { providerId: string; model: string }>>
export interface AgentModelPreferences {
  claudeRoleAssignments?: ClaudeRoleAssignments
  contextWindows: Record<string, ContextWindowMode>
  compaction: { mode: 'follow' | 'custom'; providerId?: string; model?: string; effort: CompactionEffort }
}
export interface ModelContextChoice {
  id: string
  resolvedModel?: string
  name: string
  defaultTokens?: number
  maximumTokens?: number
  fixedTokens: number
  supportedEfforts?: Exclude<CompactionEffort, 'follow'>[]
}
export interface AgentModelProvider {
  id: string
  name: string
  cli: 'claude-code' | 'codex'
  compactionAvailable: boolean
  models: ModelContextChoice[]
}
export interface AgentModelSettingsSnapshot {
  preferences: AgentModelPreferences
  providers: AgentModelProvider[]
}
