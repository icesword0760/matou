import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { readFile } from 'node:fs/promises'
import { homedir, userInfo } from 'node:os'
import { join } from 'node:path'
import { createHash } from 'node:crypto'
import type { ClaudeTargetConnection } from './claude-role-assignments'
const exec = promisify(execFile)
export async function officialHeaders(env: NodeJS.ProcessEnv, inheritToken = true): Promise<Record<string, string>> {
  // Only the configured official account is eligible; never reuse the source provider's API key.
  const configuredToken = env.CLAUDE_CODE_OAUTH_TOKEN || (inheritToken ? process.env.CLAUDE_CODE_OAUTH_TOKEN : undefined)
  if (configuredToken) return { authorization: `Bearer ${configuredToken}`, 'anthropic-beta': 'oauth-2025-04-20' }
  let raw: string | undefined
  const root = env.CLAUDE_CONFIG_DIR || join(homedir(), '.claude')
  if (process.platform === 'darwin') {
    const secureRoot = env.CLAUDE_SECURESTORAGE_CONFIG_DIR || env.CLAUDE_CONFIG_DIR
    const suffix = secureRoot ? '-' + createHash('sha256').update(secureRoot).digest('hex').slice(0, 8) : ''
    try { raw = (await exec('/usr/bin/security', ['find-generic-password', '-a', env.USER || userInfo().username, '-s', 'Claude Code-credentials' + suffix, '-w'], { timeout: 5000, maxBuffer: 1024 * 1024 })).stdout } catch { /* Native file fallback below. */ }
  }
  try { raw ??= await readFile(join(root, '.credentials.json'), 'utf8') } catch { /* Report a safe account error. */ }
  try {
    const oauth = JSON.parse(raw || '{}').claudeAiOauth
    if (typeof oauth?.accessToken === 'string' && (!oauth.expiresAt || oauth.expiresAt > Date.now())) return { authorization: `Bearer ${oauth.accessToken}`, 'anthropic-beta': 'oauth-2025-04-20' }
  } catch { /* No raw credential diagnostics escape. */ }
  throw new Error('Sign in to the official Claude account before using it for compaction')
}

/** The native account configuration stores stable identity separately from tokens. */
export async function captureOfficialClaudeAccount(env: NodeJS.ProcessEnv): Promise<Extract<ClaudeTargetConnection, { kind: 'oauth' }>> {
  const captured = { ...env }
  const config = captured.CLAUDE_CONFIG_DIR ? join(captured.CLAUDE_CONFIG_DIR, '.claude.json') : join(homedir(), '.claude.json')
  captured.CLAUDE_CONFIG_DIR ||= join(homedir(), '.claude')
  const readIdentity = async (): Promise<string> => {
    let account: { accountUuid?: unknown; emailAddress?: unknown; organizationUuid?: unknown } | undefined
    try { account = JSON.parse(await readFile(config, 'utf8')).oauthAccount } catch { /* Safe error below. */ }
    const id = typeof account?.accountUuid === 'string' && account.accountUuid ? account.accountUuid : account?.emailAddress
    if (typeof id !== 'string' || !id) throw new Error('Official Claude account identity is unavailable in local account configuration')
    return JSON.stringify([id, account?.organizationUuid ?? ''])
  }
  const identity = await readIdentity()
  return { kind: 'oauth', baseUrl: 'https://api.anthropic.com', identity, async readHeaders() {
    if (await readIdentity() !== identity) throw new Error('Official Claude account identity changed')
    const headers = await officialHeaders(captured, false)
    if (await readIdentity() !== identity) throw new Error('Official Claude account identity changed')
    return { identity, headers }
  } }
}
