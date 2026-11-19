import type { AgentModelSettingsSnapshot, ClaudeModelRole, ProviderConfigView, ReasoningEffort, SubscriptionAccountDetails } from '@matou/contracts'

export const CLAUDE_ROLES = ['opus', 'sonnet', 'haiku'] as const
export function allocationModels(provider: ProviderConfigView | undefined, snapshot?: AgentModelSettingsSnapshot) {
  if (!provider) return []
  const authoritative = provider.subscription?.modelMetadata ?? provider.modelMetadata
  return authoritative !== undefined ? authoritative.map(m => ({ id: m.id, name: m.displayName ?? m.id }))
    : snapshot?.providers.find(p => p.id === provider.id)?.models ?? []
}
export function allocationModelSupportedEfforts(provider: ProviderConfigView | undefined, modelId: string, snapshot?: AgentModelSettingsSnapshot): Exclude<ReasoningEffort, 'follow'>[] | undefined {
  if (!provider || !modelId.trim()) return undefined
  const authoritative = provider.subscription?.modelMetadata ?? provider.modelMetadata
  if (authoritative !== undefined) return authoritative.find(m => m.id === modelId)?.supportedEfforts
  return snapshot?.providers.find(p => p.id === provider.id)?.models.find(m => m.id === modelId)?.supportedEfforts
}
export function allocationDefaultEfforts(provider: ProviderConfigView | undefined, modelId: string): Exclude<ReasoningEffort, 'follow'>[] {
  const label = `${provider?.id ?? ''} ${provider?.name ?? ''} ${modelId}`
  if (provider?.subscription) {
    return provider.subscription.provider !== 'codex' && !/\b(gpt|codex|o[1-9])\b/i.test(label) && /claude|opus|sonnet|haiku/i.test(label)
      ? ['low', 'medium', 'high', 'xhigh', 'max']
      : ['low', 'medium', 'high', 'xhigh']
  }
  if (/\b(gpt|codex|o[1-9])\b/i.test(label)) return ['low', 'medium', 'high', 'xhigh']
  return ['low', 'medium', 'high', 'xhigh', 'max']
}
export function allocationEffortChoices(provider: ProviderConfigView | undefined, modelId: string, current: ReasoningEffort, snapshot?: AgentModelSettingsSnapshot): ReasoningEffort[] {
  const supported = allocationModelSupportedEfforts(provider, modelId, snapshot)
  const choices: ReasoningEffort[] = ['follow', ...(supported ? [...new Set(supported)] : allocationDefaultEfforts(provider, modelId))]
  if (current !== 'follow' && !choices.includes(current)) choices.push(current)
  return choices
}
export function allocationEffortSupported(provider: ProviderConfigView | undefined, modelId: string, effort: ReasoningEffort, snapshot?: AgentModelSettingsSnapshot): boolean {
  if (effort === 'follow') return true
  const supported = allocationModelSupportedEfforts(provider, modelId, snapshot)
  return supported ? supported.includes(effort) : allocationDefaultEfforts(provider, modelId).includes(effort)
}
export function allocationEffortName(effort: ReasoningEffort, zh: boolean): string {
  if (effort === 'follow') return zh ? '跟随会话' : 'Follow session'
  if (effort === 'low') return zh ? '低' : 'Low'
  if (effort === 'medium') return zh ? '中' : 'Medium'
  if (effort === 'high') return zh ? '高' : 'High'
  if (effort === 'xhigh') return zh ? '极高' : 'XHigh'
  return zh ? '最大' : 'Max'
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
