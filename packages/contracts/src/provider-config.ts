export type ReasoningEffort = 'follow' | 'low' | 'medium' | 'high' | 'xhigh' | 'max'
export type RoleEffortMapping = Partial<Record<'opus' | 'sonnet' | 'haiku', ReasoningEffort>>
export type ProviderCli = 'claude-code' | 'codex'

/**
 * Sentinel stored in `ProviderConfigView.model` when a provider keeps the model
 * the CLI picks itself. It is an identifier, not display text: the renderer
 * matches it and shows a localised label instead.
 */
export const CLI_DEFAULT_MODEL = 'CLI 默认'

export interface ProviderConfigView {
  id: string
  cli: ProviderCli
  name: string
  endpoint: string
  model: string
  hasApiKey: boolean
  /** Metadata fetched from this API service, retained when its configuration is saved. */
  modelMetadata?: ProviderModelMetadata[]
  reasoningEffort?: ReasoningEffort
  effortMapping?: RoleEffortMapping
  modelMapping?: Partial<Record<'opus' | 'sonnet' | 'haiku', string>>
  builtIn?: boolean
  kind?: 'api-key' | 'subscription'
  subscription?: {
    provider: SubscriptionProvider
    account: SubscriptionAccountSummary
    modelMetadata: SubscriptionModel[]
  }
}

export interface ProviderConfigSnapshot {
  revision: number
  providers: Record<ProviderCli, ProviderConfigView[]>
  activeProviderIds: Record<ProviderCli, string>
  /** Runtime capability; omitted by storage-only mutation results. */
  subscriptionSupported?: boolean
}

export type ProviderSessionActivationDeferredReason =
  | 'durability-fault'
  | 'recovery-not-ready'
  | 'provider-identity-pending'
  | 'session-not-running'
  | 'binding-update-failed'
  | 'restart-unavailable'

export type ProviderSessionActivationTransition =
  | { sessionId: string; status: 'updated' }
  | {
      sessionId: string
      status: 'deferred'
      reason: ProviderSessionActivationDeferredReason
    }

/** Structured preparation failure; card account status keeps its existing enum. */
export interface ProviderActivationFailure {
  status: 'router-start-failed' | 'needs-sign-in'
  /** Compatible display string in the runtime locale; never raw engine details. */
  reason?: string
  reasonLocalized?: { zh: string; en: string }
  /** Complete user-facing copy, including the localized reason when present. */
  message: { zh: string; en: string }
  recovery: 'retry' | 'sign-in'
}

export interface ProviderConfigActivationResult extends ProviderConfigSnapshot {
  sessionTransitions: ProviderSessionActivationTransition[]
}

export interface ApiKeyProviderConfigInput {
  kind?: 'api-key'
  id?: string
  cli: ProviderCli
  name: string
  endpoint: string
  model: string
  apiKey?: string
  modelMetadata?: ProviderModelMetadata[]
  reasoningEffort?: ReasoningEffort
  effortMapping?: RoleEffortMapping
  modelMapping?: Partial<Record<'opus' | 'sonnet' | 'haiku', string>>
}

export type SubscriptionProvider = 'codex' | 'antigravity'
export interface ProviderModelMetadata {
  id: string
  displayName?: string
  maxContextWindow?: number
  supportedEfforts?: Exclude<ReasoningEffort, 'follow'>[]
  contextWindow?: number
  maxOutputTokens?: number
}
export interface SubscriptionModel extends ProviderModelMetadata {
  resolvedModel?: string
  ownedBy?: string
}
export interface SubscriptionAccountSummary {
  email?: string
  plan?: string
  status: 'ready' | 'needs-sign-in' | 'account-unavailable' | 'router-not-running'
  reason?: string
}
export interface SubscriptionQuotaWindow {
  name: string
  group?: string
  windowSeconds?: number
  usedPercent: number
  resetsAt?: string
}
export interface SubscriptionAccountDetails {
  account: SubscriptionAccountSummary
  models: SubscriptionModel[]
  quota?: SubscriptionQuotaWindow[]
}
export interface SubscriptionProviderConfigInput {
  kind: 'subscription'
  id?: string
  cli: 'claude-code'
  name: string
  subscription: { provider: SubscriptionProvider; attemptId?: string }
  effortMapping?: RoleEffortMapping
  modelMapping: Record<'opus' | 'sonnet' | 'haiku', string>
  // Keep legacy property lookups type-safe while rejecting these on the wire.
  endpoint?: never
  model?: never
  apiKey?: never
}
export type ProviderConfigInput = ApiKeyProviderConfigInput | SubscriptionProviderConfigInput
export interface SubscriptionLoginStartInput { provider: SubscriptionProvider; locale?: 'zh-CN' | 'en' }
export type SubscriptionModelsReadInput = { attemptId: string; providerId?: never } | { providerId: string; attemptId?: never }
export interface SubscriptionLoginView {
  attemptId: string
  provider: SubscriptionProvider
  status: 'waiting' | 'exchanging' | 'ready' | 'failed' | 'cancelled' | 'expired' | 'saved'
  reason?: string
  reasonLocalized?: { zh: string; en: string }
  /** Browser launch failed, but this attempt can still receive its callback. */
  canReopen?: boolean
  details?: SubscriptionAccountDetails
}
export interface SubscriptionSettingsSnapshot {
  snapshot: ProviderConfigSnapshot
  accounts: Record<string, SubscriptionAccountDetails>
  antigravityRiskAccepted: boolean
}

export interface ProviderModelsInput {
  cli: ProviderCli
  providerId?: string
  endpoint: string
  apiKey?: string
}
export type ProviderModelsResult =
  | { models: string[]; modelMetadata?: ProviderModelMetadata[] }
  | { error: 'auth' | 'unsupported' | 'timeout' | 'network' | 'invalid' | 'key-required' | 'empty' | 'too-large' }

/** Curated direct-connect presets adapted from CC Switch (MIT).
 * Source commit: 6b4c09aca28cbf5357121814314011d7a203675a, 2026-10-13.
 * See ../third-party/provider-presets-LICENSE. Referral links intentionally omitted.
 * These are configuration defaults, not assertions of tested account access.
 */
export interface ProviderPreset {
  id: string
  name: string
  endpoint: string
  model: string
  keyUrl: string
  modelsUrl?: string
  modelMapping?: ProviderConfigInput['modelMapping']
}
const mapping = (model: string, haiku = model) => ({ opus: model, sonnet: model, haiku })
export const PROVIDER_PRESETS: Record<ProviderCli, readonly ProviderPreset[]> = {
  'claude-code': [
    { id: 'deepseek', name: 'DeepSeek', endpoint: 'https://api.deepseek.com/anthropic', model: 'deepseek-v4-pro', modelMapping: mapping('deepseek-v4-pro', 'deepseek-flash'), keyUrl: 'https://platform.deepseek.com/api_keys', modelsUrl: 'https://api.deepseek.com/models' },
    { id: 'glm', name: '智谱 GLM', endpoint: 'https://open.bigmodel.cn/api/anthropic', model: 'glm-5.3', modelMapping: mapping('glm-5.3'), keyUrl: 'https://open.bigmodel.cn' },
    { id: 'kimi', name: 'Kimi', endpoint: 'https://api.moonshot.cn/anthropic', model: 'kimi-k2.7-code', modelMapping: mapping('kimi-k2.7-code'), keyUrl: 'https://platform.kimi.com/console/api-keys' },
    { id: 'minimax', name: 'MiniMax', endpoint: 'https://api.minimax.cn/anthropic', model: 'MiniMax-M3[1M]', modelMapping: mapping('MiniMax-M3[1M]'), keyUrl: 'https://platform.minimax.cn/subscribe/token-plan' }
  ],
  codex: [
    { id: 'deepseek', name: 'DeepSeek', endpoint: 'https://api.deepseek.com', model: 'deepseek-flash', keyUrl: 'https://platform.deepseek.com/api_keys', modelsUrl: 'https://api.deepseek.com/models' },
    { id: 'glm', name: '智谱 GLM', endpoint: 'https://open.bigmodel.cn/api/v1', model: 'glm-5.3', keyUrl: 'https://open.bigmodel.cn' },
    { id: 'kimi', name: 'Kimi', endpoint: 'https://api.moonshot.cn/v1', model: 'kimi-k3', keyUrl: 'https://platform.kimi.com/console/api-keys' },
    { id: 'minimax', name: 'MiniMax', endpoint: 'https://api.minimax.cn/v1', model: 'MiniMax-M3', keyUrl: 'https://platform.minimax.cn/subscribe/token-plan' }
  ]
}
