import { createServer } from 'node:http'
import { gzipSync } from 'node:zlib'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { afterEach, expect, it, vi } from 'vitest'
import type { AgentModelPreferences } from '@matou/contracts'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { SubscriptionAccounts } from '../model-router/subscription-accounts'
import type { ModelRouterManager } from '../model-router/manager'
import { claudeModelRouting } from './claude-model-routing'
import { startClaudeRequestGateway, GatewayRoutingError } from './claude-request-gateway'
import type { ResolvedClaudeRoleSnapshot, ResolvedClaudeRoleTarget } from './claude-role-assignments'

const evidenceRoot = resolve(import.meta.dirname, '../../../../node_modules/.cache/search-auth')
const disposals: Array<()=>Promise<void>>=[]
afterEach(async()=>{for(const close of disposals.splice(0).reverse())await close()})
async function upstream(name:string) {
 const requests:Array<{path:string;headers:Record<string,unknown>;body:any}>=[]
 const reply={status:200,body:JSON.stringify({target:name,content:[{type:'web_search_tool_result',content:'local search result'}]}) as string | Buffer,headers:{} as Record<string,string>}
 const server=createServer(async(req,res)=>{const chunks:Buffer[]=[];for await(const chunk of req)chunks.push(chunk);requests.push({path:req.url!,headers:req.headers,body:JSON.parse(Buffer.concat(chunks).toString())});res.writeHead(reply.status,{'content-type':'application/json',...reply.headers});res.end(reply.body)})
 await new Promise<void>(r=>server.listen(0,'127.0.0.1',r));disposals.push(async()=>{server.closeAllConnections();await new Promise<void>(r=>server.close(()=>r()))})
 return {url:`http://127.0.0.1:${(server.address() as {port:number}).port}`,requests,reply}
}
async function fixture(search=true,kind:'api'|'subscription'|'oauth'='api',locale='zh-CN') {
 await mkdir(evidenceRoot,{recursive:true})
 const main=await upstream('haiku'),searchUpstream=await upstream('search'),compact=await upstream('compaction')
 const preferences:AgentModelPreferences={contextWindows:{},compaction:{mode:'custom',effort:'follow',providerId:'compact',model:'summary'}}
 const mainTarget:ResolvedClaudeRoleTarget={providerId:'main',providerName:'Main supplier',catalogModel:'gemini',model:'gemini',kind:'api',baseUrl:main.url,headers:{'x-api-key':'main-key'},hideTimeZone:false}
 const searchTarget:ResolvedClaudeRoleTarget={...mainTarget,providerId:'search',providerName:'Search supplier',model:'search-model',catalogModel:'search-model',kind,accountIdentity:'["fixture.json","fixture-auth"]',baseUrl:searchUpstream.url,headers:{authorization:'Bearer search-key'},metadata:{id:'search-model',contextWindow:8000,maxOutputTokens:512}}
 const snapshot:ResolvedClaudeRoleSnapshot={preferences,roles:{opus:mainTarget,sonnet:mainTarget,haiku:mainTarget},compactionTarget:{...mainTarget,providerId:'compact',baseUrl:compact.url,model:'summary'},...(search?{webSearchTarget:searchTarget}:{}),officialFamilies:[]}
 const routing=claudeModelRouting({preferences,roleSnapshot:snapshot,store:{} as ProviderConfigStore},{})
 const failures:any[]=[]
 let handler:(input:any)=>Promise<void>=async input=>{failures.push(input)}
 const gateway=await startClaudeRequestGateway({upstreamUrl:main.url,env:{MATOU_LOCALE:locale},resolveOverride:routing.resolveOverride,onAuthenticationFailure:input=>handler(input)})
 disposals.push(gateway.close)
 const send=(extra:Record<string,unknown>={},path='/v1/messages',marker?:string)=>fetch(gateway.baseUrl+path,{method:'POST',headers:{authorization:'Bearer source-secret',cookie:'private-cookie',...(marker?{'x-claude-code-compaction':marker}:{})},body:JSON.stringify({model:'matou-role/haiku',max_tokens:9999,messages:[{role:'user',content:'conversation context'}],tools:[{type:'web_search_20250305',name:'web_search'}],...extra})})
 return {main,searchUpstream,compact,send,snapshot,routing,failures,setHandler:(next:typeof handler)=>{handler=next}}
}
it('routes Haiku search messages and counts to the exact search model and authentication',async()=>{
 const f=await fixture()
 const replies=[]
 for(const path of ['/v1/messages','/v1/messages/count_tokens']){const response=await f.send({},path);expect(response.status).toBe(200);replies.push(await response.json())}
 expect(replies).toEqual([expect.objectContaining({target:'search'}),expect.objectContaining({target:'search'})])
 expect(f.main.requests).toHaveLength(0)
 for(const request of f.searchUpstream.requests){expect(request.body.model).toBe('search-model');expect(request.headers.authorization).toBe('Bearer search-key');expect(request.headers.cookie).toBeUndefined();expect(request.body.messages[0].content).toBe('conversation context')}
 expect(f.searchUpstream.requests[0]!.body.max_tokens).toBe(512)
 expect(f.searchUpstream.requests[1]!.body.max_tokens).toBe(9999)
 const evidence=join(evidenceRoot,'evidence.json');await mkdir(evidenceRoot,{recursive:true});await writeFile(evidence,JSON.stringify({requests:f.searchUpstream.requests,replies},null,2))
})
it('keeps unconfigured snapshots on Haiku and does not reroute empty, mixed or fetch tools',async()=>{
 const old=await fixture(false);const reply=await(await old.send()).json();expect(reply).toMatchObject({target:'haiku'});expect(old.searchUpstream.requests).toHaveLength(0)
 await writeFile(join(evidenceRoot,'unconfigured.json'),JSON.stringify({requests:old.main.requests,reply,searchTargetRequests:old.searchUpstream.requests.length},null,2))
 const f=await fixture()
 for(const tools of [undefined,[],[{name:'client'}],[{type:'web_search_20250305'},{name:'client'}],[{type:'web_fetch_20250910'}],[{type:'web_search_20250305'},null]])expect(await(await f.send({tools})).json()).toMatchObject({target:'haiku'})
 expect(f.searchUpstream.requests).toHaveLength(0)
})
it.each(['manual','auto'])('gives %s compaction precedence over search, including counts',async marker=>{
 const f=await fixture()
 expect(await(await f.send({},'/v1/messages',marker)).json()).toMatchObject({target:'compaction'})
 expect(await(await f.send({},'/v1/messages/count_tokens',marker)).json()).toMatchObject({target:'haiku'})
 expect(f.searchUpstream.requests).toHaveLength(0)
})
it('refuses a blocked official search target without fallback',async()=>{
 const f=await fixture();(f.snapshot.webSearchTarget as {family:string}).family='chatgpt-subscription'
 const gateway=await startClaudeRequestGateway({upstreamUrl:f.main.url,env:{},resolveOverride:f.routing.resolveOverride,assertTargetAllowed:family=>{expect(family).toBe('chatgpt-subscription');throw new GatewayRoutingError('protected search')}});disposals.push(gateway.close)
 const response=await fetch(gateway.baseUrl+'/v1/messages',{method:'POST',body:JSON.stringify({model:'matou-role/haiku',tools:[{type:'web_search_20250305'}]})})
 expect(response.status).toBe(502);expect(f.searchUpstream.requests).toHaveLength(0);expect(f.main.requests).toHaveLength(0)
})
it.each([401,403])('rewrites subscription %s and notifies the real account service without exposing upstream diagnostics',async status=>{
 const f=await fixture(true,'subscription')
 const root=await mkdtemp(join(evidenceRoot,'account-'));disposals.push(()=>rm(root,{recursive:true,force:true}))
 const store=new ProviderConfigStore(root)
 const saved=await store.adoptSubscription({name:'Search supplier',modelMapping:{opus:'search-model',sonnet:'search-model',haiku:'search-model'},subscription:{provider:'codex',credentialRef:{name:'fixture.json',authIndex:'fixture-auth'},account:{status:'ready'},modelMetadata:[{id:'search-model'}]}})
 const accounts=new SubscriptionAccounts({dataRoot:root,store,manager:{onStateChange:()=>()=>{}} as unknown as ModelRouterManager,openAuthorizationUrl:async()=>{}});disposals.push(()=>accounts.close())
 const changed=vi.fn();accounts.onChange(changed)
 ;(f.snapshot.webSearchTarget as {providerId:string}).providerId=saved.id
 f.setHandler(input=>accounts.markNeedsSignIn(input.providerId,input.accountIdentity))
 f.searchUpstream.reply.status=status;f.searchUpstream.reply.body='secret upstream token=private-diagnostic'
 const response=await f.send();const body=await response.json()
 expect(response.status).toBe(status);expect(body.error.message).toBe('网页搜索 · Search supplier 登录已过期，请到 设置 → Claude Code 重新登录后重试。')
 expect(JSON.stringify(body)).not.toMatch(/private-diagnostic|search-key|fixture-auth/)
 const accountStatus=(await accounts.snapshot()).providers['claude-code'].find(p=>p.id===saved.id)?.subscription?.account.status
 expect(accountStatus).toBe('needs-sign-in');expect(changed).toHaveBeenCalledOnce();expect(f.main.requests).toHaveLength(0)
 await writeFile(join(evidenceRoot,`auth-${status}.json`),JSON.stringify({status:response.status,body,accountStatus,requests:f.searchUpstream.requests},null,2))
})
it('uses tier labels and English for OAuth expiry and handles explicit sign-in errors',async()=>{
 const f=await fixture(false,'oauth','en');Object.assign(f.snapshot.roles.haiku,{kind:'oauth',providerName:'OAuth supplier',accountIdentity:'official-identity'})
 f.main.reply.status=400;f.main.reply.body=JSON.stringify({error:{code:'invalid_grant',message:'secret diagnostic'}})
 const response=await f.send();expect(response.status).toBe(400);expect((await response.json()).error.message).toMatch(/haiku · OAuth supplier.*Settings → Claude Code/);expect(f.failures).toHaveLength(1)
})
it.each([401,403,429,500])('passes API key errors through unchanged at %s',async status=>{
 const f=await fixture();f.searchUpstream.reply.status=status;f.searchUpstream.reply.body='original error'
 const response=await f.send();expect(response.status).toBe(status);expect(await response.text()).toBe('original error');expect(f.failures).toHaveLength(0)
})
it.each(['quota exceeded','safety refusal','internal server error'])('does not mislabel subscription failures: %s',async message=>{
 const f=await fixture(true,'subscription');f.searchUpstream.reply.status=429;f.searchUpstream.reply.body=JSON.stringify({error:{message}})
 expect(await(await f.send()).text()).toBe(f.searchUpstream.reply.body);expect(f.failures).toHaveLength(0)
})
it('keeps revoked search targets failed without sending to main or search',async()=>{
 const f=await fixture(true,'subscription')
 ;(f.snapshot.webSearchTarget as {assertActive:()=>void}).assertActive=()=>{throw new Error('private revoked account diagnostic')}
 const response=await f.send();expect(response.status).toBe(502);expect(await response.text()).toMatch(/网页搜索.*Search supplier/)
 expect(f.main.requests).toHaveLength(0);expect(f.searchUpstream.requests).toHaveLength(0)
})
it('reports subscription expiry on count requests and ordinary tier requests',async()=>{
 const f=await fixture(true,'subscription')
 f.searchUpstream.reply.status=401
 const count=await f.send({},'/v1/messages/count_tokens');expect(count.status).toBe(401);expect(await count.text()).toMatch(/网页搜索/)
 Object.assign(f.snapshot.roles.haiku,{kind:'subscription',providerName:'Haiku account'})
 f.main.reply.status=403
 const main=await f.send({tools:[]});expect(main.status).toBe(403);expect(await main.text()).toMatch(/haiku · Haiku account/)
 expect(f.failures).toHaveLength(2)
})
it('keeps large non-auth errors byte-for-byte and successful SSE streams untouched',async()=>{
 const f=await fixture(true,'subscription')
 f.searchUpstream.reply.status=500;f.searchUpstream.reply.body='x'.repeat(300000)
 expect(await(await f.send()).text()).toBe(f.searchUpstream.reply.body)
 f.searchUpstream.reply.status=200;f.searchUpstream.reply.headers={'content-type':'text/event-stream'};f.searchUpstream.reply.body='event: message_start\ndata: {"type":"message_start"}\n\n'
 expect(await(await f.send()).text()).toBe(f.searchUpstream.reply.body);expect(f.failures).toHaveLength(0)
})
it('gives compaction-follow precedence over search',async()=>{
 const f=await fixture();Object.assign(f.snapshot.preferences.compaction,{mode:'follow'})
 expect(await(await f.send({},'/v1/messages','manual')).json()).toMatchObject({target:'haiku'})
 expect(f.searchUpstream.requests).toHaveLength(0)
})

it('recognizes explicit sign-in errors inside bounded gzip responses',async()=>{
 const f=await fixture(true,'subscription')
 f.searchUpstream.reply.status=400;f.searchUpstream.reply.headers={'content-encoding':'gzip'}
 f.searchUpstream.reply.body=gzipSync(JSON.stringify({error:{code:'invalid_grant',message:'private compressed diagnostic'}}))
 const response=await f.send();expect(response.status).toBe(400);expect(await response.text()).toMatch(/网页搜索.*登录已过期/);expect(f.failures).toHaveLength(1)
})
