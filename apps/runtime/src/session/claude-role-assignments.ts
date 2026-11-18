import { type AgentModelPreferences, type ClaudeModelRole, type ClaudeModelTarget, type ClaudeRoleAssignments, type EgressFamily, type ProviderConfigSnapshot, type ProviderConfigView, type ProviderModelMetadata, type ReasoningEffort, type SubscriptionModel, type SubscriptionProvider } from '@matou/contracts'

export const CLAUDE_MODEL_ROLES = ['opus', 'sonnet', 'haiku'] as const
export function validateClaudeTargetId(value: unknown, maximum: number): string {
  if (typeof value !== 'string' || !value.trim() || value.length > maximum || /[\x00-\x1f\x7f]/.test(value)) throw new Error('Invalid Claude target ID')
  return value.trim()
}
export function validateClaudeModelTarget(value: unknown): ClaudeModelTarget {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid Claude model target')
  const row = value as Record<string, unknown>
  return { providerId: validateClaudeTargetId(row.providerId, 256), model: validateClaudeTargetId(row.model, 240) }
}
export function validateClaudeRoleAssignments(value: unknown): ClaudeRoleAssignments {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid Claude role assignments')
  const result: ClaudeRoleAssignments = {}
  for (const [role, target] of Object.entries(value)) {
    if (!CLAUDE_MODEL_ROLES.includes(role as typeof CLAUDE_MODEL_ROLES[number]) || !target || typeof target !== 'object' || Array.isArray(target)) throw new Error('Invalid Claude role assignment')
    result[role as typeof CLAUDE_MODEL_ROLES[number]] = validateClaudeModelTarget(target)
  }
  return result
}

export function usesNativeClaudeCatalog(provider: ProviderConfigView): boolean {
  return provider.cli === 'claude-code' && provider.builtIn === true && provider.modelMetadata === undefined && provider.endpoint.replace(/\/$/, '') === 'https://api.anthropic.com'
}
/** provider.model is a startup choice, never an implicit mapping for every tier. */
export function defaultClaudeRoleModel(provider: ProviderConfigView, role: ClaudeModelRole, nativeModels: readonly SubscriptionModel[]): string {
  if (provider.modelMapping?.[role]) return provider.modelMapping[role]!
  const native = nativeModels.find(m => m.id === role) ?? nativeModels.find(m => m.id.startsWith(`claude-${role}-`))
  if (provider.subscription || !native) throw new Error(`${role} · ${provider.name}: native role model is unavailable; configure this role's model`)
  return usesNativeClaudeCatalog(provider) ? native.id : native.resolvedModel ?? native.id
}
/** Assignment validation never promotes a saved role mapping to catalog evidence. */
export function assignedModelMetadata(provider: ProviderConfigView, model: string, nativeModels: readonly ProviderModelMetadata[]): ProviderModelMetadata | undefined {
  if (provider.cli !== 'claude-code') throw new Error('Assignment provider is unavailable')
  if (provider.subscription && provider.subscription.account.status !== 'ready') throw new Error('Assignment account is unavailable')
  const catalog = provider.subscription?.modelMetadata ?? (usesNativeClaudeCatalog(provider) ? nativeModels : provider.modelMetadata)
  if (catalog === undefined) return undefined // Existing API custom-model support; no network probe.
  if (!catalog.length) throw new Error('Assignment model catalog is unavailable')
  const metadata = catalog.find(item => item.id === model)
  if (!metadata) throw new Error('Assignment model is unavailable')
  return metadata
}

// Connection values are supplied by runtime preparation, never a renderer payload.
export type ClaudeTargetConnection =
  | { kind: 'api'; baseUrl: string; headers: Readonly<Record<string, string>> }
  | { kind: 'oauth'; baseUrl: string; identity: string; readHeaders(): Promise<{ identity: string; headers: Record<string, string> }> }
  | { kind: 'subscription'; models: Readonly<Record<string, { baseUrl: string; headers: Readonly<Record<string, string>>; model: string; metadata: ProviderModelMetadata; accountIdentity: string; assertActive: () => void; targetSignal?: AbortSignal }>> }
export interface ResolvedClaudeRoleTarget {
  readonly providerId: string
  readonly providerName?: string
  readonly catalogModel: string
  readonly model: string
  readonly kind: ClaudeTargetConnection['kind']
  readonly baseUrl: string
  readonly headers?: Readonly<Record<string, string>>
  readonly assertActive?: () => void
  readonly targetSignal?: AbortSignal
  readonly resolveHeaders?: () => Promise<Record<string, string>>
  readonly metadata?: Readonly<ProviderModelMetadata>
  readonly effort?: ReasoningEffort
  readonly family?: EgressFamily
  readonly subscriptionProvider?: SubscriptionProvider
  readonly accountIdentity?: string
  readonly hideTimeZone: boolean
}
export interface ResolvedClaudeRoleSnapshot {
  readonly preferences: Readonly<AgentModelPreferences>
  readonly roles: Readonly<Record<ClaudeModelRole, ResolvedClaudeRoleTarget>>
  readonly webSearchTarget?: ResolvedClaudeRoleTarget
  readonly compactionTarget?: ResolvedClaudeRoleTarget
  readonly nativeRoleModels?: Readonly<Partial<Record<ClaudeModelRole, string>>>
  readonly officialFamilies: readonly EgressFamily[]
}
export interface ClaudeRoleSnapshotInput {
  preferences: AgentModelPreferences
  providers: ProviderConfigSnapshot
  connections: Readonly<Record<string, ClaudeTargetConnection>>
  nativeModels?: readonly SubscriptionModel[]
}

function freezeData<T>(value: T): T {
  // Account revocation must retain the signal's mutable internal state.
  if (value && typeof value === 'object' && !(value instanceof AbortSignal)) {
    for (const child of Object.values(value)) freezeData(child)
    Object.freeze(value)
  }
  return value
}

/** Pure capture: no store reads, HTTP, credential access or lease ownership here.
 * The caller owns prepared subscription leases until the process ends (Task 2).
 */
export function resolveClaudeRoleSnapshot(input: ClaudeRoleSnapshotInput): ResolvedClaudeRoleSnapshot | undefined {
  const assignments = validateClaudeRoleAssignments(input.preferences.claudeRoleAssignments === undefined ? {} : input.preferences.claudeRoleAssignments)
  if (!Object.keys(assignments).length && !input.preferences.claudeWebSearch) return undefined
  const preferences = freezeData(structuredClone(input.preferences))
  const providers = structuredClone(input.providers.providers['claude-code'])
  const nativeModels = structuredClone(input.nativeModels ?? [])
  const findProvider = (id: string) => {
    const provider = providers.find(p => p.id === id && p.cli === 'claude-code')
    if (!provider) throw new Error('Assignment provider is unavailable')
    return provider
  }
  const resolve = (provider: ProviderConfigView, model: string, role?: ClaudeModelRole, label = role ?? 'compaction'): ResolvedClaudeRoleTarget => {
    model = validateClaudeTargetId(model, 240)
    let metadata: ProviderModelMetadata | undefined
    try { metadata = assignedModelMetadata(provider, model, nativeModels) }
    catch { throw new Error(`${label} · ${provider.name}: assignment model or account is unavailable; configure this role's model`) }
    const connection = input.connections[provider.id]
    if (!connection) throw new Error('Assignment target connection is unavailable')
    const effort = (role ? provider.effortMapping?.[role] : undefined) ?? provider.reasoningEffort
    const common = { providerId: provider.id, providerName: provider.name, catalogModel: model, ...(effort ? { effort } : {}) }
    if (provider.subscription) {
      if (connection.kind !== 'subscription') throw new Error('Subscription target connection is unavailable')
      const prepared = connection.models[model]
      const prefix = provider.subscription.provider === 'codex' ? 'matou-chatgpt' : 'matou-antigravity'
      if (!prepared || !prepared.accountIdentity || typeof prepared.assertActive !== 'function' || prepared.model !== `${prefix}/${model}` || prepared.metadata.id !== model) throw new Error('Exact subscription model target is unavailable')
      const family = provider.subscription.provider === 'codex' ? 'chatgpt-subscription' as const : undefined
      return freezeData({ ...common, kind: 'subscription', subscriptionProvider: provider.subscription.provider, hideTimeZone: true, baseUrl: prepared.baseUrl, model: prepared.model, accountIdentity: prepared.accountIdentity, assertActive: prepared.assertActive.bind(prepared), ...(prepared.targetSignal ? { targetSignal: prepared.targetSignal } : {}), headers: { ...prepared.headers }, metadata: structuredClone(prepared.metadata), ...(family ? { family } : {}) })
    }
    if (connection.kind === 'subscription') throw new Error('API target connection is unavailable')
    const family = new URL(connection.baseUrl).hostname === 'api.anthropic.com' ? 'anthropic' as const : undefined
    const actualModel = (metadata as SubscriptionModel | undefined)?.resolvedModel ?? model
    const target = { ...common, hideTimeZone: family !== undefined, kind: connection.kind, baseUrl: connection.baseUrl, model: actualModel, ...(metadata ? { metadata: structuredClone(metadata) } : {}), ...(family ? { family } : {}) }
    if (connection.kind === 'api') return freezeData({ ...target, headers: { ...connection.headers } })
    // Capture callback and identity separately; mutating the source object cannot
    // replace the account reader. The reader may refresh tokens for this identity.
    const identity = connection.identity, readHeaders = connection.readHeaders.bind(connection)
    if (!identity || family !== 'anthropic') throw new Error('Official account identity is unavailable')
    return freezeData({ ...target, accountIdentity: identity, resolveHeaders: async () => {
      const current = await readHeaders()
      if (current.identity !== identity) throw new Error('Official account identity changed')
      return { ...current.headers }
    } })
  }
  const roles = {} as Record<ClaudeModelRole, ResolvedClaudeRoleTarget>
  for (const role of CLAUDE_MODEL_ROLES) {
    const assignment = assignments[role]
    const provider = findProvider(assignment?.providerId ?? input.providers.activeProviderIds['claude-code'])
    const model = assignment?.model ?? defaultClaudeRoleModel(provider, role, nativeModels)
    roles[role] = resolve(provider, model, role)
  }
  const compact = preferences.compaction
  if (compact.mode === 'custom' && (!compact.providerId || !compact.model)) throw new Error('Compaction target is unavailable')
  const compactionTarget = compact.mode === 'custom' && compact.providerId && compact.model
    ? resolve(findProvider(compact.providerId), compact.model) : undefined
  const search = preferences.claudeWebSearch
  const webSearchTarget = search ? resolve(findProvider(search.providerId), search.model, undefined, '网页搜索') : undefined
  const officialFamilies = [...new Set([...Object.values(roles), ...(webSearchTarget ? [webSearchTarget] : []), ...(compactionTarget ? [compactionTarget] : [])].flatMap(t => t.family ? [t.family] : []))]
  const nativeRoleModels = Object.fromEntries(CLAUDE_MODEL_ROLES.flatMap(role => {
    const model = nativeModels.find(m => m.id === role)?.resolvedModel
    return model ? [[role, model]] : []
  }))
  return freezeData({ preferences, roles, nativeRoleModels, ...(webSearchTarget ? { webSearchTarget } : {}), ...(compactionTarget ? { compactionTarget } : {}), officialFamilies })
}
