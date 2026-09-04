import { randomUUID } from 'node:crypto'
import { chmod, mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import type {
  ProviderCli, ProviderConfigInput, ProviderConfigSnapshot, ProviderConfigView
} from '@matou/contracts'

interface StoredProvider {
  id: string
  cli: ProviderCli
  name: string
  endpoint: string
  model: string
  apiKey?: string
  builtIn?: boolean
}

interface StoredProviderState {
  version: 1
  revision: number
  providers: Record<ProviderCli, StoredProvider[]>
  activeProviderIds: Record<ProviderCli, string>
}

export interface ProviderLaunchConfig {
  model?: string
  env: Record<string, string>
}

export interface ProviderLaunchSelection {
  providerConfigId: string
  model: string
  env: Record<string, string>
}

const DEFAULT_STATE: StoredProviderState = {
  version: 1,
  revision: 1,
  providers: {
    'claude-code': [{
      id: 'anthropic-official', cli: 'claude-code', name: 'Anthropic 官方',
      endpoint: 'https://api.anthropic.com', model: '', builtIn: true
    }],
    codex: [{
      id: 'openai-official', cli: 'codex', name: 'OpenAI 官方',
      endpoint: 'https://api.openai.com/v1', model: '', builtIn: true
    }]
  },
  activeProviderIds: {
    'claude-code': 'anthropic-official', codex: 'openai-official'
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

  async upsert(input: ProviderConfigInput): Promise<ProviderConfigView> {
    return this.#mutate(async (state) => {
      const normalized = normalizeInput(input)
      const list = state.providers[normalized.cli]
      const existing = input.id ? list.find(({ id }) => id === input.id) : undefined
      const apiKey = normalized.apiKey || existing?.apiKey
      const provider: StoredProvider = {
        id: existing?.id ?? randomUUID(),
        cli: normalized.cli,
        name: normalized.name,
        endpoint: normalized.endpoint,
        model: normalized.model,
        ...(existing?.builtIn ? { builtIn: true } : {}),
        ...(apiKey ? { apiKey } : {})
      }
      if (input.id && !existing) throw new Error('供应商配置不存在')
      const index = list.findIndex(({ id }) => id === provider.id)
      if (index >= 0) list[index] = provider
      else list.push(provider)
      state.revision += 1
      return view(provider)
    })
  }

  async delete(cli: ProviderCli, providerId: string): Promise<ProviderConfigSnapshot> {
    return this.#mutate(async (state) => {
      const list = state.providers[cli]
      const provider = list.find(({ id }) => id === providerId)
      if (!provider) throw new Error('供应商配置不存在')
      if (state.activeProviderIds[cli] === providerId) throw new Error('使用中的供应商需要先切换后再删除')
      if (provider.builtIn) throw new Error('官方供应商配置保留为默认入口')
      state.providers[cli] = list.filter(({ id }) => id !== providerId)
      state.revision += 1
      return publicSnapshot(state)
    })
  }

  async activate(cli: ProviderCli, providerId: string): Promise<ProviderConfigSnapshot> {
    return this.#mutate(async (state) => {
      if (!state.providers[cli].some(({ id }) => id === providerId)) throw new Error('供应商配置不存在')
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
    if (!provider) throw new Error('会话绑定的供应商配置不存在')
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
      if (isMissing(error)) return structuredClone(DEFAULT_STATE)
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

function normalizeInput(input: ProviderConfigInput): ProviderConfigInput & { apiKey: string } {
  if (input.cli !== 'claude-code' && input.cli !== 'codex') throw new Error('CLI 类型不正确')
  const name = input.name.trim()
  const model = input.model.trim() === '__cli_default__' ? '' : input.model.trim()
  if (!name) throw new Error('供应商名称不能为空')
  if (!model && input.model.trim() !== '__cli_default__') throw new Error('默认模型不能为空')
  let endpoint: string
  try {
    const url = new URL(input.endpoint.trim())
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) throw new Error()
    endpoint = url.toString().replace(/\/$/, '')
  } catch {
    throw new Error('API 地址需要使用有效的 HTTP 或 HTTPS 地址')
  }
  return { ...input, name, model, endpoint, apiKey: input.apiKey?.trim() ?? '' }
}

function normalizeState(value: Partial<StoredProviderState>): StoredProviderState {
  if (value.version !== 1 || !value.providers || !value.activeProviderIds) {
    return structuredClone(DEFAULT_STATE)
  }
  const state = value as StoredProviderState
  for (const cli of ['claude-code', 'codex'] as const) {
    if (!Array.isArray(state.providers[cli]) || state.providers[cli].length === 0) {
      state.providers[cli] = structuredClone(DEFAULT_STATE.providers[cli])
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
    endpoint: provider.endpoint, model: provider.model || 'CLI 默认',
    hasApiKey: Boolean(provider.apiKey), ...(provider.builtIn ? { builtIn: true } : {})
  }
}

function isMissing(error: unknown): boolean {
  return typeof error === 'object' && error !== null && 'code' in error && error.code === 'ENOENT'
}
