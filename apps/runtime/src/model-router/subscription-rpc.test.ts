import { EventEmitter } from 'node:events'
import { mkdir, mkdtemp } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { PROTOCOL_VERSION, type RuntimeMessage, type RpcMethod } from '@matou/contracts'
import { RuntimeServer } from '../runtime-server'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { RuntimeSessionRegistry } from '../session/runtime-session-registry'
import { RuntimeAccessPolicy } from '../storage/runtime-access-policy'
import { SubscriptionLaunchError, SubscriptionPlatformError, type SubscriptionAccounts, type SubscriptionConnection } from './subscription-accounts'

class Port extends EventEmitter {
  sent: RuntimeMessage[] = []
  postMessage(message: RuntimeMessage) { this.sent.push(message) }
  start() {}
  close() { this.emit('close') }
  send(data: unknown) { this.emit('message', { data }) }
}
async function fixture(readOnly = false) {
  const output = resolve(import.meta.dirname, '../../../../output/model-router-stage3'); await mkdir(output, { recursive: true })
  const root = await mkdtemp(join(output, 'rpc-')), database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const connection = { start: vi.fn(async () => ({ attemptId: 'attempt', status: 'waiting' })), reopen: vi.fn(async () => ({ attemptId: 'attempt', status: 'waiting' })), status: vi.fn(() => ({ attemptId: 'attempt', status: 'ready' })), cancel: vi.fn(async () => ({ status: 'cancelled' })), models: vi.fn(async () => ({ account: { status: 'ready' }, models: [] })), settings: vi.fn(async () => ({ accounts: {} })), closeSettings: vi.fn(async () => {}), confirmRisk: vi.fn(async () => ({ accepted: true })), save: vi.fn(async () => ({ id: 'subscription' })) }
  let changed = () => {}
  const unsubscribe = vi.fn(() => { changed = () => {} })
  const accounts = { onChange: vi.fn((listener: () => void) => { changed = listener; return unsubscribe }), prepareLaunch: vi.fn(async (_providerId: string, _scope?: string) => { throw new SubscriptionLaunchError('fixture preparation failed access_token=private-value') }), snapshot: vi.fn(async () => ({ providers: {} })), delete: vi.fn(async () => ({ providers: {} })) }
  const store = new ProviderConfigStore(root), sessions = new RuntimeSessionRegistry()
  const port = new Port(), server = new RuntimeServer(port, root, database, undefined, undefined, sessions, undefined, undefined, { providerConfigs: store, subscriptionAccounts: accounts as unknown as SubscriptionAccounts, subscriptionConnection: connection as unknown as SubscriptionConnection, accessPolicy: new RuntimeAccessPolicy(readOnly ? 'read-only' : 'normal') })
  port.send({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'settings-window' })
  let next = 0
  async function rpc(method: RpcMethod, payload: unknown = {}) {
    const requestId = `rpc-${next++}`
    port.send({ type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId, capability: 'renderer', method, payload, deadlineAt: Date.now() + 5000 })
    await vi.waitFor(() => expect(port.sent.some(message => (message.type === 'rpc.response' || message.type === 'rpc.error') && message.requestId === requestId)).toBe(true))
    return port.sent.find(message => (message.type === 'rpc.response' || message.type === 'rpc.error') && message.requestId === requestId)!
  }
  return { connection, accounts, store, sessions, port, changed: () => changed(), unsubscribe, rpc, close: () => { server.close(); database.close() } }
}
afterEach(() => { vi.unstubAllEnvs(); vi.restoreAllMocks() })
it.each(['zh-CN', 'en'])('keeps compatible failure strings while exporting bilingual reasons in %s', locale => {
  vi.stubEnv('MATOU_LOCALE', locale)
  const logger = vi.spyOn(console, 'error').mockImplementation(() => {})
  for (const kind of ['needs-sign-in', 'models-not-ready', 'unsupported-context-window', 'router-start-failed'] as const) {
    const error = new SubscriptionLaunchError('Fixture diagnostic access_token=private-value', kind)
    const failure = error.providerFailure
    const key = locale === 'en' ? 'en' : 'zh'
    expect(error.message).toBe(failure.message[key])
    expect(failure.reason === undefined || typeof failure.reason === 'string').toBe(true)
    if (failure.reasonLocalized) {
      expect(failure.reason).toBe(failure.reasonLocalized[key])
      expect(failure.message[key]).toContain(failure.reasonLocalized[key])
    }
    expect(failure.status).toBe(kind === 'needs-sign-in' ? 'needs-sign-in' : 'router-start-failed')
    expect(failure.recovery).toBe(kind === 'needs-sign-in' ? 'sign-in' : 'retry')
    expect(JSON.stringify(failure)).not.toMatch(/Fixture|private-value|unsupported-context-window|access_token/)
  }
  expect(logger.mock.calls.map(row => row.join(' ')).join(' ')).toContain('Fixture diagnostic')
  expect(logger.mock.calls.map(row => row.join(' ')).join(' ')).not.toContain('private-value')
})
describe('subscription RPC through the actual runtime message dispatcher', () => {
  it.each(['zh-CN', 'en'] as const)('returns the macOS requirement unchanged before activation or login in %s', async locale => {
    vi.stubEnv('MATOU_LOCALE', locale)
    const f = await fixture()
    try {
      const provider = await f.store.adoptSubscription({ name: 'Migrated subscription', modelMapping: { opus: 'gpt-test', sonnet: 'gpt-test', haiku: 'gpt-test' }, subscription: { provider: 'codex', credentialRef: { name: 'migrated.json' }, account: { status: 'ready' }, modelMetadata: [{ id: 'gpt-test' }] } })
      f.accounts.prepareLaunch.mockRejectedValueOnce(new SubscriptionPlatformError(locale))
      f.connection.start.mockRejectedValueOnce(new SubscriptionPlatformError(locale))
      for (const [method, payload] of [
        ['provider-config.activate', { cli: 'claude-code', providerId: provider.id }],
        ['provider-config.subscription.login-start', { provider: 'codex', locale }]
      ] as const) {
        expect(await f.rpc(method, payload)).toMatchObject({ type: 'rpc.error', retryable: false,
          message: locale === 'en' ? 'Requires macOS 15 or later' : '需要 macOS 15 或更高版本',
          messageLocalized: { zh: '需要 macOS 15 或更高版本', en: 'Requires macOS 15 or later' } })
      }
      expect((await f.store.snapshot()).activeProviderIds['claude-code']).toBe('anthropic-official')
    } finally { f.close() }
  })
  it.each(['zh-CN', 'en'] as const)('returns controlled bilingual login-start failures in %s and allows a new attempt', async locale => {
    const f = await fixture()
    const logger = vi.spyOn(console, 'error').mockImplementation(() => {})
    try {
      f.connection.start.mockRejectedValueOnce(new Error('Model router failed to start access_token=private-value'))
      const result = await f.rpc('provider-config.subscription.login-start', { provider: 'codex', locale })
      expect(result).toMatchObject({ type: 'rpc.error', retryable: true, messageLocalized: {
        zh: '模型路由未能启动：本地模型路由暂时无法使用，请重试。',
        en: 'Model router failed to start: The local model router is temporarily unavailable. Try again.'
      } })
      if (result.type !== 'rpc.error') throw new Error('Expected sign-in error')
      expect(result.message).toBe(result.messageLocalized![locale === 'en' ? 'en' : 'zh'])
      expect(JSON.stringify(result)).not.toMatch(/access_token|private-value/)
      expect((await f.rpc('provider-config.subscription.login-start', { provider: 'codex', locale })).type).toBe('rpc.response')
      expect(f.connection.start).toHaveBeenCalledTimes(2)
      expect(logger.mock.calls.flat().join(' ')).not.toContain('private-value')
    } finally { f.close() }
  })
  it('routes each subscription operation to the connection owner and uses coordinated card save/delete', async () => {
    const f = await fixture()
    try {
      const prefix = 'provider-config.subscription.'
      expect((await f.rpc(`${prefix}login-start` as RpcMethod, { provider: 'codex' })).type).toBe('rpc.response')
      await f.rpc(`${prefix}login-reopen` as RpcMethod, { attemptId: 'attempt' })
      await f.rpc(`${prefix}login-status` as RpcMethod, { attemptId: 'attempt' })
      await f.rpc(`${prefix}login-cancel` as RpcMethod, { attemptId: 'attempt' })
      await f.rpc(`${prefix}models-read` as RpcMethod, { attemptId: 'attempt' })
      await f.rpc(`${prefix}settings-read` as RpcMethod)
      await f.rpc(`${prefix}settings-close` as RpcMethod)
      await f.rpc(`${prefix}risk-confirm` as RpcMethod, { accepted: true })
      const input = { kind: 'subscription', cli: 'claude-code', name: 'ChatGPT', subscription: { provider: 'codex', attemptId: 'attempt' }, modelMapping: { opus: 'model', sonnet: 'model', haiku: 'model' } }
      await f.rpc('provider-config.upsert', { provider: input })
      await f.rpc('provider-config.delete', { cli: 'claude-code', providerId: 'subscription' })
      await f.rpc('provider-config.snapshot')
      expect(f.connection.start).toHaveBeenCalledWith({ provider: 'codex' })
      expect(f.connection.status).toHaveBeenCalledWith('attempt')
      expect(f.connection.reopen).toHaveBeenCalledWith('attempt')
      expect(f.connection.cancel).toHaveBeenCalledWith('attempt')
      expect(f.connection.models).toHaveBeenCalledWith({ attemptId: 'attempt' })
      expect(f.connection.settings).toHaveBeenCalledOnce()
      expect(f.connection.closeSettings).toHaveBeenCalledOnce()
      expect(f.connection.confirmRisk).toHaveBeenCalledWith(true)
      expect(f.connection.save).toHaveBeenCalledWith(input)
      expect(f.accounts.delete).toHaveBeenCalledWith('claude-code', 'subscription')
      expect(f.accounts.snapshot).toHaveBeenCalledOnce()
      expect((await f.rpc(`${prefix}login-status` as RpcMethod, { attemptId: 3 })).type).toBe('rpc.error')
    } finally { f.close() }
  })
  it('prepares a subscription before persisting activation and returns the mapped failure without touching any live session', async () => {
    const f = await fixture()
    const dispose = vi.fn()
    try {
      const provider = await f.store.adoptSubscription({ name: 'Subscription', modelMapping: { opus: 'gpt-test', sonnet: 'gpt-test', haiku: 'gpt-test' }, subscription: { provider: 'codex', credentialRef: { name: 'fixture.json' }, account: { status: 'ready' }, modelMetadata: [{ id: 'gpt-test', contextWindow: 200000 }] } })
      f.sessions.set({ sessionId: 'live', profile: 'claude-code', pid: 12345, dispose } as never)
      const result = await f.rpc('provider-config.activate', { cli: 'claude-code', providerId: provider.id })
      expect(result).toMatchObject({ type: 'rpc.error', code: 'INTERNAL_ERROR', retryable: true, providerFailure: { status: 'router-start-failed', recovery: 'retry' } })
      expect(JSON.stringify(result)).not.toContain('private-value')
      expect((await f.store.snapshot()).activeProviderIds['claude-code']).toBe('anthropic-official')
      expect(f.sessions.get('live')?.pid).toBe(12345)
      expect(dispose).not.toHaveBeenCalled()
      expect(f.accounts.prepareLaunch).toHaveBeenCalledWith(provider.id, 'subscription-activation-preparation')
    } finally { f.sessions.delete('live'); f.close() }
  })
  it('pushes the provider change notification through the current runtime connection and stops after disconnection', async () => {
    const f = await fixture()
    try {
      await f.rpc('provider-config.snapshot')
      f.changed()
      expect(f.port.sent.at(-1)).toEqual({ type: 'provider-config.changed', protocolVersion: PROTOCOL_VERSION })
      f.port.close()
      const count = f.port.sent.length
      f.changed()
      expect(f.port.sent).toHaveLength(count)
      expect(f.unsubscribe).toHaveBeenCalledOnce()
    } finally { f.close() }
  })
  it('retains the read-only storage gate for every new side-effecting account operation', async () => {
    const f = await fixture(true)
    try {
      for (const method of ['login-start', 'login-reopen', 'login-cancel', 'models-read', 'settings-read', 'risk-confirm']) expect((await f.rpc(`provider-config.subscription.${method}` as RpcMethod)).type).toBe('rpc.error')
      expect(f.connection.start).not.toHaveBeenCalled()
      expect(f.connection.settings).not.toHaveBeenCalled()
      expect((await f.rpc('provider-config.snapshot')).type).toBe('rpc.response')
    } finally { f.close() }
  })
})
