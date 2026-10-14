import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import type { ClaudeAccountSession } from '@matou/contracts'

type Observation = NonNullable<ClaudeAccountSession['usage']>
interface Entry { accountId?: string | undefined; eligibleAfter: number; promptReady: boolean; duration: number; promptDuration: number; promptId?: string; oldPromptId?: string | undefined; usage?: Observation }
/** No credentials or HTTP. Only fresh responses from the official statusLine. */
export class AccountUsageTracker {
  private entries = new Map<string, Entry>()
  constructor(private readonly now = Date.now, private readonly identity = defaultAccountIdentity) {}
  spawn(id: string) { this.entries.set(id, { accountId: this.identity(), eligibleAfter: 0, promptReady: false, duration: 0, promptDuration: 0 }) }
  delete(id: string) { this.entries.delete(id) }
  invalidate() {
    for (const entry of this.entries.values()) {
      // macOS CLI caches Keychain reads for ~30s. A later statusLine alone
      // may still contain the previous turn's quota. Require a new prompt AND
      // an API response after the cache margin, not merely elapsed wall time.
      entry.accountId = this.identity()
      entry.eligibleAfter = this.now() + 35_000
      entry.promptReady = false; delete entry.usage
    }
  }
  observe(id: string, payload: Record<string, unknown>, windows?: Observation['windows']) {
    const entry = this.entries.get(id)
    if (!entry) return
    const accountId = this.identity()
    if (accountId !== entry.accountId) {
      entry.accountId = accountId; entry.eligibleAfter = this.now() + 35_000
      entry.promptReady = false; delete entry.usage
    }
    if (!accountId) return
    if (payload.hook_event_name === 'UserPromptSubmit') {
      entry.promptReady = this.now() >= entry.eligibleAfter
      entry.promptDuration = entry.duration; entry.oldPromptId = entry.promptId
      return
    }
    const cost = payload.cost as Record<string, unknown> | undefined
    const duration = typeof cost?.total_api_duration_ms === 'number' ? cost.total_api_duration_ms : undefined
    const prompt = typeof payload.prompt_id === 'string' ? payload.prompt_id : undefined
    if (duration !== undefined && Number.isFinite(duration)) {
      if (entry.promptReady && duration > entry.promptDuration && windows && (!prompt || prompt !== entry.oldPromptId)) {
        entry.usage = { accountId, observedAt: this.now(), windows }
        entry.promptDuration = duration
      }
      entry.duration = duration
    }
    if (prompt) entry.promptId = prompt
  }
  snapshot(id: string) { const usage = this.entries.get(id)?.usage; return usage ? structuredClone(usage) : undefined }
}

/** Local official identity only; no Keychain read or token in IPC. */
function defaultAccountIdentity(): string | undefined {
  try {
    const path = process.env.CLAUDE_CONFIG_DIR ? join(process.env.CLAUDE_CONFIG_DIR, '.claude.json') : join(homedir(), '.claude.json')
    const account = JSON.parse(readFileSync(path, 'utf8')).oauthAccount
    if (typeof account?.emailAddress !== 'string') return undefined
    // Match bridge.identity_id's stable JSON serialization, including separator.
    return createHash('sha256').update(`[${JSON.stringify(account.emailAddress)}, ${JSON.stringify(account.organizationUuid || '')}]`).digest('hex').slice(0, 32)
  } catch { return undefined }
}
