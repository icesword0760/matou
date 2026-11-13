import { EventEmitter } from 'node:events'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { RouterManagementClient } from './management-client'
import type { ModelRouterManager } from './manager'
import { SubscriptionAccounts, SubscriptionPlatformError } from './subscription-accounts'

const cleanups: Array<() => Promise<void>> = []
afterEach(async () => { while (cleanups.length) await cleanups.pop()!(); vi.restoreAllMocks() })
async function fixture(systemVersion: string) {
  const output = resolve(import.meta.dirname, '../../../../output/model-router-platform')
  await mkdir(output, { recursive: true })
  const dataRoot = await mkdtemp(join(output, 'os-'))
  const store = new ProviderConfigStore(dataRoot)
  const lease = { connection: { baseUrl: 'http://127.0.0.1:1', clientKey: 'fixture', managementKey: 'fixture', pid: 1 }, release: vi.fn() }
  const manager = {
    acquire: vi.fn(async (_scope: string) => lease),
    openSettings: vi.fn(async (_id: string) => lease),
    releaseConnection: vi.fn(), rememberModelAliases: vi.fn(),
    onStateChange: vi.fn(() => vi.fn())
  }
  const accounts = new SubscriptionAccounts({ dataRoot, store, manager: manager as unknown as ModelRouterManager,
    platform: 'darwin', systemVersion: () => systemVersion, openAuthorizationUrl: vi.fn() })
  const port = new EventEmitter(), connection = accounts.bindConnection(port)
  cleanups.push(async () => { await accounts.close(); await rm(dataRoot, { recursive: true, force: true }) })
  return { store, manager, accounts, connection, lease }
}
async function migratedCard(store: ProviderConfigStore) {
  return store.adoptSubscription({ name: 'Migrated ChatGPT', modelMapping: { opus: 'gpt-test', sonnet: 'gpt-test', haiku: 'gpt-test' },
    subscription: { provider: 'codex', credentialRef: { name: 'migrated.json' }, account: { status: 'ready', plan: 'Plus' },
      modelMetadata: [{ id: 'gpt-test', displayName: 'Saved model', contextWindow: 200000 }] } })
}

describe('subscription macOS support boundary', () => {
  it.each([['14.0', false], ['14.7', false], ['15.0', true]] as const)(
    'publishes the runtime capability for injected macOS %s without a user agent', async (version, supported) => {
      const f = await fixture(version)
      vi.spyOn(RouterManagementClient.prototype, 'credentials').mockResolvedValue([])
      expect((await f.accounts.snapshot()).subscriptionSupported).toBe(supported)
      const settings = await f.connection.settings()
      expect(settings.snapshot.subscriptionSupported).toBe(supported)
      expect(f.manager.openSettings).toHaveBeenCalledTimes(supported ? 1 : 0)
      expect(f.manager.acquire).not.toHaveBeenCalled()
    })
  it('keeps migrated cards and saved model metadata while skipping startup reconciliation and quota reads on macOS 14', async () => {
    const f = await fixture('14.7'), card = await migratedCard(f.store)
    await f.accounts.initialize()
    const result = await f.connection.settings()
    expect(result.snapshot.providers['claude-code']).toContainEqual(expect.objectContaining({ id: card.id, name: 'Migrated ChatGPT', subscription: expect.objectContaining({ account: { status: 'ready', plan: 'Plus' } }) }))
    expect(result.accounts[card.id]?.models).toEqual([{ id: 'gpt-test', displayName: 'Saved model', contextWindow: 200000 }])
    expect(f.manager.acquire).not.toHaveBeenCalled()
    expect(f.manager.openSettings).not.toHaveBeenCalled()
    expect(f.manager.rememberModelAliases).not.toHaveBeenCalled()
  })
  it.each(['zh-CN', 'en'] as const)('rejects login and migrated-card launch before acquiring a router on macOS 14 in %s', async locale => {
    const f = await fixture('14.7'), card = await migratedCard(f.store)
    const before = await f.store.snapshot()
    const message = locale === 'en' ? 'Requires macOS 15 or later' : '需要 macOS 15 或更高版本'
    await expect(f.connection.start({ provider: 'codex', locale })).rejects.toThrow(message)
    await expect(f.connection.start({ provider: 'antigravity', locale })).rejects.toThrow(message)
    await expect(f.accounts.prepareLaunch(card.id, 'subscription-activation-preparation')).rejects.toBeInstanceOf(SubscriptionPlatformError)
    await expect(f.connection.models({ providerId: card.id })).rejects.toBeInstanceOf(SubscriptionPlatformError)
    expect(await f.store.snapshot()).toEqual(before)
    expect(f.manager.acquire).not.toHaveBeenCalled()
    expect(f.manager.openSettings).not.toHaveBeenCalled()
  })
  it('keeps API-key configuration and deletion available on macOS 14 without starting the subscription router', async () => {
    const f = await fixture('14.0')
    const api = await f.store.upsert({ cli: 'claude-code', name: 'Local API', endpoint: 'https://example.invalid', apiKey: 'fixture-key', model: 'fixture' })
    const result = await f.accounts.delete('claude-code', api.id)
    expect(result.providers['claude-code'].some(row => row.id === api.id)).toBe(false)
    expect(result.providers['claude-code'].some(row => row.builtIn)).toBe(true)
    expect(f.manager.acquire).not.toHaveBeenCalled()
  })
  it('continues the normal preparation path on macOS 15.0', async () => {
    const f = await fixture('15.0')
    await f.accounts.initialize()
    const card = await migratedCard(f.store)
    vi.spyOn(RouterManagementClient.prototype, 'credentials').mockResolvedValue([])
    const log = vi.spyOn(console, 'error').mockImplementation(() => {})
    await expect(f.accounts.prepareLaunch(card.id)).rejects.toMatchObject({ providerFailure: { status: 'needs-sign-in' } })
    expect(f.manager.acquire).toHaveBeenCalledWith('subscription-session')
    expect(f.lease.release).toHaveBeenCalledOnce()
    log.mockRestore()
  })
})
