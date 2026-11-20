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
import type { SessionContextUsageSnapshot } from './deepseek-token-usage'
import { isGeminiModelId } from './gemini-sse-keepalive'

export interface ClaudeModelRoutingOptions {
  preferences: AgentModelPreferences
  provider?: ProviderConfigView
  store: ProviderConfigStore
  accounts?: SubscriptionAccounts
  roleSnapshot?: ResolvedClaudeRoleSnapshot
  sourceConnection?: PreparedClaudeRoleLaunch['sourceConnection']
  readContextUsage?: () => SessionContextUsageSnapshot | undefined
  releaseTargets?: () => void
  isClosed?: () => boolean
  assertTargetAllowed?: (family?: EgressFamily) => void | Promise<void>
}
/** Credentials and router leases stay entirely in the runtime process. */
export function claudeModelRouting(options: ClaudeModelRoutingOptions, env: NodeJS.ProcessEnv) {
  let target: ReturnType<typeof prepareTarget> | undefined
  let lease: RouterLease | undefined
  let closed = false
  const authentication = (label: string, provider = options.provider): GatewayOverride['authentication'] => {
    if (!provider || !provider.subscription && (provider.hasApiKey || provider.endpoint.replace(/\/$/, '') !== 'https://api.anthropic.com')) return undefined
    return { providerId: provider.id, providerName: provider.name, label }
  }
  async function prepareTarget() {
    const compact = options.preferences.compaction
    const selected = (await options.store.snapshot()).providers['claude-code'].find(p => p.id === compact.providerId)
    if (!selected || !compact.model) throw new Error('Compaction provider or model is unavailable')
    if (selected.subscription) {
      if (!options.accounts) throw new Error('Subscription service unavailable')
      const prepared = await options.accounts.prepareLaunch(selected.id, 'compaction')
      if (closed) { prepared.lease.release(); throw new Error('Session closed') }
      lease = prepared.lease
      const headers: Record<string, string> = { authorization: `Bearer ${lease.connection.clientKey}` }
      return { routerTarget: true as const, authentication: authentication('compaction', selected)!, ...(selected.subscription.provider === 'codex' ? {family:'chatgpt-subscription' as const} : {}), baseUrl: lease.connection.baseUrl, headers,
        model: `${prepared.input.accountPrefix}/${compact.model.replace(/\[1m\]$/i, '')}`,
        ...(isGeminiModelId(compact.model) ? { streamKeepAlive: true } : {}),
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
    return { ...(official && !key ? { authentication: authentication('compaction', selected)! } : {}), baseUrl, headers, model, ...(isGeminiModelId(model) ? { streamKeepAlive: true } : {}), ...(official ? {family: 'anthropic' as const} : {}) }
  }
  return {
    async resolveOverride(input: { compaction: boolean; model: string; webSearch?: boolean; kind?: 'message' | 'count_tokens' }): Promise<GatewayOverride | undefined> {
      if (options.roleSnapshot) {
        const snapshot = options.roleSnapshot
        const role = claudeRequestRole(input.model, snapshot.nativeRoleModels)
        if (closed) throw new GatewayRoutingError('Session model routing is closed')
        if (!role && /^matou-role\//.test(input.model)) throw new GatewayRoutingError('Unknown Claude model role')
        const custom = input.kind !== 'count_tokens' && input.compaction && snapshot.preferences.compaction.mode === 'custom'
        const search = !input.compaction && input.webSearch && snapshot.webSearchTarget
        const selected = custom ? snapshot.compactionTarget : search || (role ? snapshot.roles[role] : undefined)
        if (!selected) {
          if (custom) throw new GatewayRoutingError('Custom compaction target is unavailable')
          const source = options.sourceConnection
          if (!source) throw new GatewayRoutingError('Original supplier source connection was not bound at startup; choose a configured role')
          try {
            if (source.kind === 'subscription') source.assertActive()
            const headers = source.kind === 'oauth' ? (await source.readHeaders()).headers : { ...source.headers }
            const family = source.kind === 'oauth' || new URL(source.baseUrl).hostname === 'api.anthropic.com' ? 'anthropic' as const : source.kind === 'subscription' && options.provider?.subscription?.provider === 'codex' ? 'chatgpt-subscription' as const : undefined
            const sourceEffort = input.kind === 'count_tokens' ? undefined : roleEffort(options.provider, input.model)
            return { ...(source.kind === 'api' ? {} : { authentication: authentication(claudeRequestRole(input.model, snapshot.nativeRoleModels) ?? 'main')! }), baseUrl: source.baseUrl, headers, model: input.model, ...(source.kind === 'subscription' ? { routerTarget: true, ...(source.targetSignal ? { targetSignal: source.targetSignal } : {}) } : {}), ...(family ? { family } : {}), ...(sourceEffort && sourceEffort !== 'follow' ? { effort: sourceEffort } : {}) }
          } catch (error) { throw new GatewayRoutingError(error instanceof SubscriptionAccountBindingError ? error.message : 'Original supplier account identity is unavailable') }
        }
        const targetLabel = search ? 'webSearch' : role ?? (custom ? 'compaction' : 'main')
        const label = `${targetLabel === 'webSearch' ? '网页搜索' : targetLabel} · ${selected.providerName ?? selected.providerId}`
        try {
          selected.assertActive?.()
          const headers = selected.resolveHeaders ? await selected.resolveHeaders() : { ...selected.headers }
          const base: GatewayOverride = { ...(selected.kind === 'subscription' ? { routerTarget: true } : {}), ...(selected.kind === 'api' ? {} : { authentication: { providerId: selected.providerId, providerName: selected.providerName ?? selected.providerId, ...(selected.accountIdentity ? { accountIdentity: selected.accountIdentity } : {}), label: targetLabel } }), baseUrl: selected.baseUrl, headers, model: selected.model, ...(selected.targetSignal ? { targetSignal: selected.targetSignal } : {}), ...(selected.family ? { family: selected.family } : {}) }
          if (input.kind === 'count_tokens') return base
          const compactEffort = snapshot.preferences.compaction.effort
          const effort = custom && compactEffort !== 'follow' ? compactEffort : custom && role ? snapshot.roles[role].effort : selected.effort
          if (effort && effort !== 'follow' && selected.metadata?.supportedEfforts && !selected.metadata.supportedEfforts.includes(effort)) throw new Error('Unsupported effort')
          const mode = snapshot.preferences.contextWindows[selected.providerId] ?? 'default'
          const rawWindow = mode === 'maximum' ? selected.metadata?.maxContextWindow ?? selected.metadata?.contextWindow : selected.metadata?.contextWindow
          const contextWindow = rawWindow === undefined ? undefined : mode === '512k' ? Math.min(512_000, rawWindow) : rawWindow
          const streamKeepAlive = isGeminiModelId(selected.catalogModel) || isGeminiModelId(selected.model)
          return { ...base,
            effort: effort && effort !== 'follow' ? effort : 'omit' as const,
            thinking: effort && effort !== 'follow' && /^claude-/.test(selected.model) ? 'adaptive' as const : 'omit' as const,
            ...(selected.metadata?.maxOutputTokens ? { maxOutputTokens: selected.metadata.maxOutputTokens } : {}),
            ...(contextWindow ? { contextWindow } : {}),
            ...(streamKeepAlive ? { streamKeepAlive: true } : {}) }
        } catch (error) { throw new GatewayRoutingError(error instanceof SubscriptionAccountBindingError ? error.message : `${label}: model target or reasoning effort is unavailable`) }
      }
      const compact = options.preferences.compaction
      if (input.compaction && compact.mode === 'custom') {
        // Read active official credentials for each compaction; router leases are reused per session.
        if (!target) target = prepareTarget().catch(error => { target = undefined; throw error })
        const selected = await target
        const official = new URL(selected.baseUrl).origin === 'https://api.anthropic.com' && !selected.headers['x-api-key']
        const effort = compact.effort === 'follow' ? roleEffort(options.provider, input.model) : compact.effort
        return { ...selected, ...(selected.authentication ? { authentication: { ...selected.authentication, label: claudeRequestRole(input.model) ?? 'compaction' } } : {}), ...(official ? { headers: await officialHeaders(env) } : {}),
          ...(!effort || effort === 'follow' ? {} : { effort, thinking: 'adaptive' as const }) }
      }
      const effort = roleEffort(options.provider, input.model)
      const detectedRole = claudeRequestRole(input.model)
      const apiRole = /^matou-api\/(opus|sonnet|haiku)$/.exec(input.model)?.[1] as 'opus' | 'sonnet' | 'haiku' | undefined
      const model = apiRole && !options.provider?.subscription ? options.provider?.modelMapping?.[apiRole] : undefined
      const catalogCandidate = model ?? (detectedRole ? options.provider?.modelMapping?.[detectedRole] : undefined) ?? options.provider?.model
      const streamKeepAlive = isGeminiModelId(catalogCandidate) || isGeminiModelId(input.model)
      const resolvedModelId = (model ?? input.model).replace(/\[1m\]$/i, '')
      const metadata = options.provider?.modelMetadata?.find(entry => entry.id === resolvedModelId)
      const mode = options.provider ? options.preferences.contextWindows[options.provider.id] ?? 'default' : 'default'
      const rawWindow = mode === 'maximum' ? metadata?.maxContextWindow ?? metadata?.contextWindow : metadata?.contextWindow
      const contextWindow = rawWindow === undefined ? undefined : mode === '512k' ? Math.min(512_000, rawWindow) : rawWindow
      const auth = authentication(detectedRole ?? 'main')
      return auth || model || contextWindow || streamKeepAlive || effort && effort !== 'follow'
        ? {
            ...(options.provider?.subscription ? { routerTarget: true } : {}),
            ...(auth ? { authentication: auth } : {}),
            ...(model ? { model } : {}),
            ...(effort && effort !== 'follow' ? { effort } : {}),
            ...(contextWindow ? { contextWindow } : {}),
            ...(streamKeepAlive ? { streamKeepAlive: true } : {})
          }
        : undefined
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
  const gemini = [provider?.model, ...Object.values(provider?.modelMapping ?? {})].some(isGeminiModelId)
  return Boolean(provider?.subscription || provider && !provider.hasApiKey && provider.endpoint.replace(/\/$/, '') === 'https://api.anthropic.com') || Boolean(preferences.claudeWebSearch) || Object.keys(preferences.claudeRoleAssignments ?? {}).length > 0 || deepSeek || gemini || preferences.compaction.mode === 'custom' || Object.values(provider?.effortMapping ?? {}).some(value => value !== 'follow') || Boolean(provider?.reasoningEffort && provider.reasoningEffort !== 'follow')
}
export function restoredProviderModel(bound: string | undefined, selected: string | undefined, provider: ProviderConfigView | undefined, preferences: AgentModelPreferences): string | undefined {
  if (/^matou-role\//.test(bound ?? '')) return Object.keys(preferences.claudeRoleAssignments ?? {}).length || preferences.claudeWebSearch ? bound : selected
  const apiRole = /^matou-api\/(opus|sonnet|haiku)$/.exec(bound ?? '')?.[1] as 'opus' | 'sonnet' | 'haiku' | undefined
  if (apiRole) {
    const mapped = provider?.modelMapping?.[apiRole]
    if (provider?.subscription || !mapped) return selected
    return modelRoutingEnabled(preferences, provider) ? bound : mapped
  }
  if (!provider?.subscription && /^matou-(chatgpt|antigravity)\//.test(bound ?? '')) return selected
  return bound ?? selected
}
