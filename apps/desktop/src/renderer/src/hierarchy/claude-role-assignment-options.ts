import type { AgentModelSettingsSnapshot, ClaudeModelRole, ProviderConfigView, SubscriptionAccountDetails } from '@matou/contracts'

export const CLAUDE_ROLES = ['opus', 'sonnet', 'haiku'] as const
export function allocationModels(provider: ProviderConfigView | undefined, snapshot?: AgentModelSettingsSnapshot) {
  if (!provider) return []
  const authoritative = provider.subscription?.modelMetadata ?? provider.modelMetadata
  return authoritative !== undefined ? authoritative.map(m => ({ id: m.id, name: m.displayName ?? m.id }))
    : snapshot?.providers.find(p => p.id === provider.id)?.models ?? []
}
export function defaultAllocationModel(role: ClaudeModelRole, provider: ProviderConfigView | undefined, providers: readonly ProviderConfigView[], snapshot?: AgentModelSettingsSnapshot) {
  if (!provider) return undefined
  const mapped = provider.modelMapping?.[role]
  if (mapped) return { id: mapped, name: allocationModels(provider, snapshot).find(m => m.id === mapped)?.name ?? mapped }
  if (provider.subscription) return undefined
  const nativeProvider = providers.find(p => p.builtIn && p.endpoint.replace(/\/$/, '') === 'https://api.anthropic.com' && p.modelMetadata === undefined)
  const native = snapshot?.providers.find(p => p.id === nativeProvider?.id)?.models ?? []
  // Snapshot contains native selector names and concrete IDs, never infer from
  // provider.model (the startup choice may be a custom model unrelated to tiers).
  const selected = native.find(m => m.id === role) ?? native.find(m => m.id.startsWith(`claude-${role}-`))
  if (!selected) return undefined
  return provider.id === nativeProvider?.id ? selected : { ...selected, id: selected.resolvedModel ?? (selected.id === role ? '' : selected.id) }
}
export function allocationProblem(provider: ProviderConfigView | undefined, model: string, snapshot?: AgentModelSettingsSnapshot, accounts?: Record<string, SubscriptionAccountDetails>) {
  if (!provider) return 'provider'
  if (provider.subscription && (accounts?.[provider.id]?.account.status ?? provider.subscription.account.status) !== 'ready') return 'account'
  const officialOAuth = provider.builtIn === true && provider.modelMetadata === undefined && provider.endpoint.replace(/\/$/, '') === 'https://api.anthropic.com'
  if (!provider.subscription && !officialOAuth && !provider.hasApiKey) return 'key'
  if (!model.trim()) return 'empty'
  if (model.length > 240 || /[\x00-\x1f\x7f]/.test(model)) return 'invalid'
  const directory = provider.subscription?.modelMetadata ?? provider.modelMetadata
  if (directory !== undefined && !directory.some(m => m.id === model)) return 'model'
  if (provider.builtIn && !provider.modelMetadata && provider.endpoint.replace(/\/$/, '') === 'https://api.anthropic.com') {
    const models = allocationModels(provider, snapshot)
    if (!models.length) return 'catalog'
    if (!models.some(m => m.id === model)) return 'model'
  }
  return undefined
}
