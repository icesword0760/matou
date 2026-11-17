import { createServer, type Server } from 'node:http'
import { afterEach, expect, it } from 'vitest'
import { claudeModelRouting } from './claude-model-routing'
import { startClaudeRequestGateway, GatewayRoutingError } from './claude-request-gateway'
import type { ResolvedClaudeRoleSnapshot } from './claude-role-assignments'
import type { ProviderConfigStore } from '../provider-config/provider-config-store'
const disposals: Array<() => Promise<void>> = []
afterEach(async () => { for (const close of disposals.splice(0).reverse()) await close() })
async function upstream() {
 const requests: Array<{ url?: string; headers: Record<string, any>; body: any }> = []
 const server: Server = createServer(async (req, res) => { const chunks = []; for await (const c of req) chunks.push(c); requests.push({ url: req.url!, headers: req.headers, body: JSON.parse(Buffer.concat(chunks).toString()) }); res.setHeader('content-type', 'application/json'); res.end('{"ok":true,"input_tokens":11}') })
 await new Promise<void>(r => server.listen(0, '127.0.0.1', r)); const url = `http://127.0.0.1:${(server.address() as {port:number}).port}`
 disposals.push(async () => { server.closeAllConnections(); await new Promise<void>(r => server.close(() => r())) })
 return { url, requests }
}
async function fixture(custom = false) {
 const source = await upstream(), a = await upstream(), b = await upstream(), c = await upstream()
 const preferences = { contextWindows: {}, compaction: { mode: custom ? 'custom' as const : 'follow' as const, effort: 'follow' as const } }
 const target = (id: string, url: string) => ({ providerId: id, catalogModel: 'same', kind: 'api' as const, baseUrl: url, model: 'same', headers: { 'x-api-key': id }, hideTimeZone: false, metadata: { id: 'same', maxOutputTokens: 1024, supportedEfforts: ['low' as const] }, effort: 'low' as const })
 const snapshot: ResolvedClaudeRoleSnapshot = { preferences, roles: { opus: target('a', a.url), sonnet: target('b', b.url), haiku: target('source', source.url) }, ...(custom ? { compactionTarget: target('c', c.url) } : {}), officialFamilies: [] }
 const routing = claudeModelRouting({ preferences, roleSnapshot: snapshot, sourceConnection: {kind:'api',baseUrl:source.url,headers:{'x-api-key':'source'}}, store: {} as ProviderConfigStore }, {})
 const gateway = await startClaudeRequestGateway({ upstreamUrl: source.url, env: {}, resolveOverride: routing.resolveOverride }); disposals.push(gateway.close)
 const send = async (model: string, path = '/v1/messages', compaction = false) => fetch(gateway.baseUrl + path, { method: 'POST', headers: { authorization: 'Bearer source-secret', cookie: 'source-cookie', ...(compaction ? { 'x-claude-code-compaction': 'manual' } : {}) }, body: JSON.stringify({ model, max_tokens: 9999, thinking: { type: 'enabled', budget_tokens: 8000 }, output_config: { effort: 'high' }, messages: [] }) })
 return { source, a, b, c, snapshot, send, routing }
}
it('sends messages and counts with same model IDs to separate exact authenticated targets', async () => {
 const f = await fixture()
 for (const path of ['/v1/messages', '/v1/messages/count_tokens']) for (const role of ['opus','sonnet']) expect((await f.send(`matou-role/${role}`, path)).status).toBe(200)
 expect(f.source.requests).toHaveLength(0)
 for (const [target, key] of [[f.a,'a'],[f.b,'b']] as const) {
  expect(target.requests).toHaveLength(2)
  for (const r of target.requests) { expect(r.headers['x-api-key']).toBe(key); expect(r.headers.authorization).toBeUndefined(); expect(r.headers.cookie).toBeUndefined(); expect(r.body.model).toBe('same') }
  expect(target.requests[0]!.body.max_tokens).toBe(1024)
  expect(target.requests[0]!.body.output_config.effort).toBe('low')
  expect(target.requests[0]!.body.thinking).toBeUndefined()
  expect(target.requests[1]!.body.max_tokens).toBe(9999)
  expect(target.requests[1]!.body.output_config.effort).toBe('high')
 }
})
it('uses custom compaction only for message creation, and follows the actual role otherwise', async () => {
 const f = await fixture(true)
 expect((await f.send('matou-role/sonnet','/v1/messages',true)).status).toBe(200)
 expect((await f.send('matou-role/sonnet','/v1/messages/count_tokens',true)).status).toBe(200)
 expect(f.c.requests).toHaveLength(1); expect(f.b.requests).toHaveLength(1); expect(f.source.requests).toHaveLength(0)
 const follow = await fixture()
 await follow.send('matou-role/sonnet','/v1/messages',true)
 expect(follow.b.requests).toHaveLength(1)
})
it.each(['matou-role/opus-fallback','fable','claude-fable-5-5','matou-role/opus[1M]','matou-chatgpt/matou-opus','opus'])('preserves selector identity for %s', async selector => {
 const f = await fixture(); await f.send(selector); expect(f.a.requests).toHaveLength(1); expect(f.source.requests).toHaveLength(0)
})
it('keeps literal custom models on the source but refuses an invalid virtual role', async () => {
 const f = await fixture(); expect((await f.send('custom-literal')).status).toBe(200)
 expect(f.source.requests).toHaveLength(1)
 expect((await f.send('matou-role/invalid')).status).toBe(502)
 expect(f.source.requests).toHaveLength(1)
})
it('rejects unsupported target effort without calling a fallback service', async () => {
 const f = await fixture(); (f.snapshot.roles.opus as {effort:string}).effort = 'max'
 const response = await f.send('matou-role/opus')
 expect(response.status).toBe(502)
 expect(await response.text()).toMatch(/opus.*a/)
 expect(f.a.requests).toHaveLength(0); expect(f.source.requests).toHaveLength(0)
})

it('passes the actual official family to the send-time protection hook before upstream HTTP', async () => {
 const f = await fixture();(f.snapshot.roles.opus as {family:string}).family='chatgpt-subscription'
 const families: Array<string|undefined> = []
 const gateway = await startClaudeRequestGateway({upstreamUrl:f.source.url,env:{},resolveOverride:f.routing.resolveOverride,assertTargetAllowed:family=>{families.push(family);throw new GatewayRoutingError('Fixture target is protected')}});disposals.push(gateway.close)
 const response=await fetch(gateway.baseUrl+'/v1/messages/count_tokens',{method:'POST',body:JSON.stringify({model:'matou-role/opus',messages:[]})})
 expect(response.status).toBe(502);expect(families).toEqual(['chatgpt-subscription']);expect(f.a.requests).toHaveLength(0)
})

it('Task3 refuses late source selection without binding credentials or falling back', async () => {
 const f=await fixture()
 const routing=claudeModelRouting({preferences:f.snapshot.preferences as any,roleSnapshot:f.snapshot,store:{} as ProviderConfigStore},{})
 const gateway=await startClaudeRequestGateway({upstreamUrl:f.source.url,env:{},resolveOverride:routing.resolveOverride});disposals.push(gateway.close)
 const response=await fetch(gateway.baseUrl+'/v1/messages',{method:'POST',body:JSON.stringify({model:'literal-source-custom',messages:[]})})
 expect(response.status).toBe(502);expect(await response.text()).toMatch(/not bound at startup/)
 expect(f.source.requests).toHaveLength(0);expect(f.a.requests).toHaveLength(0)
})

it.each(['vendor/claude-sonnet-custom','claude-sonnet-private'])('review keeps custom name %s on source for messages and count',async model=>{
 const f=await fixture()
 for(const path of ['/v1/messages','/v1/messages/count_tokens'])expect((await f.send(model,path)).status).toBe(200)
 expect(f.source.requests.map(r=>r.body.model)).toEqual([model,model]);expect(f.b.requests).toHaveLength(0)
})
