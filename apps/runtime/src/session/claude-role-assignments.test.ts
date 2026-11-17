import { expect, it } from 'vitest'
import type { AgentModelPreferences, ProviderConfigSnapshot } from '@matou/contracts'
import * as routing from './claude-role-assignments'

function fixture() {
  const preferences: AgentModelPreferences = { contextWindows: {}, compaction: { mode: 'follow', effort: 'follow' }, claudeRoleAssignments: { opus: { providerId: 'a', model: 'same' }, sonnet: { providerId: 'b', model: 'same' } } }
  const providers: ProviderConfigSnapshot = { revision: 1, activeProviderIds: { 'claude-code': 'default', codex: '' }, providers: { codex: [], 'claude-code': [
    { id: 'default', cli: 'claude-code', name: 'Default', endpoint: 'https://default.invalid', hasApiKey: true, model: 'main', modelMapping: { haiku: 'fast' } },
    { id: 'a', cli: 'claude-code', name: 'A', endpoint: 'https://a.invalid', hasApiKey: true, model: 'old', modelMetadata: [{ id: 'same', contextWindow: 200000, maxOutputTokens: 4096, supportedEfforts: ['low'] }] },
    { id: 'b', cli: 'claude-code', name: 'B', endpoint: 'https://b.invalid', hasApiKey: true, model: 'other', effortMapping: { sonnet: 'high' } }
  ] } }
  const connections = {
    default: { kind: 'api' as const, baseUrl: 'https://default.invalid', headers: { 'x-api-key': 'default-key' } },
    a: { kind: 'api' as const, baseUrl: 'https://a.invalid', headers: { 'x-api-key': 'a-key' } },
    b: { kind: 'api' as const, baseUrl: 'https://b.invalid', headers: { 'x-api-key': 'b-key' } }
  }
  return { preferences, providers, connections }
}
it('captures different providers for identical model IDs and resolves the default Haiku independently', () => {
  const snapshot = routing.resolveClaudeRoleSnapshot(fixture())!
  expect(snapshot.roles.opus).toMatchObject({ providerId: 'a', model: 'same', baseUrl: 'https://a.invalid', headers: { 'x-api-key': 'a-key' }, metadata: { contextWindow: 200000 } })
  expect(snapshot.roles.sonnet).toMatchObject({ providerId: 'b', model: 'same', headers: { 'x-api-key': 'b-key' }, effort: 'high' })
  expect(snapshot.roles.haiku).toMatchObject({ providerId: 'default', model: 'fast' })
  expect(snapshot.roles.sonnet.metadata).toBeUndefined()
})
it('freezes preferences, endpoint, API key and nested capabilities against subsequent store/input changes', () => {
  const input = fixture(), snapshot = routing.resolveClaudeRoleSnapshot(input)!
  input.preferences.claudeRoleAssignments!.opus!.model = 'changed'
  input.connections.a.baseUrl = 'https://changed.invalid'; input.connections.a.headers['x-api-key'] = 'changed'
  input.providers.providers['claude-code'][1]!.modelMetadata![0]!.supportedEfforts!.push('max')
  expect(snapshot.roles.opus).toMatchObject({ model: 'same', baseUrl: 'https://a.invalid', headers: { 'x-api-key': 'a-key' }, metadata: { supportedEfforts: ['low'] } })
  expect(snapshot.preferences.claudeRoleAssignments!.opus!.model).toBe('same')
  expect(Object.isFrozen(snapshot.roles.opus.metadata?.supportedEfforts)).toBe(true)
})
it.each([undefined, {}])('keeps absent or empty assignments completely on the old path', assignments => {
  const input = fixture()
  if (assignments === undefined) delete input.preferences.claudeRoleAssignments
  else input.preferences.claudeRoleAssignments = assignments
  expect(routing.resolveClaudeRoleSnapshot({ ...input, connections: {} })).toBeUndefined()
})
it('includes exact custom compaction target and every actual official family, without default fallback', () => {
  const input = fixture()
  input.preferences.compaction = { mode: 'custom', effort: 'low', providerId: 'b', model: 'summary' }
  input.connections.b.baseUrl = 'https://api.anthropic.com'
  const snapshot = routing.resolveClaudeRoleSnapshot(input)!
  expect(snapshot.compactionTarget).toMatchObject({ providerId: 'b', model: 'summary', baseUrl: 'https://api.anthropic.com' })
  expect(snapshot.officialFamilies).toEqual(['anthropic'])
})
it('rejects a deleted assigned provider and an explicit catalog model missing from the target', () => {
  const input = fixture()
  input.providers.providers['claude-code'] = input.providers.providers['claude-code'].filter(p => p.id !== 'a')
  expect(() => routing.resolveClaudeRoleSnapshot(input)).toThrow(/provider.*unavailable/i)
  const other = fixture(); other.preferences.claudeRoleAssignments!.opus!.model = 'deleted'
  expect(() => routing.resolveClaudeRoleSnapshot(other)).toThrow(/model.*unavailable/i)
})
it('captures exact subscription raw model and local connection instead of its shared role mapping', () => {
  const input = fixture()
  input.providers.providers['claude-code'][1] = { id: 'a', cli: 'claude-code', name: 'Subscription', endpoint: '', model: 'mapped', hasApiKey: false, modelMapping: { opus: 'mapped', sonnet: 'mapped', haiku: 'mapped' }, subscription: { provider: 'codex', account: { status: 'ready' }, modelMetadata: [{ id: 'same', contextWindow: 272000 }, { id: 'mapped' }] } }
  const snapshot = routing.resolveClaudeRoleSnapshot({ ...input, connections: { ...input.connections, a: { kind: 'subscription', models: { same: { accountIdentity: 'fixture-account', assertActive: () => {}, baseUrl: 'http://127.0.0.1:9999', headers: { authorization: 'Bearer fixture-router' }, model: 'matou-chatgpt/same', metadata: { id: 'same', contextWindow: 272000 } } } } } })!
  expect(snapshot.roles.opus).toMatchObject({ kind: 'subscription', model: 'matou-chatgpt/same', catalogModel: 'same', headers: { authorization: 'Bearer fixture-router' } })
  expect(snapshot.officialFamilies).toEqual(['chatgpt-subscription'])
})
it('allows token refresh only for the captured official account identity', async () => {
  const input = fixture()
  input.providers.providers['claude-code'][1]!.endpoint = 'https://api.anthropic.com'
  input.providers.providers['claude-code'][1]!.hasApiKey = false
  let identity = 'official-account-A', token = 'first'
  const snapshot = routing.resolveClaudeRoleSnapshot({ ...input, connections: { ...input.connections, a: { kind: 'oauth', baseUrl: 'https://api.anthropic.com', identity, readHeaders: async () => ({ identity, headers: { authorization: `Bearer ${token}` } }) } } })!
  token = 'refreshed'
  expect(await snapshot.roles.opus.resolveHeaders!()).toEqual({ authorization: 'Bearer refreshed' })
  identity = 'different-account'
  await expect(snapshot.roles.opus.resolveHeaders!()).rejects.toThrow(/identity/i)
})
it('uses the native role metadata for a default official selector instead of forcing every role to Opus', () => {
  const input = fixture()
  input.providers.providers['claude-code'][0] = { id: 'default', cli: 'claude-code', name: 'Official', endpoint: 'https://api.anthropic.com', hasApiKey: true, builtIn: true, model: 'CLI 默认' }
  const snapshot = routing.resolveClaudeRoleSnapshot({ ...input, nativeModels: [{ id: 'haiku', resolvedModel: 'claude-haiku-exact', contextWindow: 200000 }] })!
  expect(snapshot.roles.haiku).toMatchObject({ model: 'claude-haiku-exact', metadata: { contextWindow: 200000 } })
})
it('does not accept an unprepared subscription connection or unknown role as an API fallback', () => {
  const input = fixture()
  input.preferences.claudeRoleAssignments = { fable: { providerId: 'a', model: 'same' } } as never
  expect(() => routing.resolveClaudeRoleSnapshot(input)).toThrow(/Invalid Claude role/)
})

it('keeps default subscription role resolution exact and rejects an unprepared connection', () => {
  const input = fixture()
  input.preferences.claudeRoleAssignments = { opus: { providerId: 'b', model: 'same' } }
  input.providers.providers['claude-code'][0] = { id: 'default', cli: 'claude-code', name: 'Subscription', endpoint: '', model: 'mapped', hasApiKey: false, modelMapping: { opus: 'mapped', sonnet: 'mapped', haiku: 'fast' }, subscription: { provider: 'codex', account: { status: 'ready' }, modelMetadata: [{ id: 'mapped' }, { id: 'fast' }] } }
  expect(() => routing.resolveClaudeRoleSnapshot(input)).toThrow(/Subscription target connection is unavailable/)
})
it('records official-account identity and Antigravity time-zone policy without adding a monitor', () => {
  const input = fixture()
  input.providers.providers['claude-code'][1] = { id: 'a', cli: 'claude-code', name: 'Subscription', endpoint: '', model: 'same', hasApiKey: false, subscription: { provider: 'antigravity', account: { status: 'ready' }, modelMetadata: [{ id: 'same' }] } }
  const snapshot = routing.resolveClaudeRoleSnapshot({ ...input, connections: { ...input.connections, a: { kind: 'subscription', models: { same: { accountIdentity: 'fixture-account', assertActive: () => {}, baseUrl: 'http://127.0.0.1:9999', headers: { authorization: 'Bearer local' }, model: 'matou-antigravity/same', metadata: { id: 'same' } } } } } })!
  expect(snapshot.roles.opus).toMatchObject({ subscriptionProvider: 'antigravity', hideTimeZone: true })
  expect(snapshot.officialFamilies).toEqual([])
})

it('rejects null assignments and incomplete custom compaction instead of silently omitting a target', () => {
  const input = fixture()
  input.preferences.claudeRoleAssignments = null as never
  expect(() => routing.resolveClaudeRoleSnapshot(input)).toThrow(/Invalid Claude role/)
})
it('rejects an incomplete custom compaction target', () => {
  const input = fixture()
  input.preferences.compaction = { mode: 'custom', effort: 'follow' }
  expect(() => routing.resolveClaudeRoleSnapshot(input)).toThrow(/Compaction.*unavailable/i)
})
it('resolves an API CLI-default role through native identity without borrowing official capabilities', () => {
  const input = fixture()
  input.providers.providers['claude-code'][0]!.model = 'CLI 默认'
  delete input.providers.providers['claude-code'][0]!.modelMapping
  const snapshot = routing.resolveClaudeRoleSnapshot({ ...input, nativeModels: [{ id: 'haiku', resolvedModel: 'claude-haiku-exact', contextWindow: 1000000 }] })!
  expect(snapshot.roles.haiku).toMatchObject({ model: 'claude-haiku-exact' })
  expect(snapshot.roles.haiku.metadata).toBeUndefined()
})

it('preserves an API model literally named default instead of interpreting it as an official tier', () => {
  const input = fixture()
  input.providers.providers['claude-code'][0]!.model = 'default'
  input.preferences.claudeRoleAssignments!.haiku = { providerId: 'default', model: 'default' }
  delete input.providers.providers['claude-code'][0]!.modelMapping
  const snapshot = routing.resolveClaudeRoleSnapshot(input)!
  expect(snapshot.roles.haiku.model).toBe('default')
})
