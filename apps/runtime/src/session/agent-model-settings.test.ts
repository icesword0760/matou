import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { AgentModelSettings } from './agent-model-settings'
vi.mock('./native-model-catalog', () => ({
  nativeClaudeModels: async () => [{ id: 'default', contextWindow: 1_000_000, maxContextWindow: 1_000_000 }, { id: 'claude-test', contextWindow: 200000, maxContextWindow: 1_000_000 }, ...['opus','sonnet','haiku'].map(role=>({id:role,resolvedModel:`claude-${role}-native`,contextWindow:200000}))],
  nativeCodexModels: async () => [{ id: 'gpt-test', contextWindow: 272000, maxContextWindow: 872000, supportedEfforts: ['low', 'high'] }],
  nativeCodexDefaultModel: async () => 'gpt-test'
}))
let root: string, db: RuntimeDatabase, store: ProviderConfigStore, settings: AgentModelSettings
beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-model-settings-')); db = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(db, FOUNDATION_MIGRATIONS).migrate(); store = new ProviderConfigStore(root); settings = new AgentModelSettings(db, store)
})
afterEach(async () => { db.close(); await rm(root, { recursive: true, force: true }) })
it('persists the independent policy without activating a different main provider', async () => {
  const before = await store.snapshot(), target = before.providers['claude-code'][0]!
  const preferences = { contextWindows: { [target.id]: 'maximum' }, compaction: { mode: 'custom', providerId: target.id, model: 'claude-test', effort: 'high' } } as const
  await settings.set(preferences)
  expect(new AgentModelSettings(db, store).preferences()).toEqual(preferences)
  expect((await store.snapshot()).activeProviderIds).toEqual(before.activeProviderIds)
  await expect(settings.set({ ...preferences, compaction: { ...preferences.compaction, providerId: 'deleted' } })).rejects.toThrow('unavailable')
  expect(settings.preferences()).toEqual(preferences)
})
it('shows native default, exact fixed and extended windows and resolves the chosen Codex default', async () => {
  const snapshot = await settings.snapshot()
  expect(snapshot.preferences.contextWindows).toMatchObject({ 'openai-official': '512k', 'anthropic-official': 'default' })
  expect(snapshot.providers.find(p => p.cli === 'codex')!.models[0]).toMatchObject({ defaultTokens: 272000, fixedTokens: 512000, maximumTokens: 872000 })
  const provider = (await store.snapshot()).providers.codex[0]!
  expect(await settings.launchWindow(provider)).toBe(512000)
  await settings.set({ contextWindows: { 'openai-official': 'default' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(await settings.launchWindow(provider)).toBeUndefined()
  await settings.set({ contextWindows: { 'openai-official': 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(await settings.launchWindow(provider)).toBe(872000)
})
it('applies 512K to all models, preserves unknown limits, and uses the safe common window for role switching', async () => {
  const unknown = await store.upsert({ cli: 'claude-code', name: 'API', endpoint: 'https://api.example', model: 'private-model', apiKey: 'test' })
  await settings.set({ contextWindows: { [unknown.id]: '512k' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(await settings.launchWindow(unknown)).toBe(512000)
  expect((await settings.snapshot()).providers.find(p => p.id === unknown.id)!.models[0]).toEqual({ id: 'private-model', name: 'private-model', fixedTokens: 512000 })
  await settings.set({ contextWindows: { 'openai-official': 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(await settings.launchWindow(unknown)).toBeUndefined()
  const mapped = await store.upsert({ cli: 'claude-code', name: 'API', endpoint: 'https://api.example', model: 'claude-test', apiKey: 'test', modelMapping: { opus: 'claude-test', sonnet: 'unknown' } })
  expect(await settings.launchWindow(mapped)).toBeUndefined()
})
it('rejects malformed input before changing the saved policy', async () => {
  await expect(settings.set({ contextWindows: { test: '512k' }, compaction: { mode: 'custom', model: 'bad\nmodel', effort: 'high' } })).rejects.toThrow()
  await expect(settings.set({ contextWindows: { test: 'guess' }, compaction: { mode: 'follow', effort: 'high' } })).rejects.toThrow()
  expect(settings.preferences().contextWindows).toEqual({})
})

it('keeps provider window choices independent across saves and launches', async () => {
  const snapshot = await settings.snapshot()
  const providers = (await store.snapshot()).providers
  await settings.set({ ...snapshot.preferences, contextWindows: { ...snapshot.preferences.contextWindows, 'anthropic-official': 'maximum', 'openai-official': '512k' } })
  expect(await settings.launchWindow(providers['claude-code'][0])).toBe(1000000)
  expect(await settings.launchWindow(providers.codex[0])).toBe(512000)
  await settings.set({ ...settings.preferences(), contextWindows: { ...settings.preferences().contextWindows, 'openai-official': 'default' } })
  expect(await settings.launchWindow(providers['claude-code'][0])).toBe(1000000)
  expect(await settings.launchWindow(providers.codex[0])).toBeUndefined()
})

it('shows saved API model names and uses the smallest known window across the default and mapped Claude models', async () => {
  const provider = await store.upsert({
    cli: 'claude-code', name: 'API', endpoint: 'https://api.example', model: 'main', apiKey: 'test',
    modelMapping: { opus: 'large', sonnet: '', haiku: 'small' },
    modelMetadata: [
      { id: 'main', displayName: 'Main Model', contextWindow: 90_000, maxContextWindow: 120_000, supportedEfforts: ['low'] },
      { id: 'large', contextWindow: 1_048_576 },
      { id: 'small', contextWindow: 200_000 },
      { id: 'output-only', maxOutputTokens: 8192 }
    ]
  })
  expect(await settings.launchWindow(provider)).toBeUndefined()
  const snapshot = await settings.set({ contextWindows: { [provider.id]: 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(snapshot.providers.find(p => p.id === provider.id)?.models).toContainEqual({
    id: 'main', name: 'Main Model', defaultTokens: 90_000, maximumTokens: 120_000, fixedTokens: 512_000, supportedEfforts: ['low']
  })
  expect(await settings.launchWindow(provider)).toBe(120_000)
  expect(await settings.launchWindow(provider, 'matou-api/opus')).toBe(200_000)
  expect(await settings.launchWindow(provider, 'output-only')).toBeUndefined()
  const updated = await store.upsert({ ...provider, kind: 'api-key', modelMapping: { opus: 'large', haiku: 'unlisted' } })
  expect((await settings.snapshot()).providers.find(p => p.id === provider.id)?.models).toContainEqual({
    id: 'unlisted', name: 'unlisted', fixedTokens: 512_000
  })
  expect(await settings.launchWindow(updated)).toBeUndefined()
})

it('uses the custom Codex API catalog when its model ID matches a native model', async () => {
  const provider = await store.upsert({
    cli: 'codex', name: 'API', endpoint: 'https://api.example', model: 'gpt-test', apiKey: 'test',
    modelMetadata: [{ id: 'gpt-test', displayName: 'Gateway Model', contextWindow: 1_048_576 }]
  })
  const snapshot = await settings.set({ contextWindows: { [provider.id]: 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(snapshot.providers.find(p => p.id === provider.id)?.models).toEqual([
    { id: 'gpt-test', name: 'Gateway Model', defaultTokens: 1_048_576, fixedTokens: 512_000 }
  ])
  expect(await settings.launchWindow(provider)).toBe(1_048_576)
  expect(await settings.launchWindow(provider, 'unknown')).toBeUndefined()
})

it('keeps native Codex metadata out of a built-in configuration edited to use a gateway', async () => {
  const provider = await store.upsert({
    id: 'openai-official', cli: 'codex', name: 'Gateway', endpoint: 'https://api.example', model: 'gpt-test'
  })
  const snapshot = await settings.set({ contextWindows: { [provider.id]: 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } })
  expect(snapshot.providers.find(p => p.id === provider.id)?.models).toEqual([{ id: 'gpt-test', name: 'gpt-test', fixedTokens: 512_000 }])
  expect(await settings.launchWindow(provider)).toBeUndefined()
})

it('patches only role assignments and lets an old settings writer preserve them', async () => {
  const a = await store.upsert({ cli: 'claude-code', name: 'Target', endpoint: 'https://target.invalid', model: 'shared', apiKey: 'private-fixture' })
  const old = settings.preferences()
  await settings.patchAssignments({ assignments: { opus: { providerId: a.id, model: 'shared' }, sonnet: { providerId: a.id, model: 'custom' } } })
  await settings.set({ ...old, contextWindows: { [a.id]: 'maximum' }, claudeRoleAssignments: {} })
  expect(settings.preferences()).toMatchObject({ contextWindows: { [a.id]: 'maximum' }, claudeRoleAssignments: { opus: { providerId: a.id, model: 'shared' }, sonnet: { providerId: a.id, model: 'custom' } } })
  expect(JSON.stringify(await settings.snapshot())).not.toContain('private-fixture')
  await settings.patchAssignments({ assignments: {} })
  expect(settings.preferences()).toMatchObject({ contextWindows: { [a.id]: 'maximum' }, claudeRoleAssignments: {} })
})

it('merges a delayed assignment patch into the latest context and compaction settings', async () => {
  const a = await store.upsert({ cli: 'claude-code', name: 'Target', endpoint: 'https://target.invalid', model: 'shared', apiKey: 'fixture' })
  const original = store.snapshot.bind(store)
  let entered!: () => void, release!: () => void
  const ready = new Promise<void>(r => { entered = r }), gate = new Promise<void>(r => { release = r })
  vi.spyOn(store, 'snapshot').mockImplementationOnce(async () => { entered(); await gate; return original() })
  const patch = settings.patchAssignments({ assignments: { haiku: { providerId: a.id, model: 'shared' } } })
  await ready
  await settings.set({ contextWindows: { [a.id]: '512k' }, compaction: { mode: 'custom', effort: 'high', providerId: a.id, model: 'custom-summary' } })
  release(); await patch
  expect(settings.preferences()).toMatchObject({ contextWindows: { [a.id]: '512k' }, compaction: { model: 'custom-summary' }, claudeRoleAssignments: { haiku: { providerId: a.id, model: 'shared' } } })
})

it('preserves a new assignment while an old set awaits compaction validation', async () => {
  const a = await store.upsert({ cli: 'claude-code', name: 'Target', endpoint: 'https://target.invalid', model: 'shared', apiKey: 'fixture' })
  const original = store.snapshot.bind(store)
  let entered!: () => void, release!: () => void
  const ready = new Promise<void>(r => { entered = r }), gate = new Promise<void>(r => { release = r })
  vi.spyOn(store, 'snapshot').mockImplementationOnce(async () => { entered(); await gate; return original() })
  const save = settings.set({ contextWindows: {}, compaction: { mode: 'custom', effort: 'follow', providerId: a.id, model: 'summary' } })
  await ready
  await settings.patchAssignments({ assignments: { opus: { providerId: a.id, model: 'shared' } } })
  release(); await save
  expect(settings.preferences()).toMatchObject({ compaction: { model: 'summary' }, claudeRoleAssignments: { opus: { providerId: a.id, model: 'shared' } } })
})

it.each([null, [], {}, { assignments: [] }, { assignments: { fable: { providerId: 'p', model: 'm' } } },
  { assignments: { opus: { providerId: '', model: 'm' } } }, { assignments: { opus: { providerId: 'p', model: '\n' } } },
  { assignments: { opus: { providerId: 'p', model: 'x'.repeat(241) } } },
  { assignments: { opus: { providerId: 'p'.repeat(257), model: 'm' } } },
  { assignments: { opus: { providerId: 'p\u007f', model: 'm' } } }
])('rejects malformed role patches without changing preferences: %j', async input => {
  const before = settings.preferences()
  await expect(settings.patchAssignments(input as never)).rejects.toThrow()
  expect(settings.preferences()).toEqual(before)
})

it('rejects missing providers and explicit missing API catalog models, retaining an old selection on read', async () => {
  const a = await store.upsert({ cli: 'claude-code', name: 'Catalog', endpoint: 'https://target.invalid', model: 'stale', apiKey: 'fixture', modelMetadata: [{ id: 'valid' }] })
  await expect(settings.patchAssignments({ assignments: { opus: { providerId: a.id, model: 'stale' } } })).rejects.toThrow(/model.*unavailable/i)
  await settings.patchAssignments({ assignments: { opus: { providerId: a.id, model: 'valid' } } })
  await store.delete('claude-code', a.id)
  expect((await settings.snapshot()).preferences.claudeRoleAssignments?.opus).toEqual({ providerId: a.id, model: 'valid' })
  await expect(settings.patchAssignments({ assignments: { opus: { providerId: a.id, model: 'valid' } } })).rejects.toThrow(/provider.*unavailable/i)
})

it('rejects unavailable subscription accounts and does not use role mappings as a model catalog', async () => {
  const a = await store.adoptSubscription({ name: 'Subscription', modelMapping: { opus: 'm', sonnet: 'm', haiku: 'm' }, subscription: { provider: 'codex', credentialRef: { name: 'fixture.json' }, account: { status: 'ready' }, modelMetadata: [{ id: 'm' }] } })
  await expect(settings.patchAssignments({ assignments: { opus: { providerId: a.id, model: 'other' } } })).rejects.toThrow(/model.*unavailable/i)
  const snapshot = await store.snapshot()
  snapshot.providers['claude-code'].find(p => p.id === a.id)!.subscription!.account.status = 'needs-sign-in'
  vi.spyOn(store, 'snapshot').mockResolvedValue(snapshot)
  await expect(settings.patchAssignments({ assignments: { opus: { providerId: a.id, model: 'm' } } })).rejects.toThrow(/account.*unavailable/i)
})

it('exposes assignment patch through RPC without activating a provider and rejects read-only writes', async () => {
  const { RuntimeRpcRouter } = await import('../rpc/runtime-rpc-router')
  const { RuntimeAccessPolicy } = await import('../storage/runtime-access-policy')
  const { RPC_METHODS, parseRendererMessage, PROTOCOL_VERSION } = await import('@matou/contracts')
  expect(RPC_METHODS).toContain('agent-model-settings.patch-assignments')
  expect(parseRendererMessage({ type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'assignment-patch', capability: 'renderer', deadlineAt: Date.now() + 1000, method: 'agent-model-settings.patch-assignments', payload: { assignments: {} } })).toMatchObject({ type: 'rpc.request', method: 'agent-model-settings.patch-assignments' })
  const a = await store.upsert({ cli: 'claude-code', name: 'Target', endpoint: 'https://target.invalid', model: 'm', apiKey: 'fixture' })
  const before = (await store.snapshot()).activeProviderIds
  const router = new RuntimeRpcRouter(db, undefined, { providerConfigs: store })
  await router.handle('agent-model-settings.patch-assignments', { assignments: { opus: { providerId: a.id, model: 'm' } } })
  expect(settings.preferences().claudeRoleAssignments?.opus).toEqual({ providerId: a.id, model: 'm' })
  expect((await store.snapshot()).activeProviderIds).toEqual(before)
  const readOnly = new RuntimeRpcRouter(db, undefined, { providerConfigs: store, accessPolicy: new RuntimeAccessPolicy('read-only') })
  await expect(readOnly.handle('agent-model-settings.patch-assignments', { assignments: {} })).rejects.toMatchObject({ code: 'STORAGE_READ_ONLY' })
})

it('keeps a captured runtime route when the real provider store changes endpoint, key and model', async () => {
  const { resolveClaudeRoleSnapshot } = await import('./claude-role-assignments')
  const a = await store.upsert({ cli: 'claude-code', name: 'Target', endpoint: 'https://before.invalid', model: 'same', apiKey: 'before-key' })
  await store.activate('claude-code', a.id)
  await settings.patchAssignments({ assignments: { opus: { providerId: a.id, model: 'same' }, sonnet: { providerId: a.id, model: 'same' }, haiku: { providerId: a.id, model: 'same' } } })
  const selection = await store.launchSelection('claude-code', a.id)
  const captured = resolveClaudeRoleSnapshot({ preferences: settings.preferences(), providers: await store.snapshot(), connections: { [a.id]: { kind: 'api', baseUrl: selection.env.ANTHROPIC_BASE_URL!, headers: { 'x-api-key': selection.env.ANTHROPIC_API_KEY! } } } })!
  await store.upsert({ id: a.id, cli: 'claude-code', name: 'Target', endpoint: 'https://after.invalid', model: 'changed', apiKey: 'after-key' })
  await settings.patchAssignments({ assignments: {} })
  expect(captured.roles.opus).toMatchObject({ baseUrl: 'https://before.invalid', model: 'same', headers: { 'x-api-key': 'before-key' } })
  expect(captured.preferences.claudeRoleAssignments?.sonnet).toEqual({ providerId: a.id, model: 'same' })
})

it('round2 rejects invalid effective default tiers when saving mixed assignments, but permits clearing them', async () => {
 const a=await store.upsert({cli:'claude-code',name:'Default API',endpoint:'https://default.invalid',model:'unrelated',apiKey:'fixture',modelMetadata:[{id:'a'}]})
 await store.activate('claude-code',a.id)
 await expect(settings.patchAssignments({assignments:{opus:{providerId:a.id,model:'a'}}})).rejects.toThrow(/sonnet/i)
 await expect(settings.patchAssignments({assignments:{}})).resolves.toBeDefined()
})
it('round2 validates implicit subscription account status but never requires an unused official account', async () => {
 const a=await store.upsert({cli:'claude-code',name:'Target',endpoint:'https://target.invalid',model:'a',apiKey:'fixture'})
 const sub=await store.adoptSubscription({name:'Expired default',modelMapping:{opus:'m',sonnet:'m',haiku:'m'},subscription:{provider:'codex',credentialRef:{name:'fixture.json'},account:{status:'needs-sign-in'},modelMetadata:[{id:'m'}]}})
 await store.activate('claude-code',sub.id)
 await expect(settings.patchAssignments({assignments:{opus:{providerId:a.id,model:'a'}}})).rejects.toThrow(/account/i)
 await store.activate('claude-code','anthropic-official')
 const target={providerId:a.id,model:'a'}
 await expect(settings.patchAssignments({assignments:{opus:target,sonnet:target,haiku:target}})).resolves.toBeDefined()
})

it('round2 projects exact native identity for API default validation and leaves unknown API catalogs permitted', async () => {
 expect((await settings.snapshot()).providers.find(p=>p.id==='anthropic-official')!.models.find(m=>m.id==='sonnet')?.resolvedModel).toBe('claude-sonnet-native')
 const a=await store.upsert({cli:'claude-code',name:'Unknown catalog',endpoint:'https://unknown.invalid',model:'unrelated',apiKey:'fixture'})
 await store.activate('claude-code',a.id)
 await expect(settings.patchAssignments({assignments:{opus:{providerId:a.id,model:'arbitrary-custom'}}})).resolves.toBeDefined()
})
