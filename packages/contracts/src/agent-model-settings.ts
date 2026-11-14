export type ContextWindowMode = 'default' | '512k' | 'maximum'
export type CompactionEffort = 'follow' | 'low' | 'medium' | 'high' | 'xhigh' | 'max'
export interface AgentModelPreferences {
  contextWindows: Record<string, ContextWindowMode>
  compaction: { mode: 'follow' | 'custom'; providerId?: string; model?: string; effort: CompactionEffort }
}
export interface ModelContextChoice {
  id: string
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
