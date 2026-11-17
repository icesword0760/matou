import type { EgressFamily } from '@matou/contracts'
import type { ResolvedClaudeRoleSnapshot } from './claude-role-assignments'
import { claudeRequestRole, type PreparedClaudeRoleLaunch } from './claude-role-launch'
import { officialHeaders } from './claude-official-account'
import type { AgentModelPreferences, ProviderConfigView, ReasoningEffort } from '@matou/contracts'
import type { ProviderConfigStore } from '../provider-config/provider-config-store'
import type { SubscriptionAccounts } from '../model-router/subscription-accounts'
import { SubscriptionAccountBindingError } from '../model-router/subscription-account-binding'
import type { RouterLease } from '../model-router/manager'
import { nativeClaudeModels } from './native-model-catalog'
import { GatewayRoutingError, type GatewayOverride } from './claude-request-gateway'
import { isDeepSeekMessagesDestination } from './deepseek-inline-tools'

export interface ClaudeModelRoutingOptions {
  preferences: AgentModelPreferences
  provider?: ProviderConfigView
  store: ProviderConfigStore
  accounts?: SubscriptionAccounts
  roleSnapshot?: ResolvedClaudeRoleSnapshot
  sourceConnection?: PreparedClaudeRoleLaunch['sourceConnection']
  releaseTargets?: () => void
  isClosed?: () => boolean
  assertTargetAllowed?: (family?: EgressFamily) => void | Promise<void>
}
/** Credentials and router leases stay entirely in the runtime process. */
export function claudeModelRouting(options: ClaudeModelRoutingOptions, env: NodeJS.ProcessEnv) {
  let target: Promise<{ baseUrl: string; headers: Record<string, string>; model: string; maxOutputTokens?: number; family?: EgressFamily }> | undefined
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
      return { ...(selected.subscription.provider === 'codex' ? {family:'chatgpt-subscription' as const} : {}), baseUrl: lease.connection.baseUrl, headers: { authorization: `Bearer ${lease.connection.clientKey}` },
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
    return { baseUrl, headers, model, ...(official ? {family: 'anthropic' as const} : {}) }
  }
  return {
    async resolveOverride(input: { compaction: boolean; model: string; kind?: 'message' | 'count_tokens' }): Promise<GatewayOverride | undefined> {
      if (options.roleSnapshot) {
        const snapshot = options.roleSnapshot
        const role = claudeRequestRole(input.model, snapshot.nativeRoleModels)
        if (closed) throw new GatewayRoutingError('Session model routing is closed')
        if (!role && /^matou-role\//.test(input.model)) throw new GatewayRoutingError('Unknown Claude model role')
        const custom = input.kind !== 'count_tokens' && input.compaction && snapshot.preferences.compaction.mode === 'custom'
        const selected = custom ? snapshot.compactionTarget : role ? snapshot.roles[role] : undefined
        if (!selected) {
          if (custom) throw new GatewayRoutingError('Custom compaction target is unavailable')
          const source = options.sourceConnection
          if (!source) throw new GatewayRoutingError('Original supplier source connection was not bound at startup; choose a configured role')
          try {
            if (source.kind === 'subscription') source.assertActive()
            const headers = source.kind === 'oauth' ? (await source.readHeaders()).headers : { ...source.headers }
            const family = source.kind === 'oauth' || new URL(source.baseUrl).hostname === 'api.anthropic.com' ? 'anthropic' as const : source.kind === 'subscription' && options.provider?.subscription?.provider === 'codex' ? 'chatgpt-subscription' as const : undefined
            return { baseUrl: source.baseUrl, headers, model: input.model, ...(source.kind === 'subscription' && source.targetSignal ? { targetSignal: source.targetSignal } : {}), ...(family ? { family } : {}) }
          } catch (error) { throw new GatewayRoutingError(error instanceof SubscriptionAccountBindingError ? error.message : 'Original supplier account identity is unavailable') }
        }
        const label = `${role ?? 'compaction'} · ${selected.providerName ?? selected.providerId}`
        try {
          selected.assertActive?.()
          const headers = selected.resolveHeaders ? await selected.resolveHeaders() : { ...selected.headers }
          const base = { baseUrl: selected.baseUrl, headers, model: selected.model, ...(selected.targetSignal ? { targetSignal: selected.targetSignal } : {}), ...(selected.family ? { family: selected.family } : {}) }
          if (input.kind === 'count_tokens') return base
          const compactEffort = snapshot.preferences.compaction.effort
          const effort = custom && compactEffort !== 'follow' ? compactEffort : custom && role ? snapshot.roles[role].effort : selected.effort
          if (effort && effort !== 'follow' && selected.metadata?.supportedEfforts && !selected.metadata.supportedEfforts.includes(effort)) throw new Error('Unsupported effort')
          return { ...base,
            effort: effort && effort !== 'follow' ? effort : 'omit' as const,
            thinking: effort && effort !== 'follow' && /^claude-/.test(selected.model) ? 'adaptive' as const : 'omit' as const,
            ...(selected.metadata?.maxOutputTokens ? { maxOutputTokens: selected.metadata.maxOutputTokens } : {}) }
        } catch (error) { throw new GatewayRoutingError(error instanceof SubscriptionAccountBindingError ? error.message : `${label}: model target or reasoning effort is unavailable`) }
      }
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
    close() { if (closed) return; closed = true; lease?.release(); lease = undefined; options.releaseTargets?.() }
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

export function modelRoutingEnabled(preferences: AgentModelPreferences, provider?: ProviderConfigView): boolean {
  let deepSeek = false
  if (provider?.endpoint) {
    try {
      const destination = new URL(provider.endpoint)
      destination.pathname = destination.pathname.replace(/\/+$/, '') + '/v1/messages'
      deepSeek = isDeepSeekMessagesDestination(destination)
    } catch { /* Invalid provider endpoints are handled during launch. */ }
  }
  return Object.keys(preferences.claudeRoleAssignments ?? {}).length > 0 || deepSeek || preferences.compaction.mode === 'custom' || Object.values(provider?.effortMapping ?? {}).some(value => value !== 'follow') || Boolean(provider?.reasoningEffort && provider.reasoningEffort !== 'follow')
}
export function restoredProviderModel(bound: string | undefined, selected: string | undefined, provider: ProviderConfigView | undefined, preferences: AgentModelPreferences): string | undefined {
  if (/^matou-role\//.test(bound ?? '')) return Object.keys(preferences.claudeRoleAssignments ?? {}).length ? bound : selected
  const apiRole = /^matou-api\/(opus|sonnet|haiku)$/.exec(bound ?? '')?.[1] as 'opus' | 'sonnet' | 'haiku' | undefined
  if (apiRole) {
    const mapped = provider?.modelMapping?.[apiRole]
    if (provider?.subscription || !mapped) return selected
    return modelRoutingEnabled(preferences, provider) ? bound : mapped
  }
  if (!provider?.subscription && /^matou-(chatgpt|antigravity)\//.test(bound ?? '')) return selected
  return bound ?? selected
}
