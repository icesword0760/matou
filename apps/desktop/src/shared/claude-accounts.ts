import type { ClaudeAccountSession } from '@matou/contracts'

export interface AccountUsage {
  checkedAt: number
  stale: boolean
  fiveHour?: { used: number; resetsAt: string }
  weekly?: { used: number; resetsAt: string }
}
export interface ClaudeAccountView {
  id: string
  name: string
  email: string
  plan: string
  needsLogin: boolean
  usageIssue?: 'unavailable' | 'rate-limited' | 'unsupported'
  usageRetryAt?: number
  usage?: AccountUsage
}
export type AccountError = 'network' | 'login-required' | 'login-timeout' | 'login-failed' | 'login-code-invalid' | 'login-ended' |
  'browser-missing' | 'browser-closed' | 'storage' | 'keychain' | 'cli-missing' | 'busy' | 'runtime' | 'identity' | 'invalid' |
  'engine-missing' | 'engine-version' | 'engine-error' | 'switch-failed' | 'conflict' | 'unsupported' | 'recovery-required' | 'resume-failed'
export interface ClaudeAccountsState {
  usageRefreshing?: boolean
  usageRefreshFailed?: boolean
  revision: number
  engineVersion?: string
  accounts: ClaudeAccountView[]
  currentId?: string
  detected?: ClaudeAccountView
  rememberScope: boolean
  sessions: ClaudeAccountSession[]
  operation: 'idle' | 'loading' | 'login' | 'switching' | 'resuming'
  reauthId?: string
  renewedIds?: string[]
  appliedReauthId?: string
  loginAttemptId?: string
  loginCodeSubmitted?: boolean
  draft?: ClaudeAccountView
  queuedId?: string
  switchedAt?: number
  pendingSessionIds: string[]
  error?: AccountError
}
export type ClaudeAccountCommand =
  | { type: 'usage-visibility'; visible: boolean; recheck?: boolean }
  | { type: 'inspect' | 'refresh-sessions' | 'refresh-usage' | 'cancel-login' | 'cancel-switch' | 'dismiss-error' | 'recover' }
  | { type: 'login'; replaceId?: string }
  | { type: 'submit-login-code'; attemptId: string; code: string }
  | { type: 'save-detected' | 'save-draft'; name: string }
  | { type: 'rename'; id: string; name: string }
  | { type: 'remove' | 'apply-reauth'; id: string }
  | { type: 'switch'; id: string; rememberScope: boolean; waitForIdle: boolean }
  | { type: 'resume'; sessionId: string; runId: string }

export function isAccountCommand(value: unknown): value is ClaudeAccountCommand {
  if (!value || typeof value !== 'object') return false
  const v = value as Record<string, unknown>
  const id = (key: string) => typeof v[key] === 'string' && (v[key] as string).length > 0 && (v[key] as string).length <= 200
  const name = () => typeof v.name === 'string' && v.name.trim().length > 0 && [...v.name.trim()].length <= 12 && !/[\x00-\x1f\x7f]/.test(v.name)
  switch (v.type) {
    case 'usage-visibility': return typeof v.visible === 'boolean' && (v.recheck === undefined || typeof v.recheck === 'boolean')
    case 'inspect': case 'refresh-sessions': case 'refresh-usage': case 'cancel-login': case 'cancel-switch': case 'dismiss-error': case 'recover': return true
    case 'submit-login-code': return id('attemptId') && typeof v.code === 'string' && v.code.length <= 4096
    case 'login': return v.replaceId === undefined || id('replaceId')
    case 'save-detected': case 'save-draft': return name()
    case 'rename': return id('id') && name()
    case 'remove': case 'apply-reauth': return id('id')
    case 'switch': return id('id') && typeof v.rememberScope === 'boolean' && typeof v.waitForIdle === 'boolean'
    case 'resume': return id('sessionId') && id('runId')
    default: return false
  }
}

/** Accept only the official one-time code#state, never URLs or terminal commands. */
export function validLoginCode(code: string): boolean {
  return code.length <= 4096 && /^[A-Za-z0-9_~.\-]{10,}#[A-Za-z0-9_~.\-]{10,}$/.test(code.trim())
}
