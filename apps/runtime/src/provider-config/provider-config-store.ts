import { randomUUID } from 'node:crypto'
import { chmod, mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import {
  CLI_DEFAULT_MODEL,
  type ProviderModelMetadata, type ProviderModelsInput, type ProviderModelsResult, type ProviderCli, type ProviderConfigInput, type ApiKeyProviderConfigInput, type ProviderConfigSnapshot, type ProviderConfigView,
  type SubscriptionProvider, type SubscriptionModel, type SubscriptionAccountSummary
} from '@matou/contracts'

import { toClientModelId } from '../model-router/claude-launch'
import { discoverProviderModels, normalizeProviderModelMetadata } from './provider-models'

import { EntityMissingError } from '../errors'
import { runtimeMessages } from '../i18n/messages'

interface StoredProvider {
  id: string
  cli: ProviderCli
  name: string
  endpoint: string
  model: string
  apiKey?: string
  modelMetadata?: ProviderModelMetadata[]
  reasoningEffort?: ProviderConfigView['reasoningEffort']
  effortMapping?: ProviderConfigView['effortMapping']
  modelMapping?: ProviderConfigInput['modelMapping']
  builtIn?: boolean
  kind?: 'api-key' | 'subscription'
  subscription?: StoredSubscription
}

export interface CredentialRef { name: string; authIndex?: string }
export interface StoredSubscription {
  provider: SubscriptionProvider
  credentialRef: CredentialRef
  account: SubscriptionAccountSummary
  modelMetadata: SubscriptionModel[]
}
export interface StoredSubscriptionProvider {
  id: string
  name: string
  effortMapping?: ProviderConfigView['effortMapping']
  modelMapping: Record<'opus' | 'sonnet' | 'haiku', string>
  subscription: StoredSubscription
}

interface StoredProviderState {
  version: 1
  revision: number
  providers: Record<ProviderCli, StoredProvider[]>
  activeProviderIds: Record<ProviderCli, string>
  policies?: { antigravityRiskAcceptedAt?: string }
}

export interface ProviderLaunchConfig {
  model?: string
  env: Record<string, string>
}

export interface ProviderLaunchSelection {
  subscription?: StoredSubscriptionProvider
  providerConfigId: string
  model: string
  env: Record<string, string>
}

/** Built fresh on every call so the seed names follow the current locale. */
function defaultState(): StoredProviderState {
  const messages = runtimeMessages().providerConfig
  return {
    version: 1,
    revision: 1,
    providers: {
      'claude-code': [{
        id: 'anthropic-official', cli: 'claude-code', name: messages.builtIn.anthropic,
        endpoint: 'https://api.anthropic.com', model: '', builtIn: true
      }],
      codex: [{
        id: 'openai-official', cli: 'codex', name: messages.builtIn.openai,
        endpoint: 'https://api.openai.com/v1', model: '', builtIn: true
      }]
    },
    activeProviderIds: {
      'claude-code': 'anthropic-official', codex: 'openai-official'
    }
  }
}

export class ProviderConfigStore {
  readonly #root: string
  readonly #path: string
  #writeChain: Promise<unknown> = Promise.resolve()

  constructor(dataRoot: string) {
    this.#root = dataRoot
    this.#path = join(dataRoot, 'provider-config.json')
  }

  async snapshot(): Promise<ProviderConfigSnapshot> {
    return publicSnapshot(await this.#read())
  }

  async discoverModels(input: ProviderModelsInput): Promise<ProviderModelsResult> {
    if (!['claude-code', 'codex'].includes(input.cli)) return { error: 'invalid' }
    const state = await this.#read()
    const saved = input.providerId ? state.providers[input.cli].find(p => p.id === input.providerId) : undefined
    // A retained secret may only be sent to the exact saved endpoint.
    const sameEndpoint = saved?.endpoint.replace(/\/$/, '') === input.endpoint.trim().replace(/\/$/, '')
    const apiKey = input.apiKey?.trim() || (sameEndpoint ? saved?.apiKey : undefined)
    if (!apiKey) return { error: 'key-required' }
    return discoverProviderModels({ ...input, apiKey })
  }

  async upsert(input: ProviderConfigInput): Promise<ProviderConfigView> {
    if (input.kind === 'subscription') throw new Error('Subscription requires a completed sign-in')
    return this.#mutate(async (state) => {
      const normalized = normalizeInput(input)
      const list = state.providers[normalized.cli]
      const existing = input.id ? list.find(({ id }) => id === input.id) : undefined
      if (existing?.kind === 'subscription') throw new Error('Cannot change a subscription into an API key provider')
      if (existing?.apiKey && normalized.endpoint !== existing.endpoint && !normalized.apiKey) throw new Error('Endpoint changed: enter the API key for the new service')
      const apiKey = normalized.apiKey || existing?.apiKey
      const sameService = existing?.endpoint === normalized.endpoint && existing.apiKey === apiKey
      const modelMetadata = normalizeProviderModelMetadata(normalized.modelMetadata) ?? (sameService ? existing.modelMetadata : undefined)
      const provider: StoredProvider = {
        ...existing,
        id: existing?.id ?? randomUUID(),
        cli: normalized.cli,
        name: normalized.name,
        endpoint: normalized.endpoint,
        model: normalized.model,
        ...(normalized.reasoningEffort ? { reasoningEffort: normalized.reasoningEffort } : {}),
        ...(normalized.effortMapping ? { effortMapping: normalized.effortMapping } : {}),
        ...((normalized.modelMapping ?? existing?.modelMapping) ? { modelMapping: normalized.modelMapping ?? existing!.modelMapping! } : {}),
        ...(existing?.builtIn ? { builtIn: true } : {}),
        ...(apiKey ? { apiKey } : {})
      }
      if (modelMetadata === undefined) delete provider.modelMetadata
      else provider.modelMetadata = modelMetadata
      if (input.id && !existing) {
        throw new EntityMissingError(runtimeMessages().providerConfig.notFound)
      }
      const index = list.findIndex(({ id }) => id === provider.id)
      if (index >= 0) list[index] = provider
      else list.push(provider)
      state.revision += 1
      return view(provider)
    })
  }

  async subscriptions(): Promise<StoredSubscriptionProvider[]> {
    const state = await this.#read()
    return state.providers['claude-code'].filter(provider => provider.kind === 'subscription' && provider.subscription)
      .map(provider => ({ id: provider.id, name: provider.name, ...(provider.effortMapping ? { effortMapping: provider.effortMapping } : {}), modelMapping: { opus: provider.modelMapping?.opus ?? '', sonnet: provider.modelMapping?.sonnet ?? '', haiku: provider.modelMapping?.haiku ?? '' }, subscription: structuredClone(provider.subscription!) }))
  }

  async cacheSubscriptionModels(id: string, credentialName: string, models: SubscriptionModel[]): Promise<void> {
    if (!models.length) return
    await this.#mutate(async state => {
      const row = state.providers['claude-code'].find(p => p.id === id && p.subscription?.credentialRef.name === credentialName)
      if (!row?.subscription || JSON.stringify(row.subscription.modelMetadata) === JSON.stringify(models)) return
      row.subscription.modelMetadata = structuredClone(models)
      state.revision++
    })
  }

  async antigravityRiskAccepted(): Promise<boolean> { return Boolean((await this.#read()).policies?.antigravityRiskAcceptedAt) }
  async confirmAntigravityRisk(): Promise<{ accepted: true }> {
    return this.#mutate(async state => {
      state.policies = { ...state.policies, antigravityRiskAcceptedAt: state.policies?.antigravityRiskAcceptedAt ?? new Date().toISOString() }
      state.revision += 1
      return { accepted: true as const }
    })
  }

  /** Internal commit only. Renderer supplies an attempt, never a credentialRef. */
  async adoptSubscription(input: Omit<StoredSubscriptionProvider, 'id'> & { id?: string }): Promise<ProviderConfigView> {
    return this.#mutate(async state => {
      validateEffortSettings(input)
      const provider = input.subscription.provider
      if (provider !== 'codex' && provider !== 'antigravity') throw new Error('Invalid subscription provider')
      if (provider === 'antigravity' && !state.policies?.antigravityRiskAcceptedAt) throw new Error('Antigravity risk confirmation is required')
      if (!input.name.trim() || input.name.length > 256) throw new Error('Invalid provider name')
      if (!/^[^/\\\0]+\.json$/.test(input.subscription.credentialRef.name)) throw new Error('Invalid credential reference')
      for (const role of ['opus', 'sonnet', 'haiku'] as const) {
        if (!input.subscription.modelMetadata.some(model => model.id === input.modelMapping[role])) throw new Error('Selected model is not available')
      }
      const list = state.providers['claude-code']
      const existing = list.find(value => value.kind === 'subscription' && value.subscription?.provider === provider)
      if (input.id && input.id !== existing?.id) throw new Error('Subscription identity does not match')
      const subscription: StoredSubscription = {
        ...existing?.subscription,
        provider,
        credentialRef: { name: input.subscription.credentialRef.name, ...(input.subscription.credentialRef.authIndex ? { authIndex: input.subscription.credentialRef.authIndex } : {}) },
        account: {
          status: input.subscription.account.status,
          ...(input.subscription.account.email ? { email: input.subscription.account.email } : {}),
          ...(input.subscription.account.plan ? { plan: input.subscription.account.plan } : {})
        },
        modelMetadata: input.subscription.modelMetadata.map(model => ({
          id: model.id, ...(model.displayName ? { displayName: model.displayName } : {}),
          ...(model.ownedBy ? { ownedBy: model.ownedBy } : {}),
          ...(model.contextWindow ? { contextWindow: model.contextWindow } : {}),
          ...(model.maxContextWindow ? { maxContextWindow: model.maxContextWindow } : {}),
          ...(model.supportedEfforts ? { supportedEfforts: model.supportedEfforts } : {}),
          ...(model.maxOutputTokens ? { maxOutputTokens: model.maxOutputTokens } : {})
        }))
      }
      const value: StoredProvider = { ...existing, id: existing?.id ?? randomUUID(), cli: 'claude-code', kind: 'subscription', name: input.name.trim(), endpoint: '', model: input.modelMapping.sonnet, modelMapping: { ...input.modelMapping }, ...(input.effortMapping ? { effortMapping: input.effortMapping } : {}), subscription }
      delete value.apiKey
      const index = list.findIndex(item => item.id === value.id)
      if (index === -1) list.push(value)
      else list[index] = value
      state.revision += 1
      return view(value)
    })
  }

  async delete(cli: ProviderCli, providerId: string): Promise<ProviderConfigSnapshot> {
    return this.#mutate(async (state) => {
      const list = state.providers[cli]
      const provider = list.find(({ id }) => id === providerId)
      if (!provider) throw new EntityMissingError(runtimeMessages().providerConfig.notFound)
      if (state.activeProviderIds[cli] === providerId) {
        throw new Error(runtimeMessages().providerConfig.activeCannotBeDeleted)
      }
      if (provider.builtIn) throw new Error(runtimeMessages().providerConfig.builtInKept)
      state.providers[cli] = list.filter(({ id }) => id !== providerId)
      state.revision += 1
      return publicSnapshot(state)
    })
  }

  async activate(cli: ProviderCli, providerId: string): Promise<ProviderConfigSnapshot> {
    return this.#mutate(async (state) => {
      if (!state.providers[cli].some(({ id }) => id === providerId)) {
        throw new EntityMissingError(runtimeMessages().providerConfig.notFound)
      }
      if (state.activeProviderIds[cli] !== providerId) {
        state.activeProviderIds[cli] = providerId
        state.revision += 1
      }
      return publicSnapshot(state)
    })
  }

  async launchConfig(cli: ProviderCli): Promise<ProviderLaunchConfig> {
    const selected = await this.launchSelection(cli)
    return {
      ...(selected.model ? { model: selected.model } : {}),
      env: selected.env
    }
  }

  async launchSelection(
    cli: ProviderCli,
    providerConfigId?: string
  ): Promise<ProviderLaunchSelection> {
    const state = await this.#read()
    const selectedId = providerConfigId ?? state.activeProviderIds[cli]
    const provider = state.providers[cli].find(({ id }) => id === selectedId)
    if (!provider) {
      throw new EntityMissingError(runtimeMessages().providerConfig.sessionProviderNotFound)
    }
    if (provider.kind === 'subscription' && provider.subscription) {
      const subscription = (await this.subscriptions()).find(row => row.id === provider.id)!
      const model = subscription.subscription.modelMetadata.find(model => model.id === subscription.modelMapping.opus)
      if (!model) throw new Error('Subscription Opus model metadata is missing')
      const kind = subscription.subscription.provider === 'codex' ? 'chatgpt' : 'antigravity'
      return { providerConfigId: provider.id, model: `matou-${kind}/${toClientModelId(kind, 'opus', model)}`, env: {}, subscription }
    }
    const endpoint = provider.endpoint.replace(/\/$/, '')
    if (cli === 'claude-code') {
      return {
        providerConfigId: provider.id,
        model: provider.model,
        env: {
          ...(provider.apiKey ? {
            ANTHROPIC_API_KEY: provider.apiKey,
            ANTHROPIC_AUTH_TOKEN: provider.apiKey
          } : {}),
          ...Object.fromEntries(Object.entries(provider.modelMapping ?? {}).filter(([, value]) => value).map(([role, value]) => [`ANTHROPIC_DEFAULT_${role.toUpperCase()}_MODEL`, value as string])),
          ...(endpoint ? { ANTHROPIC_BASE_URL: endpoint } : {})
        }
      }
    }
    return {
      providerConfigId: provider.id,
      model: provider.model,
      env: {
        ...(provider.apiKey ? { OPENAI_API_KEY: provider.apiKey } : {}),
        ...(endpoint ? { OPENAI_BASE_URL: endpoint } : {})
      }
    }
  }

  async #mutate<T>(operation: (state: StoredProviderState) => Promise<T>): Promise<T> {
    const result = this.#writeChain.then(async () => {
      const state = await this.#read()
      const value = await operation(state)
      await this.#write(state)
      return value
    })
    this.#writeChain = result.catch(() => undefined)
    return result
  }

  async #read(): Promise<StoredProviderState> {
    try {
      const parsed = JSON.parse(await readFile(this.#path, 'utf8')) as Partial<StoredProviderState>
      return normalizeState(parsed)
    } catch (error) {
      if (isMissing(error)) return defaultState()
      throw error
    }
  }

  async #write(state: StoredProviderState): Promise<void> {
    await mkdir(this.#root, { recursive: true, mode: 0o700 })
    const temporary = `${this.#path}.tmp`
    await writeFile(temporary, `${JSON.stringify(state, null, 2)}\n`, { encoding: 'utf8', mode: 0o600 })
    await rename(temporary, this.#path)
    await chmod(this.#path, 0o600)
  }
}

function normalizeInput(input: ApiKeyProviderConfigInput): ApiKeyProviderConfigInput & { apiKey: string } {
  validateEffortSettings(input)
  if (input.cli !== 'claude-code' && input.cli !== 'codex') {
    throw new Error(runtimeMessages().providerConfig.invalidCli)
  }
  const name = input.name.trim()
  const model = input.model.trim() === '__cli_default__' ? '' : input.model.trim()
  if (!name) throw new Error(runtimeMessages().providerConfig.nameRequired)
  if (!model && input.model.trim() !== '__cli_default__') {
    throw new Error(runtimeMessages().providerConfig.modelRequired)
  }
  let endpoint: string
  try {
    const url = new URL(input.endpoint.trim())
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) throw new Error()
    endpoint = url.toString().replace(/\/$/, '')
  } catch {
    throw new Error(runtimeMessages().providerConfig.endpointInvalid)
  }
  const apiKey = input.apiKey?.trim() ?? ''
  if (apiKey && !/^[\x21-\x7e]+$/.test(apiKey)) throw new Error('API Key contains invalid characters')
  const modelMapping = input.modelMapping === undefined ? undefined : Object.fromEntries(
    ['opus', 'sonnet', 'haiku'].map(role => {
      const value = input.modelMapping?.[role as 'opus']
      if (value !== undefined && typeof value !== 'string') throw new Error('Invalid model mapping')
      return [role, value?.trim() ?? '']
    })
  )
  return { ...input, name, model, endpoint, apiKey, ...(modelMapping ? { modelMapping } : {}) }
}

function normalizeState(value: Partial<StoredProviderState>): StoredProviderState {
  if (value.version !== 1 || !value.providers || !value.activeProviderIds) {
    return defaultState()
  }
  const state = value as StoredProviderState
  const seed = defaultState()
  for (const cli of ['claude-code', 'codex'] as const) {
    if (!Array.isArray(state.providers[cli]) || state.providers[cli].length === 0) {
      state.providers[cli] = seed.providers[cli]
    }
    for (const provider of state.providers[cli]) {
      if (provider.modelMetadata === undefined) continue
      const modelMetadata = normalizeProviderModelMetadata(provider.modelMetadata)
      if (modelMetadata === undefined) delete provider.modelMetadata
      else provider.modelMetadata = modelMetadata
    }
    if (!state.providers[cli].some(({ id }) => id === state.activeProviderIds[cli])) {
      state.activeProviderIds[cli] = state.providers[cli][0]!.id
    }
  }
  return state
}

function publicSnapshot(state: StoredProviderState): ProviderConfigSnapshot {
  return {
    revision: state.revision,
    activeProviderIds: { ...state.activeProviderIds },
    providers: {
      'claude-code': state.providers['claude-code'].map(view),
      codex: state.providers.codex.map(view)
    }
  }
}

function view(provider: StoredProvider): ProviderConfigView {
  return {
    id: provider.id, cli: provider.cli, name: provider.name,
    endpoint: provider.endpoint, model: provider.model || CLI_DEFAULT_MODEL,
    ...(provider.modelMetadata !== undefined ? { modelMetadata: provider.modelMetadata } : {}),
    ...(provider.modelMapping ? { modelMapping: provider.modelMapping } : {}),
    ...(provider.effortMapping ? { effortMapping: provider.effortMapping } : {}),
    ...(provider.reasoningEffort ? { reasoningEffort: provider.reasoningEffort } : {}),
    hasApiKey: Boolean(provider.apiKey), ...(provider.builtIn ? { builtIn: true } : {}),
    ...(provider.kind === 'subscription' && provider.subscription ? {
      kind: 'subscription' as const,
      subscription: {
        provider: provider.subscription.provider,
        account: { ...provider.subscription.account },
        modelMetadata: provider.subscription.modelMetadata.map(model => ({ ...model }))
      }
    } : {})
  }
}

function isMissing(error: unknown): boolean {
  return typeof error === 'object' && error !== null && 'code' in error && error.code === 'ENOENT'
}

export function validateEffortSettings(input: { effortMapping?: ProviderConfigView['effortMapping']; reasoningEffort?: ProviderConfigView['reasoningEffort'] }): void {
  const levels = ['follow', 'low', 'medium', 'high', 'xhigh', 'max']
  if (input.reasoningEffort !== undefined && !levels.includes(input.reasoningEffort)) throw new Error('Invalid reasoning effort')
  if (input.effortMapping !== undefined && (!input.effortMapping || typeof input.effortMapping !== 'object' || Array.isArray(input.effortMapping) || Object.entries(input.effortMapping).some(([role, value]) => !['opus', 'sonnet', 'haiku'].includes(role) || !levels.includes(value)))) throw new Error('Invalid role reasoning effort')
}
