export type ContextWindowMode = 'default' | '512k' | 'maximum'
export type CompactionEffort = 'follow' | 'low' | 'medium' | 'high' | 'xhigh' | 'max'
export type ClaudeModelRole = 'opus' | 'sonnet' | 'haiku'
export interface ClaudeModelTarget {
  providerId: string
  model: string
  effort?: CompactionEffort
}
export type ClaudeRoleAssignments = Partial<Record<ClaudeModelRole, ClaudeModelTarget>>
export interface ClaudeAssignmentsPatch {
  assignments: ClaudeRoleAssignments
  /** Absent preserves the search target for legacy clients; null clears it. */
  webSearch?: ClaudeModelTarget | null
}
export interface AgentModelPreferences {
  claudeWebSearch?: ClaudeModelTarget
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
