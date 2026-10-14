/** Main/runtime-only control messages; never contain account credentials. */
export interface ClaudeAccountSession {
  sessionId: string
  runId: string
  busy: boolean
  resumable: boolean
  defaultLogin: boolean
  /** Fresh official CLI response, never an HTTP query from account management. */
  usage?: { accountId: string; observedAt: number; windows: Array<{ label: string; percent: number; resetsAt?: number }> }
}
export type ClaudeAccountRuntimeAction = 'snapshot' | 'usage-reset' | 'hold' | 'release' | 'resume' | 'restart'
export interface ClaudeAccountRuntimeRequest {
  type: 'runtime.claude-accounts-request'
  requestId: string
  action: ClaudeAccountRuntimeAction
  sessionId?: string
  runId?: string
}
export interface ClaudeAccountRuntimeResult {
  type: 'runtime.claude-accounts-result'
  requestId: string
  sessions?: ClaudeAccountSession[]
  error?: string
}
