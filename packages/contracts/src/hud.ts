export type HudPermissionMode = 'default' | 'auto' | 'acceptEdits' | 'plan' | 'bypassPermissions'
export type HudModelStrategy = 'opusplan' | 'claude-opus-4-6' | 'claude-sonnet-4-6'
export interface HudUsageWindow { label: string; percent: number; resetsAt?: number }
export interface HudToolCount { name: string; count: number }
export interface HudToolActivity { name: string; target?: string; status: 'running' | 'completed' | 'error' }
export interface HudConfigCounts {
  instructionFiles: number
  mcpServers: number
  hooks: number
  mcpServerNames?: string[]
  hookNames?: string[]
}

export interface SessionHudWire {
  sessionId: string
  mode: 'shell' | 'agent'
  shell?: string
  cwd?: string
  gitBranch?: string
  gitDirty?: boolean
  startedAt: number
  permissionMode?: HudPermissionMode
  modelStrategy?: HudModelStrategy
  model?: string
  sessionName?: string
  contextPercent?: number
  contextWindowSize?: number
  taskStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  teamRole?: string
  teamStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  subagentCount?: number
  subagents?: string[]
  runningTools?: Array<{ name: string; target?: string }>
  toolCounts?: HudToolCount[]
  lastTool?: HudToolActivity
  usageWindows?: HudUsageWindow[]
  configCounts?: HudConfigCounts
  mcpErrors?: string[]
  todos?: Array<{ content: string; status: 'pending' | 'in_progress' | 'completed' }>
  resumable?: boolean
}
