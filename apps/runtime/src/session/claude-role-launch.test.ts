import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, expect, it, vi } from 'vitest'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { prepareClaudeRoleLaunch, claudeRoleLaunchDigest, claudeRequestRole } from './claude-role-launch'
import type { SubscriptionAccounts } from '../model-router/subscription-accounts'
vi.mock('./native-model-catalog', () => ({ nativeClaudeModels: async () => [
 { id: 'default', resolvedModel: 'claude-sonnet-fixture' }, { id: 'opus', resolvedModel: 'claude-opus-fixture' }, { id: 'sonnet', resolvedModel: 'claude-sonnet-fixture' }, { id: 'haiku', resolvedModel: 'claude-haiku-fixture' }
] }))
const roots: string[] = []
afterEach(async () => { for (const root of roots.splice(0)) await rm(root, { recursive: true, force: true }); vi.restoreAllMocks() })
async function fixture() {
 const root = await mkdtemp(join(tmpdir(), 'role-launch-')); roots.push(root)
 const store = new ProviderConfigStore(root)
 const api = await store.upsert({ cli: 'claude-code', name: 'API', endpoint: 'https://target.invalid', model: 'same', apiKey: 'old-key', modelMapping: { opus: 'same', sonnet: 'same', haiku: 'same' }, modelMetadata: [{ id: 'same', contextWindow: 200000, maxOutputTokens: 1024, supportedEfforts: ['low'] }], effortMapping: { sonnet: 'low' } })
 await store.activate('claude-code', api.id)
 const preferences = { contextWindows: { [api.id]: '512k' as const }, compaction: { mode: 'follow' as const, effort: 'follow' as const }, claudeRoleAssignments: { opus: { providerId: api.id, model: 'same' } } }
 return { root, store, api, preferences }
}
it('captures keys/models once and caps the requested window by actual target capabilities', async () => {
 const f = await fixture(), prepared = (await prepareClaudeRoleLaunch({ ...f, env: {} }))!
 await f.store.upsert({ id: f.api.id, cli: 'claude-code', name: 'API', endpoint: 'https://changed.invalid', model: 'same', apiKey: 'new-key' })
 expect(prepared.snapshot.roles.opus).toMatchObject({ baseUrl: 'https://target.invalid', headers: { 'x-api-key': 'old-key' }, model: 'same' })
 expect(prepared.contextWindow).toBe(200000)
 expect(prepared.resolveMainModel('sonnet')).toBe('matou-role/sonnet')
 expect(prepared.resolveMainModel('fable[1M]')).toBe('matou-role/opus')
 expect(prepared.resolveMainModel('literal-custom')).toBe('literal-custom')
 expect(prepared.resolveMainModel('vendor/claude-sonnet-custom')).toBe('vendor/claude-sonnet-custom')
 expect(prepared.resolveMainModel('claude-sonnet-private')).toBe('claude-sonnet-private')
 expect(prepared.resolveMainModel('claude-opus-fixture')).toBe('matou-role/opus')
 expect(prepared.resolveMainModel('default')).toBe('matou-role/sonnet')
 prepared.close()
})
it('keeps unknown capability explicit and uses a known minimum without inheriting the source 512K', async () => {
 const f = await fixture()
 const unknown = await f.store.upsert({ cli: 'claude-code', name: 'Unknown', endpoint: 'https://unknown.invalid', model: 'same', apiKey: 'unknown' })
 f.preferences.claudeRoleAssignments = { opus: { providerId: unknown.id, model: 'same' } }
 const prepared = (await prepareClaudeRoleLaunch({ ...f, env: {} }))!
 expect(prepared.unknownContextRoles).toEqual(['opus'])
 expect(prepared.contextWindow).toBe(200000)
 prepared.close()
})
it('releases acquired targets if another role preparation fails, and after close during an await', async () => {
 const f = await fixture()
 const sub = await f.store.adoptSubscription({ name: 'Fixture', modelMapping: { opus: 'gpt', sonnet: 'gpt', haiku: 'gpt' }, subscription: { provider: 'codex', credentialRef: { name: 'test.json' }, account: { status: 'ready' }, modelMetadata: [{ id: 'gpt' }, { id: 'other' }] } })
 const released: string[] = []
 const accounts = { prepareModelTarget: async (_id: string, model: string) => { if (model === 'other') throw new Error('fixture unavailable'); return { accountIdentity: 'fixture-account', assertActive: () => {}, lease: { release: () => { released.push(model) } }, baseUrl: 'http://127.0.0.1:1234', headers: { authorization: 'Bearer fixture' }, model: `matou-chatgpt/${model}`, metadata: { id: model } } } } as unknown as SubscriptionAccounts
 const preferences = { ...f.preferences, claudeRoleAssignments: { opus: { providerId: sub.id, model: 'gpt' }, sonnet: { providerId: sub.id, model: 'other' } } }
 await expect(prepareClaudeRoleLaunch({ ...f, preferences, env: {}, accounts })).rejects.toThrow(/sonnet.*Fixture/i)
 expect(released).toEqual(['gpt'])
 let closed = false
 const late = { prepareModelTarget: async () => { closed = true; return { accountIdentity: 'fixture-account', assertActive: () => {}, lease: { release: () => { released.push('late') } }, baseUrl: 'http://127.0.0.1:1234', headers: {}, model: 'matou-chatgpt/gpt', metadata: { id: 'gpt' } } } } as unknown as SubscriptionAccounts
 await expect(prepareClaudeRoleLaunch({ ...f, preferences: { ...preferences, claudeRoleAssignments: { opus: preferences.claudeRoleAssignments.opus } }, env: {}, accounts: late, isClosed: () => closed })).rejects.toThrow(/closed/i)
 expect(released).toContain('late')
})
it('rejects a too-small actual target with its role and supplier, without silently widening it', async () => {
 const f = await fixture()
 await f.store.upsert({ id: f.api.id, cli: 'claude-code', name: 'Tiny API', endpoint: 'https://target.invalid', model: 'same', modelMetadata: [{ id: 'same', contextWindow: 32000 }] })
 await expect(prepareClaudeRoleLaunch({ ...f, env: {} })).rejects.toThrow(/opus.*Tiny API.*100K/i)
})

it('uses the same launch digest for preparation and provider activation, ignoring labels and account quota', async () => {
 const f = await fixture(), prepared = (await prepareClaudeRoleLaunch({ ...f, env: {} }))!
 const captured = await f.store.captureClaudeRouting()
 expect(claudeRoleLaunchDigest(f.preferences, captured)).toBe(prepared.digest)
 captured.providers.providers['claude-code'].find(p=>p.id===f.api.id)!.name = 'Renamed'
 expect(claudeRoleLaunchDigest(f.preferences, captured)).toBe(prepared.digest)
 captured.apiConnections[f.api.id]!.headers['x-api-key'] = 'rotated'
 expect(claudeRoleLaunchDigest(f.preferences, captured)).not.toBe(prepared.digest)
 prepared.close()
})
it('retains the subscription default Opus identity while targeting another provider for Opus', async () => {
 const f = await fixture()
 const sub = await f.store.adoptSubscription({ name: 'Default subscription', modelMapping: { opus: 'gpt', sonnet: 'gpt', haiku: 'gpt' }, subscription: { provider: 'codex', credentialRef: { name: 'test.json' }, account: { status: 'ready' }, modelMetadata: [{ id: 'gpt', contextWindow: 200000 }] } })
 await f.store.activate('claude-code', sub.id)
 const controller = new AbortController(), assertActive = vi.fn()
 let leases = 0
 const accounts = { prepareModelTarget: async (_id: string, model: string) => { leases++; return { targetSignal: controller.signal, assertActive, accountIdentity: 'fixture-account', lease: { release: () => { leases-- } }, baseUrl: 'http://127.0.0.1:1234', headers: { authorization: 'Bearer fixture' }, model: `matou-chatgpt/${model}`, metadata: { id: model, contextWindow: 200000 } } } } as unknown as SubscriptionAccounts
 const prepared = (await prepareClaudeRoleLaunch({ ...f, env: {}, accounts }))!
 expect(prepared.resolveMainModel(prepared.sourceModel)).toBe('matou-role/opus')
 expect(prepared.snapshot.roles.opus.providerId).toBe(f.api.id)
 expect(prepared.sourceConnection?.kind).toBe('subscription')
 if (prepared.sourceConnection?.kind === 'subscription') { expect(prepared.sourceConnection.targetSignal).toBe(controller.signal); expect(prepared.sourceConnection.assertActive).toBe(assertActive) }
 expect(prepared.snapshot.roles.sonnet.accountIdentity).toBe('fixture-account')
 expect(leases).toBe(1)
 prepared.close(); prepared.close(); expect(leases).toBe(0)
})
it('keeps unmapped API tiers native, preserves partial mappings and leaves the custom startup model explicit', async () => {
 const f=await fixture()
 const source=await f.store.upsert({cli:'claude-code',name:'Custom API',endpoint:'https://custom.invalid',apiKey:'fixture',model:'custom'})
 await f.store.activate('claude-code',source.id)
 const first=(await prepareClaudeRoleLaunch({...f,env:{}}))!
 expect(first.snapshot.roles.haiku).toMatchObject({providerId:source.id,model:'claude-haiku-fixture'})
 expect(first.snapshot.roles.sonnet).toMatchObject({providerId:source.id,model:'claude-sonnet-fixture'})
 expect(first.snapshot.roles.haiku.metadata).toBeUndefined()
 expect(first.resolveMainModel(first.sourceModel)).toBe('custom')
 expect(first.resolveMainModel('fable')).toBe('matou-role/opus')
 first.close()
 await f.store.upsert({id:source.id,cli:'claude-code',name:'Custom API',endpoint:'https://custom.invalid',model:'custom',modelMapping:{haiku:'fast-custom'}})
 const partial=(await prepareClaudeRoleLaunch({...f,env:{}}))!
 expect(partial.snapshot.roles.haiku.model).toBe('fast-custom');expect(partial.snapshot.roles.sonnet.model).toBe('claude-sonnet-fixture');partial.close()
 const explicit=(await prepareClaudeRoleLaunch({...f,env:{},preferences:{...f.preferences,claudeRoleAssignments:{opus:{providerId:source.id,model:'explicit-opus'},sonnet:{providerId:source.id,model:'explicit-sonnet'},haiku:{providerId:source.id,model:'explicit-haiku'}}}}))!
 expect(Object.values(explicit.snapshot.roles).map(t=>t.model)).toEqual(['explicit-opus','explicit-sonnet','explicit-haiku']);explicit.close()
})
it('leaves a wholly unknown window native even when the source preference requests 512K', async () => {
 const f=await fixture(), unknown=await f.store.upsert({cli:'claude-code',name:'Unknown all',endpoint:'https://unknown.invalid',apiKey:'fixture',model:'custom'})
 await f.store.activate('claude-code',unknown.id)
 const target={providerId:unknown.id,model:'custom'}
 const prepared=(await prepareClaudeRoleLaunch({...f,env:{},preferences:{...f.preferences,contextWindows:{[unknown.id]:'512k'},claudeRoleAssignments:{opus:target,sonnet:target,haiku:target}}}))!
 expect(prepared.contextWindow).toBeNull();expect(prepared.unknownContextRoles).toEqual(['opus','sonnet','haiku']);prepared.close()
})

it.each(['official', 'missing-api', 'expired-subscription'])('Task3 does not require unused source %s for three assigned roles', async kind => {
 const f = await fixture()
 let sourceId = (await f.store.snapshot()).providers['claude-code'].find(p=>p.builtIn)!.id
 if(kind==='missing-api') sourceId=(await f.store.upsert({cli:'claude-code',name:'No key',endpoint:'https://empty.invalid',model:'default'})).id
 if(kind==='expired-subscription') sourceId=(await f.store.adoptSubscription({name:'Expired unused',modelMapping:{opus:'gpt',sonnet:'gpt',haiku:'gpt'},subscription:{provider:'codex',credentialRef:{name:'expired.json'},account:{status:'needs-sign-in'},modelMetadata:[{id:'gpt'}]}})).id
 await f.store.activate('claude-code',sourceId)
 const target={providerId:f.api.id,model:'same'}
 const prepared=(await prepareClaudeRoleLaunch({...f,env:{CLAUDE_CONFIG_DIR:join(f.root,'no-account')},preferences:{...f.preferences,compaction:{mode:'custom',providerId:f.api.id,model:'same',effort:'follow'},claudeRoleAssignments:{opus:target,sonnet:target,haiku:target}}}))!
 expect(prepared.resolveMainModel('default')).toBe('matou-role/sonnet')
 expect(prepared.sourceConnection).toBeUndefined()
 expect(()=>prepared.resolveMainModel('custom-source')).toThrow(/source|supplier/i)
 expect(Object.values(prepared.snapshot.roles).every(t=>t.providerId===f.api.id)).toBe(true)
 prepared.close()
})

it('review caps role budget by custom compaction and marks unknown without guessing',async()=>{
 const f=await fixture()
 await f.store.upsert({id:f.api.id,cli:'claude-code',name:'Large',endpoint:'https://target.invalid',model:'same',modelMetadata:[{id:'same',contextWindow:1000000}]})
 const compact=await f.store.upsert({cli:'claude-code',name:'Compact',endpoint:'https://compact.invalid',model:'summary',apiKey:'fixture',modelMetadata:[{id:'summary',contextWindow:128000,maxContextWindow:256000}]})
 const preferences={...f.preferences,contextWindows:{},compaction:{mode:'custom' as const,providerId:compact.id,model:'summary',effort:'follow' as const}}
 const prepare=(prefs=preferences)=>prepareClaudeRoleLaunch({...f,preferences:prefs,env:{}})
 const first=(await prepare())!;expect(first.contextWindow).toBe(128000);first.close()
 const max=(await prepare({...preferences,contextWindows:{[compact.id]:'maximum' as const}}))!;expect(max.contextWindow).toBe(256000);max.close()
 const follow=(await prepareClaudeRoleLaunch({...f,env:{},preferences:{...preferences,compaction:{mode:'follow',effort:'follow'}}}))!;expect(follow.contextWindow).toBe(1000000);follow.close()
 await f.store.upsert({id:compact.id,cli:'claude-code',name:'Compact',endpoint:'https://compact.invalid',model:'summary',modelMetadata:[{id:'summary'}]})
 const unknown=(await prepare())!;expect(unknown.contextWindow).toBe(1000000);expect(unknown.unknownCompactionContext).toBe(true);unknown.close()
 await f.store.upsert({id:compact.id,cli:'claude-code',name:'Compact',endpoint:'https://compact.invalid',model:'summary',modelMetadata:[{id:'summary',contextWindow:64000}]})
 await expect(prepare()).rejects.toThrow(/compaction.*Compact.*100K/)
})

it('recognizes only exact defined subscription aliases and captured native model IDs', () => {
 for (const id of ['matou-antigravity/claude-sonnet-5-5[1M]', 'matou-antigravity/claude-sonnet-5', 'matou-antigravity/matou-sonnet', 'matou-chatgpt/matou-sonnet', 'matou-api/sonnet']) expect(claudeRequestRole(id)).toBe('sonnet')
 expect(claudeRequestRole('claude-sonnet-fixture', { sonnet: 'claude-sonnet-fixture' })).toBe('sonnet')
 for (const id of ['claude-sonnet-private', 'vendor/claude-sonnet-custom', 'matou-antigravity/claude-sonnet-private', 'other/matou-sonnet', 'claude-sonnet-fixture']) expect(claudeRequestRole(id)).toBeUndefined()
})
