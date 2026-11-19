import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { SubscriptionAccounts, SubscriptionLaunchError } from './subscription-accounts'
import { RouterManagementClient } from './management-client'
import type { ModelRouterManager } from './manager'
import type { ProviderConfigStore } from '../provider-config/provider-config-store'
vi.mock('./codex-client', () => ({ CODEX_CLIENT_HEADERS: {}, credentialCarriesCodexIdentity: async () => true }))
vi.mock('./antigravity-client', () => ({ ANTIGRAVITY_CLIENT_USER_AGENT: 'fixture', credentialCarriesAntigravityIdentity: async () => true }))
let accounts: SubscriptionAccounts
let released: number, acquired: number
const row = () => ({ id: 'account', name: 'Fixture', modelMapping: { opus: 'mapped', sonnet: 'mapped', haiku: 'mapped' }, subscription: { provider: 'codex', credentialRef: { name: 'exact.json' }, account: { status: 'ready' }, modelMetadata: [{ id: 'mapped' }, { id: 'directory-model', contextWindow: 200000 }] } })
beforeEach(() => {
  released = 0; acquired = 0
  const manager = { onStateChange: () => () => {}, rememberModelAliases: () => {}, acquire: async () => { acquired++; return { connection: { baseUrl: 'http://127.0.0.1:9999', clientKey: 'fixture-client', managementKey: 'private-management' }, release: () => { released++ } } } }
  accounts = new SubscriptionAccounts({ dataRoot: '/unused-fixture', manager: manager as unknown as ModelRouterManager, store: { subscriptions: async () => [row()] } as unknown as ProviderConfigStore, openAuthorizationUrl: async () => {}, platform: 'linux' })
  vi.spyOn(accounts, 'initialize').mockResolvedValue()
  vi.spyOn(RouterManagementClient.prototype, 'credentials').mockResolvedValue([{ id: 'id', name: 'exact.json', auth_index: 'index', provider: 'codex', status: 'active' }])
  vi.spyOn(RouterManagementClient.prototype, 'models').mockResolvedValue([{ id: 'matou-chatgpt/directory-model' }])
  vi.spyOn(console, 'error').mockImplementation(() => {})
})
afterEach(async () => { await accounts.close(); vi.restoreAllMocks() })
it('prepares a raw catalog model outside all role mappings without changing shared aliases', async () => {
  const aliases = vi.spyOn(RouterManagementClient.prototype, 'setAliases')
  const target = await accounts.prepareModelTarget('account', 'directory-model')
  expect(target).toMatchObject({ baseUrl: 'http://127.0.0.1:9999', headers: { authorization: 'Bearer fixture-client' }, model: 'matou-chatgpt/directory-model', metadata: { id: 'directory-model', contextWindow: 200000 } })
  expect(aliases).not.toHaveBeenCalled()
  expect(released).toBe(0)
  target.lease.release(); expect(released).toBe(1)
})
it('rejects an unregistered catalog model and releases its lease', async () => {
  vi.mocked(RouterManagementClient.prototype.models).mockResolvedValue([])
  await expect(accounts.prepareModelTarget('account', 'directory-model')).rejects.toMatchObject({ targetFailure: 'registered-model-missing' })
  expect(released).toBe(acquired)
})
it('distinguishes a directory read failure from a missing model and releases its lease', async () => {
  vi.mocked(RouterManagementClient.prototype.models).mockRejectedValue(new Error('fixture directory unavailable'))
  await expect(accounts.prepareModelTarget('account', 'directory-model')).rejects.toMatchObject({ targetFailure: 'directory-unavailable' })
  expect(released).toBe(acquired)
})
it('never substitutes another credential with the same provider', async () => {
  vi.mocked(RouterManagementClient.prototype.credentials).mockResolvedValue([{ id: 'other', name: 'other.json', auth_index: 'other', provider: 'codex', status: 'active' }])
  await expect(accounts.prepareModelTarget('account', 'directory-model')).rejects.toMatchObject({ providerFailure: { status: 'needs-sign-in' } })
  expect(released).toBe(acquired)
})
it('releases preparation completed after close instead of returning a live target', async () => {
  let entered!: () => void, finish!: (value: Array<Record<string, unknown>>) => void
  const ready = new Promise<void>(r => { entered = r })
  vi.mocked(RouterManagementClient.prototype.models).mockImplementationOnce(() => { entered(); return new Promise(r => { finish = r }) })
  const pending = accounts.prepareModelTarget('account', 'directory-model')
  await ready
  const closing = accounts.close()
  finish([{ id: 'matou-chatgpt/directory-model' }])
  await expect(pending).rejects.toBeInstanceOf(SubscriptionLaunchError)
  await closing
  expect(released).toBe(acquired)
})
it.each(['', ' ', 'x\n', 'x\u007f', 'x'.repeat(241), 'matou-chatgpt/directory-model', 'missing'])('rejects invalid or absent raw catalog model %j without leaking a lease', async model => {
  await expect(accounts.prepareModelTarget('account', model)).rejects.toThrow()
  expect(released).toBe(acquired)
})

it('retains successful legacy launch projection through the shared preflight', async () => {
  const aliases = ['matou-opus', 'matou-sonnet', 'matou-haiku', 'matou-opus-fallback'].map(alias => ({ alias, name: 'mapped', fork: true as const }))
  vi.spyOn(RouterManagementClient.prototype, 'aliases').mockResolvedValue(aliases)
  vi.mocked(RouterManagementClient.prototype.models).mockResolvedValue(aliases.map(alias => ({ id: `matou-chatgpt/${alias.alias}` })))
  const prepared = await accounts.prepareLaunch('account')
  expect(prepared.input).toMatchObject({ models: { opus: { id: 'mapped' }, sonnet: { id: 'mapped' }, haiku: { id: 'mapped' } }, mainRole: 'opus', clientKey: 'fixture-client' })
  prepared.lease.release()
  expect(released).toBe(acquired)
})
it('adopts a cold raw-model prefix without changing shared role aliases', async () => {
  vi.mocked(RouterManagementClient.prototype.models).mockResolvedValueOnce([{ id: 'directory-model' }]).mockResolvedValue([{ id: 'matou-chatgpt/directory-model' }])
  const prefix = vi.spyOn(RouterManagementClient.prototype, 'setPrefix').mockResolvedValue()
  const aliases = vi.spyOn(RouterManagementClient.prototype, 'setAliases')
  const target = await accounts.prepareModelTarget('account', 'directory-model')
  expect(target.model).toBe('matou-chatgpt/directory-model')
  expect(prefix).toHaveBeenCalledWith('exact.json', 'matou-chatgpt')
  expect(aliases).not.toHaveBeenCalled()
  target.lease.release()
})
