import type { AgentModelPreferences, ProviderConfigView } from '@matou/contracts'
import type { PreparedSubscriptionLaunch, SubscriptionAccounts } from '../model-router/subscription-accounts'
import type { ProviderConfigStore } from '../provider-config/provider-config-store'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { claudeModelRouting, roleEffort, restoredProviderModel } from './claude-model-routing'

const credentialAccess = vi.hoisted(() => ({
  execFile: vi.fn(),
  readFile: vi.fn()
}))
vi.mock('node:child_process', () => ({ execFile: credentialAccess.execFile }))
vi.mock('node:fs/promises', () => ({ readFile: credentialAccess.readFile }))

const source: ProviderConfigView = {
  id: 'source', cli: 'claude-code', name: 'Source', endpoint: 'https://source.example',
  hasApiKey: true, model: 'claude-sonnet-model',
  modelMapping: { opus: 'claude-sonnet-model', sonnet: 'mapped-sonnet' },
  effortMapping: { opus: 'max', sonnet: 'low' }
}
const target: ProviderConfigView = {
  id: 'target', cli: 'claude-code', name: 'Target', endpoint: 'https://target.example',
  hasApiKey: true, model: 'summarizer'
}
const subscription: ProviderConfigView = {
  ...target, endpoint: '', hasApiKey: false, kind: 'subscription',
  subscription: { provider: 'codex', account: { status: 'ready' }, modelMetadata: [{ id: 'summarizer[1M]', maxOutputTokens: 4096 }] }
}

function storeFor(provider: ProviderConfigView, env: Record<string, string> = {}) {
  const methods = {
    snapshot: vi.fn<ProviderConfigStore['snapshot']>().mockResolvedValue({
      revision: 1, providers: { 'claude-code': [source, provider], codex: [] },
      activeProviderIds: { 'claude-code': source.id, codex: '' }
    }),
    launchSelection: vi.fn<ProviderConfigStore['launchSelection']>().mockResolvedValue({
      providerConfigId: provider.id, model: provider.model, env: { ANTHROPIC_BASE_URL: provider.endpoint, ...env }
    })
  }
  return { ...methods, store: methods as unknown as ProviderConfigStore }
}

function preferences(compaction: AgentModelPreferences['compaction']): AgentModelPreferences {
  return { contextWindows: {}, compaction }
}

beforeEach(() => vi.clearAllMocks())

describe('Claude model routing', () => {
  it('uses explicit API model mappings and subscription role aliases', () => {
    expect(roleEffort(source, 'claude-sonnet-model[1M]')).toBe('max')
    expect(roleEffort(source, 'mapped-sonnet')).toBe('low')
    expect(roleEffort({ ...source, subscription: subscription.subscription! }, 'matou-chatgpt/matou-sonnet')).toBe('low')
  })

  it('keeps follow-mode compaction on the source provider and applies its role effort', async () => {
    const { store, snapshot } = storeFor(target)
    const routing = claudeModelRouting({
      provider: source, store, preferences: preferences({ mode: 'follow', effort: 'follow' })
    }, {})
    expect(await routing.resolveOverride({ compaction: true, model: source.model })).toEqual({ effort: 'max' })
    expect(snapshot).not.toHaveBeenCalled()
    routing.close()
  })

  it('requires a target API key before looking up official credentials for a similar-looking domain', async () => {
    const { store } = storeFor({ ...target, endpoint: 'https://api.anthropic.com.target.example', hasApiKey: false })
    const routing = claudeModelRouting({
      provider: source, store,
      preferences: preferences({ mode: 'custom', providerId: target.id, model: target.model, effort: 'low' })
    }, {})
    await expect(routing.resolveOverride({ compaction: true, model: source.model })).rejects.toThrow(/API key/)
    expect(credentialAccess.readFile).not.toHaveBeenCalled()
    expect(credentialAccess.execFile).not.toHaveBeenCalled()
    routing.close()
  })

  it('uses the selected API credential and explicit effort only for compaction', async () => {
    const { store, launchSelection } = storeFor(target, { ANTHROPIC_API_KEY: 'target-key' })
    const routing = claudeModelRouting({
      provider: source, store,
      preferences: preferences({ mode: 'custom', providerId: target.id, model: target.model, effort: 'low' })
    }, { ANTHROPIC_API_KEY: 'source-key' })
    expect(await routing.resolveOverride({ compaction: false, model: source.model })).toEqual({ effort: 'max' })
    expect(launchSelection).not.toHaveBeenCalled()
    expect(await routing.resolveOverride({ compaction: true, model: source.model })).toEqual({
      baseUrl: target.endpoint, model: target.model, headers: { 'x-api-key': 'target-key' },
      effort: 'low', thinking: 'adaptive'
    })
    expect(launchSelection).toHaveBeenCalledExactlyOnceWith('claude-code', target.id)
    routing.close()
  })

  it('routes only compaction through the selected subscription and releases its lease', async () => {
    const { store, snapshot } = storeFor(subscription)
    const release = vi.fn()
    const connection = { baseUrl: 'http://127.0.0.1:9999', clientKey: 'target-router-key', managementKey: 'management', pid: 100 }
    const prepared: PreparedSubscriptionLaunch = {
      lease: { connection, release },
      input: {
        subscription: 'chatgpt', accountPrefix: 'matou-chatgpt', routerUrl: connection.baseUrl,
        clientKey: connection.clientKey, models: { opus: { id: 'summarizer' } }, mainRole: 'opus', providerName: 'Target'
      }
    }
    const prepareLaunch = vi.fn<SubscriptionAccounts['prepareLaunch']>().mockResolvedValue(prepared)
    const accounts = { prepareLaunch } as unknown as SubscriptionAccounts
    const routing = claudeModelRouting({
      provider: source, store, accounts,
      preferences: preferences({ mode: 'custom', providerId: subscription.id, model: 'summarizer[1M]', effort: 'follow' })
    }, {})
    const ordinary = { compaction: false, model: source.model }
    const compact = { compaction: true, model: source.model }
    expect(await routing.resolveOverride(ordinary)).toEqual({ effort: 'max' })
    expect(snapshot).not.toHaveBeenCalled()
    expect(await routing.resolveOverride(compact)).toEqual({
      baseUrl: connection.baseUrl, headers: { authorization: 'Bearer target-router-key' },
      model: 'matou-chatgpt/summarizer', maxOutputTokens: 4096, effort: 'max', thinking: 'adaptive'
    })
    await routing.resolveOverride(compact)
    expect(prepareLaunch).toHaveBeenCalledExactlyOnceWith(subscription.id, 'compaction')
    expect(await routing.resolveOverride(ordinary)).toEqual({ effort: 'max' })
    routing.close()
    routing.close()
    expect(release).toHaveBeenCalledOnce()
  })
})

it('retains a selected API role across resume, and resolves it if routing is disabled', async () => {
  const provider = { ...source, modelMapping: { opus: 'same', sonnet: 'same' }, effortMapping: { opus: 'high', sonnet: 'low' } } as const
  const prefs = preferences({ mode: 'follow', effort: 'follow' })
  expect(restoredProviderModel('matou-api/sonnet', 'default', provider, prefs)).toBe('matou-api/sonnet')
  expect(restoredProviderModel('matou-api/sonnet', 'default', { ...provider, effortMapping: {} }, prefs)).toBe('same')
  const { store } = storeFor(target)
  const routing = claudeModelRouting({ provider, preferences: prefs, store }, {})
  expect(await routing.resolveOverride({ compaction: false, model: 'matou-api/sonnet' })).toEqual({ model: 'same', effort: 'low' })
  expect(await routing.resolveOverride({ compaction: false, model: 'matou-api/opus' })).toEqual({ model: 'same', effort: 'high' })
  routing.close()
})
it('uses endpoint and key from the same selection when a provider changes during preparation', async () => {
  const { store } = storeFor(target, { ANTHROPIC_BASE_URL: 'https://replacement.example', ANTHROPIC_API_KEY: 'replacement-key' })
  const routing = claudeModelRouting({ provider: source, store, preferences: preferences({ mode: 'custom', providerId: target.id, model: 'summary', effort: 'low' }) }, {})
  expect(await routing.resolveOverride({ compaction: true, model: 'main' })).toMatchObject({ baseUrl: 'https://replacement.example', headers: { 'x-api-key': 'replacement-key' } })
  routing.close()
})
