export type HudPermissionMode = 'default' | 'acceptEdits' | 'plan' | 'bypassPermissions'
export type HudModelStrategy = 'opusplan' | 'claude-opus-4-6' | 'claude-sonnet-4-6'

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
  contextPercent?: number
  taskStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  teamRole?: string
  teamStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  subagentCount?: number
  runningTools?: Array<{ name: string; target?: string }>
  todos?: Array<{ content: string; status: 'pending' | 'in_progress' | 'completed' }>
  resumable?: boolean
}
