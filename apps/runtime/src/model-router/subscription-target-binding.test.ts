import { EventEmitter, getEventListeners } from 'node:events'
import { createServer } from 'node:http'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, expect, it, vi } from 'vitest'
import { SubscriptionAccounts } from './subscription-accounts'
import { OAuthLoopbackFlow, type OAuthOutcome } from './oauth'
import type { ModelRouterManager } from './manager'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { resolveClaudeRoleSnapshot } from '../session/claude-role-assignments'
import { claudeModelRouting } from '../session/claude-model-routing'
import { startClaudeRequestGateway } from '../session/claude-request-gateway'
vi.mock('./codex-client', () => ({ CODEX_CLIENT_HEADERS: {}, CODEX_CLIENT_USER_AGENT: 'fixture', CODEX_CLIENT_ORIGINATOR: 'fixture', credentialCarriesCodexIdentity: async () => true }))
const cleanup: Array<() => Promise<unknown>> = []
afterEach(async () => { for (const close of cleanup.splice(0).reverse()) await close(); vi.restoreAllMocks() })
const mapping = { opus: 'model', sonnet: 'model', haiku: 'model' }
async function fixture() {
 const root = await mkdtemp(join(tmpdir(), 'matou-binding-')); cleanup.push(() => rm(root, { recursive: true, force: true }))
 const store = new ProviderConfigStore(root)
 const models = [{ id: 'model', contextWindow: 200000 }]
 const saved = await store.adoptSubscription({ name: 'Fixture', modelMapping: mapping, subscription: { provider: 'codex', credentialRef: { name: 'a.json', authIndex: 'a' }, account: { status: 'ready', email: 'a@example.invalid' }, modelMetadata: models } })
 const files = new Map(['a', 'b'].map(id => [id + '.json', { id, name: id + '.json', auth_index: id, provider: 'codex', status: 'active', email: id + '@example.invalid', disabled: id === 'b', prefix: id === 'a' ? 'matou-chatgpt' : '' }]))
 let aliases = Object.keys(mapping).map(role => ({ name: 'model', alias: 'matou-' + role, fork: true }))
 let fail = false, held = false, pendingClosed = false, outstanding = 0
 let onMutation = () => {}, mutationGate: Promise<void> | undefined
 const inference: string[] = [], mutations: string[] = []
 const server = createServer(async (req, res) => {
  const url = new URL(req.url!, 'http://fixture'), chunks: Buffer[] = []
  for await (const chunk of req) chunks.push(Buffer.from(chunk))
  const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : {}
  const reply = (value: unknown) => { res.setHeader('content-type', 'application/json'); res.end(JSON.stringify(value)) }
  if (url.pathname.startsWith('/v1/messages')) {
   const current = [...files.values()].find(file => !file.disabled && file.prefix === 'matou-chatgpt')
   inference.push(current?.id ?? 'none')
   if (held) { res.once('close', () => { pendingClosed = true }); return }
   reply({ account: current?.id, input_tokens: 3 }); return
  }
  if (req.method === 'PATCH' || req.method === 'PUT' || req.method === 'DELETE') {
   mutations.push(url.pathname); onMutation(); await mutationGate
  }
  if (url.pathname.endsWith('/credentials') && req.method === 'GET') { reply({ files: [...files.values()] }); return }
  if (url.pathname.endsWith('/credentials') && req.method === 'DELETE') { files.delete(url.searchParams.get('name')!); reply({}); return }
  if (url.pathname.endsWith('/credentials/status')) { files.get(body.name)!.disabled = body.disabled; reply({}); return }
  if (url.pathname.endsWith('/credentials/fields')) {
   if (fail) { res.writeHead(500); res.end('{}'); return }
   if (typeof body.prefix === 'string') files.get(body.name)!.prefix = body.prefix
   reply({}); return
  }
  if (url.pathname.endsWith('/credentials/models')) {
   const file = files.get(url.searchParams.get('name')!)!
   reply({ models: ['model', ...aliases.map(a => a.alias), 'matou-opus-fallback'].map(id => ({ id: file.prefix ? file.prefix + '/' + id : id })) }); return
  }
  if (url.pathname.includes('/model-alias/')) { if (req.method === 'PUT') aliases = body; reply(aliases); return }
  if (url.pathname.includes('/model-definitions/')) { reply({ models }); return }
  if (url.pathname.endsWith('/requests/api-call')) { reply({ status_code: 200, body: JSON.stringify(body.url.includes('usage') ? { plan_type: 'plus' } : { models }) }); return }
  if (url.pathname.endsWith('/oauth/auth-url')) { reply({ state: 'fixture-state', url: 'https://fixture.invalid/login' }); return }
  reply({ cancelled: true, status: 'ok', auth_id: 'b' })
 })
 await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve))
 const baseUrl = `http://127.0.0.1:${(server.address() as { port: number }).port}`
 cleanup.push(async () => { server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())) })
 const manager = { onStateChange: () => () => {}, rememberModelAliases: () => {}, releaseConnection: () => {},
  acquire: async () => { outstanding++; let released = false; return { connection: { baseUrl, clientKey: 'fixture', managementKey: 'fixture', pid: 1 }, release: () => { if (!released) { outstanding--; released = true } } } },
  waitForCredentialAdoption: async ({ authIndex }: { authIndex: string }) => ({ authIndex }) }
 const accounts = new SubscriptionAccounts({ dataRoot: root, manager: manager as unknown as ModelRouterManager, store, openAuthorizationUrl: async () => {}, platform: 'linux' })
 vi.spyOn(accounts, 'initialize').mockResolvedValue()
 cleanup.push(() => accounts.close())
 const connection = accounts.bindConnection(new EventEmitter())
 let finish!: (outcome: OAuthOutcome) => void
 vi.spyOn(OAuthLoopbackFlow, 'start').mockImplementation(async () => ({ completion: new Promise<OAuthOutcome>(resolve => { finish = resolve }), callbackAccepted: true, cancel: async () => {} }) as unknown as OAuthLoopbackFlow)
 async function loginB() {
  const attempt = await connection.start({ provider: 'codex' }); finish({ status: 'ok', authId: 'b' })
  await vi.waitFor(() => expect(connection.status(attempt.attemptId).status).toBe('ready'))
  return attempt.attemptId
 }
 const target = await accounts.prepareModelTarget(saved.id, 'model')
 cleanup.push(async () => target.lease.release())
 const preferences = { contextWindows: {}, claudeRoleAssignments: Object.fromEntries(['opus', 'sonnet', 'haiku'].map(role => [role, { providerId: saved.id, model: 'model' }])), compaction: { mode: 'custom' as const, providerId: saved.id, model: 'model', effort: 'follow' as const } }
 const snapshot = resolveClaudeRoleSnapshot({ preferences, providers: await store.snapshot(), connections: { [saved.id]: { kind: 'subscription', models: { model: target } } } })!
 expect(snapshot.roles.opus.accountIdentity).toContain('a.json')
 const routing = claudeModelRouting({ preferences, provider: saved, store, roleSnapshot: snapshot, sourceConnection: { kind: 'subscription', baseUrl, headers: target.headers, assertActive: target.assertActive, targetSignal: target.targetSignal } }, {})
 const gateway = await startClaudeRequestGateway({ upstreamUrl: baseUrl, env: {}, resolveOverride: routing.resolveOverride }); cleanup.push(gateway.close)
 const send = (model = 'matou-role/opus', count = false, compaction = false) => fetch(gateway.baseUrl + '/v1/messages' + (count ? '/count_tokens' : ''), { method: 'POST', headers: compaction ? { 'x-claude-code-compaction': 'manual' } : {}, body: JSON.stringify({ model, messages: [] }) })
 const save = (attemptId?: string) => connection.save({ id: saved.id, kind: 'subscription', cli: 'claude-code', name: 'Fixture', modelMapping: mapping, subscription: { provider: 'codex', ...(attemptId ? { attemptId } : {}) } })
 return { target, accounts, saved, store, files, sameNameIdentity: () => { files.get('a.json')!.auth_index = 'a-new'; files.get('a.json')!.email = 'a-new@example.invalid' }, connection, loginB, save, send, inference, mutations, outstanding: () => outstanding, pendingClosed: () => pendingClosed, hold: () => { held = true }, unhold: () => { held = false }, fail: () => { fail = true }, mutation: (callback: () => void, gate?: Promise<void>) => { onMutation = callback; mutationGate = gate } }
}
it.each(['role', 'count', 'compaction', 'source'])('revokes before replacement mutates the shared prefix, aborts pending %s HTTP and never sends old requests to B', async kind => {
 const f = await fixture()
 expect(await (await f.send()).json()).toMatchObject({ account: 'a' })
 // Each completed request removes its target listener, including count requests.
 for (let i = 0; i < 12; i++) await (await f.send('matou-role/opus', true)).text()
 await vi.waitFor(() => expect(getEventListeners(f.target.targetSignal, 'abort')).toHaveLength(0))
 const attempt = await f.loginB()
 f.hold(); const pending = f.send(kind === 'source' ? 'source-custom' : 'matou-role/opus', kind === 'count', kind === 'compaction'); await vi.waitFor(() => expect(f.inference).toHaveLength(14))
 let release!: () => void
 const gate = new Promise<void>(resolve => { release = resolve })
 let entered = false
 f.mutation(() => { expect(f.target.targetSignal.aborted).toBe(true); entered = true }, gate)
 const saving = f.save(attempt)
 await vi.waitFor(() => expect(entered).toBe(true))
 const stopped = await pending; expect(stopped.status).toBe(502); expect(await stopped.text()).toMatch(/账号已变更|account changed/)
 await vi.waitFor(() => expect(f.pendingClosed()).toBe(true))
 for (const [model, count, compact] of [['matou-role/opus', false, false], ['matou-role/sonnet', true, false], ['matou-role/haiku', false, true], ['source-custom', false, false]] as const) {
  const response = await f.send(model, count, compact); expect(response.status).toBe(502); expect(await response.text()).toMatch(/重新启动|Restart/)
 }
 release(); await saving
 expect((await f.send('matou-role/opus', true)).status).toBe(502)
 expect(f.inference.every(account => account === 'a')).toBe(true)
 f.mutation(() => {})
 const fresh = await f.accounts.prepareModelTarget(f.saved.id, 'model'); expect(fresh.accountIdentity).toContain('b.json'); expect(() => fresh.assertActive()).not.toThrow()
 // Direct fake router request proves the shared prefix now points at B.
 f.unhold()
 const response = await fetch(fresh.baseUrl + '/v1/messages/count_tokens', { method: 'POST', body: JSON.stringify({ model: fresh.model }) })
 expect(await response.json()).toMatchObject({ account: 'b' })
 expect(f.inference.filter(account => account === 'b')).toHaveLength(1)
 fresh.lease.release()
})
it('failed replacement restores A for new preparations without reviving old targets', async () => {
 const f = await fixture(), attempt = await f.loginB(); f.fail()
 await expect(f.save(attempt)).rejects.toThrow()
 expect(() => f.target.assertActive()).toThrow(/账号已变更|account changed/)
 expect((await f.store.subscriptions())[0]!.subscription.credentialRef.name).toBe('a.json')
 const fresh = await f.accounts.prepareModelTarget(f.saved.id, 'model'); expect(() => fresh.assertActive()).not.toThrow(); fresh.lease.release()
})
it('same-account model save keeps raw targets active; delete revokes before mutations and close revokes all remaining targets', async () => {
 const f = await fixture(); await f.save()
 expect(() => f.target.assertActive()).not.toThrow()
 expect(await (await f.send()).json()).toMatchObject({ account: 'a' })
 f.mutation(() => expect(f.target.targetSignal.aborted).toBe(true))
 await f.accounts.delete('claude-code', f.saved.id)
 expect(() => f.target.assertActive()).toThrow(/账号已变更|account changed/)
 f.target.lease.release(); f.target.lease.release(); expect(f.outstanding()).toBe(0)
 const other = await fixture(), second = await other.accounts.prepareModelTarget(other.saved.id, 'model')
 await other.accounts.close(); expect(other.outstanding()).toBe(0); expect(() => other.target.assertActive()).toThrow(); expect(() => second.assertActive()).toThrow()
 other.target.lease.release(); second.lease.release(); expect(other.outstanding()).toBe(0)
})

it('round2 preserves the same credential filename after an identity change and prepares the new identity', async () => {
 const f = await fixture(); f.sameNameIdentity(); await f.save()
 expect(f.files.has('a.json')).toBe(true); expect(f.files.get('a.json')!.disabled).toBe(false)
 expect(() => f.target.assertActive()).toThrow()
 const fresh = await f.accounts.prepareModelTarget(f.saved.id, 'model')
 expect(fresh.accountIdentity).toContain('a-new'); expect(() => fresh.assertActive()).not.toThrow(); fresh.lease.release()
})
it('round2 never prepares a same-name changed identity after a failed save against the stale stored identity', async () => {
 const f = await fixture(); f.sameNameIdentity(); f.fail(); await expect(f.save()).rejects.toThrow()
 expect(() => f.target.assertActive()).toThrow()
 expect((await f.store.subscriptions())[0]!.subscription.credentialRef.authIndex).toBe('a')
 await expect(f.accounts.prepareModelTarget(f.saved.id, 'model')).rejects.toMatchObject({ providerFailure: { status: 'needs-sign-in' } })
 expect(f.files.has('a.json')).toBe(true)
})
