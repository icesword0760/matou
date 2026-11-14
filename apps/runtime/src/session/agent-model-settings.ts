import { CLI_DEFAULT_MODEL, type AgentModelPreferences, type AgentModelSettingsSnapshot, type ContextWindowMode, type ModelContextChoice, type ProviderConfigView, type ProviderModelMetadata } from '@matou/contracts'
import type { RuntimeDatabase } from '../storage/database'
import type { ProviderConfigStore } from '../provider-config/provider-config-store'
import { nativeClaudeModels, nativeCodexModels, nativeCodexDefaultModel } from './native-model-catalog'

const KEY = 'agent.modelPreferences'
const defaults = (): AgentModelPreferences => ({ contextWindows: {}, compaction: { mode: 'follow', effort: 'follow' } })
export function contextTokens(mode: ContextWindowMode, model?: Pick<ModelContextChoice, 'defaultTokens' | 'maximumTokens' | 'fixedTokens'>): number | undefined {
  if (mode === '512k') return model?.fixedTokens ?? 512_000
  return mode === 'maximum' ? model?.maximumTokens ?? model?.defaultTokens : model?.defaultTokens
}

function usesNativeModelCatalog(provider: ProviderConfigView): boolean {
  const officialEndpoint = provider.cli === 'claude-code' ? 'https://api.anthropic.com' : 'https://api.openai.com/v1'
  return provider.builtIn === true && provider.modelMetadata === undefined && provider.endpoint.replace(/\/$/, '') === officialEndpoint
}

function availableModels(provider: ProviderConfigView, native: ProviderModelMetadata[]): ProviderModelMetadata[] {
  if (usesNativeModelCatalog(provider)) return native
  if (provider.subscription?.modelMetadata.length) return provider.subscription.modelMetadata
  const byId = new Map(provider.modelMetadata?.map(model => [model.id, model]))
  for (const id of [provider.model, ...Object.values(provider.modelMapping ?? {})]) {
    if (id && id !== CLI_DEFAULT_MODEL && !byId.has(id)) byId.set(id, { id })
  }
  return [...byId.values()]
}

function modelChoice(model: ProviderModelMetadata): ModelContextChoice {
  return {
    id: model.id,
    name: model.displayName ?? model.id,
    ...(model.contextWindow ? { defaultTokens: model.contextWindow } : {}),
    ...(model.maxContextWindow ? { maximumTokens: model.maxContextWindow } : {}),
    fixedTokens: 512_000,
    ...(model.supportedEfforts ? { supportedEfforts: model.supportedEfforts } : {})
  }
}

export class AgentModelSettings {
  constructor(private readonly database: RuntimeDatabase, private readonly providers: ProviderConfigStore) {}
  preferences(): AgentModelPreferences {
    try { const row = this.database.get<{ value_json: string }>('SELECT value_json FROM preferences WHERE key = ?', KEY); if (row) return validate(JSON.parse(row.value_json)) } catch { /* Invalid persisted preferences use the initial provider choices. */ }
    return defaults()
  }
  async snapshot(): Promise<AgentModelSettingsSnapshot> {
    const [snapshot, claude, codex] = await Promise.all([this.providers.snapshot(), nativeClaudeModels(), nativeCodexModels()])
    const providers = (['claude-code', 'codex'] as const).flatMap(cli => snapshot.providers[cli].map(provider => {
      const models = availableModels(provider, cli === 'claude-code' ? claude : codex).map(modelChoice)
      return { id: provider.id, name: provider.name, cli, compactionAvailable: cli === 'claude-code', models }
    }))
    const preferences = this.preferences()
    preferences.contextWindows = Object.fromEntries((['claude-code', 'codex'] as const).flatMap(cli => snapshot.providers[cli].map(provider => [provider.id, preferences.contextWindows[provider.id] ?? initialContextMode(provider)])))
    return { preferences, providers }
  }
  async set(value: unknown): Promise<AgentModelSettingsSnapshot> {
    const preferences = validate(value)
    if (preferences.compaction.mode === 'custom') {
      const snapshot = await this.snapshot(), target = snapshot.providers.find(p => p.id === preferences.compaction.providerId && p.compactionAvailable)
      if (!target) throw new Error('Compaction provider is unavailable')
      const model = target.models.find(m => m.id === preferences.compaction.model)
      if (model?.supportedEfforts && preferences.compaction.effort !== 'follow' && !model.supportedEfforts.includes(preferences.compaction.effort)) throw new Error('Unsupported compaction reasoning effort')
    }
    this.database.run(`INSERT INTO preferences (key, value_json, updated_at) VALUES (?, ?, ?) ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at`, KEY, JSON.stringify(preferences), Date.now())
    return this.snapshot()
  }
  async launchWindow(provider: ProviderConfigView | undefined, model?: string): Promise<number | undefined> {
    if (!provider) return undefined
    const mode = this.preferences().contextWindows[provider.id] ?? initialContextMode(provider)
    // Most launches need no catalog probe. Fixed is a requested client budget.
    if (mode === '512k') return 512_000
    if (mode === 'default' && !provider.subscription) return undefined
    const native = usesNativeModelCatalog(provider)
    const main = model || provider.model
    if (provider.cli === 'codex') {
      const selected = native && (!main || main === CLI_DEFAULT_MODEL) ? await nativeCodexDefaultModel() : main
      const models = native ? await nativeCodexModels() : provider.modelMetadata ?? []
      const metadata = models.find(m => m.id === selected)
      return metadata?.maxContextWindow ?? metadata?.contextWindow
    }
    const models = availableModels(provider, native ? await nativeClaudeModels() : []).map(modelChoice)
    const aliasRole = /^matou-api\/(opus|sonnet|haiku)$/.exec(main)?.[1] as 'opus' | 'sonnet' | 'haiku' | undefined
    const actualMain = aliasRole ? provider.modelMapping?.[aliasRole] : native && (!main || main === CLI_DEFAULT_MODEL) ? 'default' : main
    const roleModels = Object.values(provider.modelMapping ?? {}).filter(Boolean)
    // Subscription aliases use the mapped role catalog, including the main role.
    const selected = [...new Set([...(provider.subscription ? [] : [actualMain]), ...roleModels])]
    const windows = selected.map(id => contextTokens(mode, models.find(m => m.id === id)))
    return windows.length && windows.every((v): v is number => v !== undefined) ? Math.min(...windows) : undefined
  }
}
function validate(value: unknown): AgentModelPreferences {
  const v = value as AgentModelPreferences
  if (!v || !v.contextWindows || typeof v.contextWindows !== 'object' || Array.isArray(v.contextWindows) || Object.entries(v.contextWindows).some(([id, mode]) => !id || id.length > 256 || !['default', '512k', 'maximum'].includes(mode)) || !v.compaction || !['follow', 'custom'].includes(v.compaction.mode) || !['follow', 'low', 'medium', 'high', 'xhigh', 'max'].includes(v.compaction.effort)) throw new Error('Invalid model preferences')
  const { providerId, model } = v.compaction
  if (v.compaction.mode === 'custom' && (typeof providerId !== 'string' || !providerId.trim() || typeof model !== 'string' || !model.trim() || model.length > 240 || /[\x00-\x1f]/.test(model))) throw new Error('Choose a compaction provider and model')
  return { contextWindows: { ...v.contextWindows }, compaction: { mode: v.compaction.mode, effort: v.compaction.effort, ...(typeof providerId === 'string' ? { providerId } : {}), ...(typeof model === 'string' ? { model: model.trim() } : {}) } }
}

/** Preserve the pre-setting GPT budget as an initial choice, independently per provider. */
function initialContextMode(provider: ProviderConfigView): ContextWindowMode {
  return provider.cli === 'codex' || provider.subscription?.provider === 'codex' || [provider.model, ...Object.values(provider.modelMapping ?? {})].some(model => /(?:^|\/)(?:gpt-|chatgpt-|o[134](?:-|$))/i.test(model ?? '')) ? '512k' : 'default'
}
