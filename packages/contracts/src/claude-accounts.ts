/** Main/runtime-only control messages; never contain account credentials. */
export interface ClaudeAccountSession {
  sessionId: string
  runId: string
  busy: boolean
  resumable: boolean
  defaultLogin: boolean
}
export type ClaudeAccountRuntimeAction = 'snapshot' | 'hold' | 'release' | 'resume' | 'restart'
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
