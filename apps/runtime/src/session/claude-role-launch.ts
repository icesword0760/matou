import { createHash } from 'node:crypto'
import { CLI_DEFAULT_MODEL, type AgentModelPreferences, type ClaudeModelRole, type ProviderConfigView } from '@matou/contracts'
import type { ProviderConfigStore } from '../provider-config/provider-config-store'
import type { SubscriptionAccounts } from '../model-router/subscription-accounts'
import type { RouterLease } from '../model-router/manager'
import { CLAUDE_MODEL_ROLES, defaultClaudeRoleModel, resolveClaudeRoleSnapshot, usesNativeClaudeCatalog, type ClaudeTargetConnection, type ResolvedClaudeRoleSnapshot } from './claude-role-assignments'
import { captureOfficialClaudeAccount } from './claude-official-account'
import { nativeClaudeModels } from './native-model-catalog'

/** Same configuration-only digest at spawn and explicit global activation. */
export function claudeRoleLaunchDigest(preferences: AgentModelPreferences, captured: Awaited<ReturnType<ProviderConfigStore['captureClaudeRouting']>>): string {
  const ids = [...new Set([captured.providers.activeProviderIds['claude-code'], ...Object.values(preferences.claudeRoleAssignments ?? {}).map(t => t.providerId), ...(preferences.claudeWebSearch ? [preferences.claudeWebSearch.providerId] : []), ...(preferences.compaction.mode === 'custom' && preferences.compaction.providerId ? [preferences.compaction.providerId] : [])])].sort()
  const providers = ids.map(id => {
    const p = captured.providers.providers['claude-code'].find(p => p.id === id)
    return { id, endpoint: p?.endpoint, model: p?.model, mapping: p?.modelMapping, effort: p?.effortMapping, reasoningEffort: p?.reasoningEffort, subscription: p?.subscription?.provider,
      models: (p?.subscription?.modelMetadata ?? p?.modelMetadata)?.map(m => ({ id: m.id, contextWindow: m.contextWindow, maxContextWindow: m.maxContextWindow, maxOutputTokens: m.maxOutputTokens, supportedEfforts: m.supportedEfforts })),
      headers: captured.apiConnections[id]?.headers, contextMode: preferences.contextWindows[id] }
  })
  const canonical = (value: unknown): unknown => Array.isArray(value) ? value.map(canonical) : value && typeof value === 'object' ? Object.fromEntries(Object.entries(value).sort(([a],[b]) => a.localeCompare(b)).map(([k,v]) => [k,canonical(v)])) : value
  return createHash('sha256').update(JSON.stringify(canonical({ defaultProviderId: captured.providers.activeProviderIds['claude-code'], assignments: preferences.claudeRoleAssignments, webSearch: preferences.claudeWebSearch, compaction: preferences.compaction, providers }))).digest('hex')
}

export function claudeRequestRole(model: string, nativeRoleModels: Readonly<Partial<Record<ClaudeModelRole, string>>> = {}): ClaudeModelRole | undefined {
  const clean = model.replace(/\[1m\]$/i, '')
  if (['matou-role/opus-fallback', 'claude-fable-5', 'claude-fable-5-2', 'claude-fable-5-5'].includes(clean)) return 'opus'
  const role = /^(?:matou-role\/|matou-api\/)?(opus|sonnet|haiku|fable)$/.exec(clean)?.[1]
    ?? /^(?:(?:matou-chatgpt|matou-antigravity)\/)?matou-(opus|sonnet|haiku|opus-fallback)$/.exec(clean)?.[1]
  const subscriptionRole = /^(?:matou-antigravity\/)?claude-(opus|sonnet)-(?:5|5-5)$/.exec(clean)?.[1]
    ?? (/^(?:matou-antigravity\/)?claude-haiku-5-5$/.test(clean) ? 'haiku' : undefined)
  if (subscriptionRole) return subscriptionRole as ClaudeModelRole
  if (role) return role === 'fable' || role === 'opus-fallback' ? 'opus' : role as ClaudeModelRole
  return CLAUDE_MODEL_ROLES.find(role => nativeRoleModels[role]?.replace(/\[1m\]$/i, '') === clean)
}
export interface PreparedClaudeRoleLaunch {
  snapshot: ResolvedClaudeRoleSnapshot
  providers: import('@matou/contracts').ProviderConfigSnapshot
  sourceModel: string
  sourceEnvironment: Record<string, string>
  sourceConnection?: Exclude<ClaudeTargetConnection, { kind: 'subscription' }> | { kind: 'subscription'; baseUrl: string; headers: Readonly<Record<string, string>>; assertActive: () => void; targetSignal?: AbortSignal }
  contextWindow: number | null
  unknownCompactionContext: boolean
  unknownContextRoles: readonly ClaudeModelRole[]
  digest: string
  resolveMainModel(model?: string): string
  close(): void
}
export async function prepareClaudeRoleLaunch(options: {
  preferences: AgentModelPreferences; store: ProviderConfigStore; accounts?: SubscriptionAccounts; env: NodeJS.ProcessEnv; isClosed?: () => boolean
}): Promise<PreparedClaudeRoleLaunch | undefined> {
  if (!Object.keys(options.preferences.claudeRoleAssignments ?? {}).length && !options.preferences.claudeWebSearch) return undefined
  const preferences = structuredClone(options.preferences)
  const leases: RouterLease[] = []
  let closed = false
  const close = () => { if (!closed) { closed = true; for (const lease of leases) lease.release() } }
  const assertOpen = () => { if (closed || options.isClosed?.()) throw new Error('Session closed during model target preparation') }
  try {
    const captured = await options.store.captureClaudeRouting()
    const { providers, apiConnections } = captured
    const nativeModels = await nativeClaudeModels()
    assertOpen()
    const connections: Record<string, ClaudeTargetConnection> = { ...apiConnections }
    const assigned = preferences.claudeRoleAssignments ?? {}
    const requirements: Array<{ role: ClaudeModelRole | 'compaction' | 'webSearch'; providerId: string; provider: ProviderConfigView | undefined; model: string }> = CLAUDE_MODEL_ROLES.map(role => {
      const providerId = assigned[role]?.providerId ?? providers.activeProviderIds['claude-code']
      const provider = providers.providers['claude-code'].find(p => p.id === providerId)
      return { role, providerId, provider, model: assigned[role]?.model ?? (provider ? defaultClaudeRoleModel(provider, role, nativeModels) : '') }
    })
    const sourceProvider = providers.providers['claude-code'].find(p => p.id === providers.activeProviderIds['claude-code'])
    if (preferences.compaction.mode === 'custom') {
      const providerId = preferences.compaction.providerId ?? ''
      requirements.push({ role: 'compaction', providerId, provider: providers.providers['claude-code'].find(p => p.id === providerId), model: preferences.compaction.model ?? '' })
    }
    if (preferences.claudeWebSearch) {
      const { providerId, model } = preferences.claudeWebSearch
      requirements.push({ role: 'webSearch', providerId, model, provider: providers.providers['claude-code'].find(p => p.id === providerId) })
    }
    for (const { role, providerId, provider, model } of requirements) {
      try {
        if (!provider) throw new Error('Provider unavailable')
        if (provider.subscription) {
          if (!options.accounts) throw new Error('Subscription service unavailable')
          let connection = connections[providerId]
          if (connection?.kind !== 'subscription') connections[providerId] = connection = { kind: 'subscription', models: {} }
          if (!connection.models[model]) {
            const target = await options.accounts.prepareModelTarget(providerId, model, 'role-session')
            leases.push(target.lease); assertOpen()
            connection.models = { ...connection.models, [model]: target }
          }
        } else if (!provider.hasApiKey && usesNativeClaudeCatalog(provider) && connections[providerId]?.kind !== 'oauth') {
          connections[providerId] = await captureOfficialClaudeAccount(options.env)
          assertOpen()
        } else if (!provider.hasApiKey && !usesNativeClaudeCatalog(provider)) throw new Error('API key unavailable')
      } catch (error) {
        if (options.isClosed?.()) throw new Error('Session closed during model target preparation')
        // Never include provider/transport diagnostics which may contain credentials.
        throw new Error(`${role === 'webSearch' ? '网页搜索' : role} · ${provider?.name ?? providerId}: model target is unavailable`)
      }
    }
    const snapshot = resolveClaudeRoleSnapshot({ preferences, providers, connections, nativeModels })!
    const unknownContextRoles: ClaudeModelRole[] = []
    const windows: number[] = []
    let unknownCompactionContext = false
    const budgetTargets = [...CLAUDE_MODEL_ROLES.map(role => ({ role, target: snapshot.roles[role] })), ...(snapshot.compactionTarget ? [{ role: 'compaction' as const, target: snapshot.compactionTarget }] : [])]
    for (const { role, target } of budgetTargets) {
      const metadata = target.metadata
      const actual = metadata?.contextWindow
      const mode = preferences.contextWindows[target.providerId] ?? 'default'
      const cap = mode === 'maximum' ? metadata?.maxContextWindow ?? actual : actual
      if (cap !== undefined && cap < 100000) throw new Error(`${role} · ${requirements.find(r => r.role === role)?.provider?.name}: context window is under 100K`)
      if (cap === undefined) { if (role === 'compaction') unknownCompactionContext = true; else unknownContextRoles.push(role) }
      else windows.push(mode === '512k' ? Math.min(512000, cap) : cap)
    }
    // Only actual roles/compaction require preparation. An unused source must not
    // read an official identity or acquire a subscription lease just to launch.
    const source = sourceProvider ? connections[sourceProvider.id] : undefined
    const sourceTarget = source?.kind === 'subscription' ? source.models[sourceProvider?.modelMapping?.opus || sourceProvider?.model || ''] : undefined
    const sourceConnection: PreparedClaudeRoleLaunch['sourceConnection'] = source?.kind === 'subscription'
      ? sourceTarget ? { kind: 'subscription', baseUrl: sourceTarget.baseUrl, headers: sourceTarget.headers, assertActive: sourceTarget.assertActive, ...(sourceTarget.targetSignal ? { targetSignal: sourceTarget.targetSignal } : {}) } : undefined
      : source?.kind === 'oauth' || sourceProvider?.hasApiKey ? source : undefined
    const sourceKey = sourceConnection?.kind === 'api' ? sourceConnection.headers['x-api-key'] : sourceConnection?.kind === 'subscription' ? sourceConnection.headers.authorization?.replace(/^Bearer /, '') : undefined
    const sourceEnvironment = { ANTHROPIC_BASE_URL: sourceConnection?.baseUrl ?? 'http://127.0.0.1:1', ANTHROPIC_API_KEY: sourceConnection?.kind === 'api' ? sourceKey ?? '' : sourceConnection ? '' : 'matou-local-role-gateway', ANTHROPIC_AUTH_TOKEN: sourceKey ?? '', CLAUDE_CODE_OAUTH_TOKEN: '' }
    const defaultModel = nativeModels.find(m => m.id === 'default')?.resolvedModel
    const digest = claudeRoleLaunchDigest(preferences, captured)
    assertOpen()
    return { snapshot, providers, sourceModel: sourceProvider!.subscription ? 'opus' : sourceProvider!.model, ...(sourceConnection ? { sourceConnection } : {}), sourceEnvironment, contextWindow: windows.length ? Math.min(...windows, 1000000) : null, unknownContextRoles, unknownCompactionContext, digest,
      resolveMainModel(model) {
        const normalized = model?.replace(/\[1m\]$/i, '')
        let role = model ? claudeRequestRole(model, snapshot.nativeRoleModels) : undefined
        if (!normalized || normalized === 'default' || normalized === CLI_DEFAULT_MODEL) {
          role = defaultModel ? claudeRequestRole(defaultModel, snapshot.nativeRoleModels) : undefined
          if (!role) throw new Error('Default Claude model role identity is unavailable')
        } else if (!role && !sourceProvider?.subscription && normalized === sourceProvider?.model?.replace(/\[1m\]$/i, '')) {
          role = CLAUDE_MODEL_ROLES.find(candidate => sourceProvider.modelMapping?.[candidate]?.replace(/\[1m\]$/i, '') === normalized)
        }
        if (!role && !sourceConnection) throw new Error('Original supplier source connection is unavailable; choose a configured role')
        return role ? `matou-role/${role}` : model!
      }, close }
  } catch (error) { close(); throw error }
}
