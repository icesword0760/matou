import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { readFile } from 'node:fs/promises'
import { homedir, userInfo } from 'node:os'
import { join } from 'node:path'
import { createHash } from 'node:crypto'
import type { AgentModelPreferences, ProviderConfigView, ReasoningEffort } from '@matou/contracts'
import type { ProviderConfigStore } from '../provider-config/provider-config-store'
import type { SubscriptionAccounts } from '../model-router/subscription-accounts'
import type { RouterLease } from '../model-router/manager'
import { nativeClaudeModels } from './native-model-catalog'
import type { GatewayOverride } from './claude-request-gateway'

const exec = promisify(execFile)
export interface ClaudeModelRoutingOptions {
  preferences: AgentModelPreferences
  provider?: ProviderConfigView
  store: ProviderConfigStore
  accounts?: SubscriptionAccounts
}
/** Credentials and router leases stay entirely in the runtime process. */
export function claudeModelRouting(options: ClaudeModelRoutingOptions, env: NodeJS.ProcessEnv) {
  let target: Promise<{ baseUrl: string; headers: Record<string, string>; model: string; maxOutputTokens?: number }> | undefined
  let lease: RouterLease | undefined
  let closed = false
  async function prepareTarget() {
    const compact = options.preferences.compaction
    const selected = (await options.store.snapshot()).providers['claude-code'].find(p => p.id === compact.providerId)
    if (!selected || !compact.model) throw new Error('Compaction provider or model is unavailable')
    if (selected.subscription) {
      if (!options.accounts) throw new Error('Subscription service unavailable')
      const prepared = await options.accounts.prepareLaunch(selected.id, 'compaction')
      if (closed) { prepared.lease.release(); throw new Error('Session closed') }
      lease = prepared.lease
      return { baseUrl: lease.connection.baseUrl, headers: { authorization: `Bearer ${lease.connection.clientKey}` },
        model: `${prepared.input.accountPrefix}/${compact.model.replace(/\[1m\]$/i, '')}`,
        ...(selected.subscription.modelMetadata.find(m => m.id === compact.model)?.maxOutputTokens ? { maxOutputTokens: selected.subscription.modelMetadata.find(m => m.id === compact.model)!.maxOutputTokens! } : {}) }
    }
    const selection = await options.store.launchSelection('claude-code', selected.id)
    const key = selection.env.ANTHROPIC_API_KEY ?? selection.env.ANTHROPIC_AUTH_TOKEN
    const baseUrl = selection.env.ANTHROPIC_BASE_URL
    if (!baseUrl) throw new Error('Compaction provider endpoint is unavailable')
    const official = new URL(baseUrl).origin === 'https://api.anthropic.com'
    if (!key && !official) throw new Error('Compaction provider requires an API key')
    const headers: Record<string, string> = key ? { 'x-api-key': key } : {}
    const model = official ? (await nativeClaudeModels()).find(m => m.id === compact.model)?.resolvedModel ?? compact.model : compact.model
    return { baseUrl, headers, model }
  }
  return {
    async resolveOverride(input: { compaction: boolean; model: string }): Promise<GatewayOverride | undefined> {
      const compact = options.preferences.compaction
      if (input.compaction && compact.mode === 'custom') {
        // Read active official credentials for each compaction; router leases are reused per session.
        if (!target) target = prepareTarget().catch(error => { target = undefined; throw error })
        const selected = await target
        const official = new URL(selected.baseUrl).origin === 'https://api.anthropic.com' && !selected.headers['x-api-key']
        const effort = compact.effort === 'follow' ? roleEffort(options.provider, input.model) : compact.effort
        return { ...selected, ...(official ? { headers: await officialHeaders(env) } : {}),
          ...(!effort || effort === 'follow' ? {} : { effort, thinking: 'adaptive' as const }) }
      }
      const effort = roleEffort(options.provider, input.model)
      const apiRole = /^matou-api\/(opus|sonnet|haiku)$/.exec(input.model)?.[1] as 'opus' | 'sonnet' | 'haiku' | undefined
      const model = apiRole && !options.provider?.subscription ? options.provider?.modelMapping?.[apiRole] : undefined
      return model || effort && effort !== 'follow' ? { ...(model ? { model } : {}), ...(effort && effort !== 'follow' ? { effort } : {}) } : undefined
    },
    close() { closed = true; lease?.release(); lease = undefined }
  }
}
export function roleEffort(provider: ProviderConfigView | undefined, model: string): ReasoningEffort | undefined {
  if (!provider) return undefined
  const roles = ['opus', 'sonnet', 'haiku'] as const
  const normalized = model.replace(/\[1m\]$/i, '')
  const mapped = roles.find(r => provider.modelMapping?.[r]?.replace(/\[1m\]$/i, '') === normalized)
  const inferred = roles.find(r => new RegExp(`(?:matou-|claude-)${r}(?:[-/\\[]|$)`, 'i').test(model))
  const virtualRole = /^matou-api\/(opus|sonnet|haiku)$/.exec(model)?.[1] as 'opus' | 'sonnet' | 'haiku' | undefined
  const role = virtualRole ?? (provider.subscription ? inferred ?? mapped : mapped ?? inferred)
  return (role ? provider.effortMapping?.[role] : undefined) ?? provider.reasoningEffort
}
async function officialHeaders(env: NodeJS.ProcessEnv): Promise<Record<string, string>> {
  // Only the configured official account is eligible; never reuse the source provider's API key.
  const configuredToken = env.CLAUDE_CODE_OAUTH_TOKEN || process.env.CLAUDE_CODE_OAUTH_TOKEN
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

export function modelRoutingEnabled(preferences: AgentModelPreferences, provider?: ProviderConfigView): boolean {
  return preferences.compaction.mode === 'custom' || Object.values(provider?.effortMapping ?? {}).some(value => value !== 'follow') || Boolean(provider?.reasoningEffort && provider.reasoningEffort !== 'follow')
}
export function restoredProviderModel(bound: string | undefined, selected: string | undefined, provider: ProviderConfigView | undefined, preferences: AgentModelPreferences): string | undefined {
  const apiRole = /^matou-api\/(opus|sonnet|haiku)$/.exec(bound ?? '')?.[1] as 'opus' | 'sonnet' | 'haiku' | undefined
  if (apiRole) {
    const mapped = provider?.modelMapping?.[apiRole]
    if (provider?.subscription || !mapped) return selected
    return modelRoutingEnabled(preferences, provider) ? bound : mapped
  }
  if (!provider?.subscription && /^matou-(chatgpt|antigravity)\//.test(bound ?? '')) return selected
  return bound ?? selected
}
