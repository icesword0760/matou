import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { createServer } from 'node:http'
import { afterEach, expect, it, vi } from 'vitest'
import { PtySession } from './pty-session'
import { prepareClaudeRoleLaunch } from './claude-role-launch'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
vi.mock('./native-model-catalog', () => ({ nativeClaudeModels: async () => [{ id: 'default', resolvedModel: 'claude-sonnet-fixture' }, { id: 'sonnet', resolvedModel: 'claude-sonnet-fixture' }] }))
const disposals: Array<() => Promise<void>> = []
afterEach(async () => { for (const close of disposals.splice(0).reverse()) await close(); vi.unstubAllEnvs() })
const until = async (check: () => boolean) => { const end = Date.now()+5000; while (!check()) { if (Date.now()>end) throw Error('fixture timeout'); await new Promise(r=>setTimeout(r,10)) } }
it('launches a real PTY with stable main/menu/subagent role identities and routes its messages and counts', async () => {
 const root = await mkdtemp(join(tmpdir(),'role-pty-')); disposals.push(()=>rm(root,{recursive:true,force:true}))
 const requests: any[] = []
 const upstream = createServer(async(req,res)=>{const chunks=[];for await(const c of req)chunks.push(c);requests.push({url:req.url,headers:req.headers,body:JSON.parse(Buffer.concat(chunks).toString())});res.end('{"input_tokens":1}')})
 await new Promise<void>(r=>upstream.listen(0,'127.0.0.1',r)); disposals.push(async()=>{upstream.closeAllConnections();await new Promise<void>(r=>upstream.close(()=>r()))})
 const base=`http://127.0.0.1:${(upstream.address() as {port:number}).port}`
 const store=new ProviderConfigStore(root), provider=await store.upsert({cli:'claude-code',name:'API',endpoint:base,apiKey:'fixture-key',model:'sonnet',modelMapping:{opus:'same',sonnet:'same',haiku:'same'}})
 await store.activate('claude-code',provider.id)
 const prefs={contextWindows:{},compaction:{mode:'follow' as const,effort:'follow' as const},claudeRoleAssignments:{opus:{providerId:provider.id,model:'same'}}}
 const prepared=(await prepareClaudeRoleLaunch({preferences:prefs,store,env:{}}))!
 const settingsPath=join(root,'settings.json');await writeFile(settingsPath,JSON.stringify({hooks:{fixture:true},env:{CLAUDE_CODE_MAX_OUTPUT_TOKENS:'7'}}))
 const executable=join(root,'fixture.cjs'),capture=join(root,'capture.json')
 await writeFile(executable,`#!/usr/bin/env node
const fs=require('fs'); const argv=process.argv.slice(2),settings=JSON.parse(fs.readFileSync(argv[argv.indexOf('--settings')+1]));
fs.writeFileSync(${JSON.stringify(capture)},JSON.stringify({env:Object.fromEntries(Object.entries(process.env).filter(([k])=>k.startsWith('ANTHROPIC_DEFAULT_')||['CLAUDE_CODE_MAX_OUTPUT_TOKENS','CLAUDE_CODE_MAX_CONTEXT_TOKENS'].includes(k))),argv,settings:{modelPicker:settings.modelPicker,modelOverrides:settings.modelOverrides}}));console.log('READY');
require('readline').createInterface({input:process.stdin}).on('line',async line=>{try{const {role,path}=JSON.parse(line);const model=process.env['ANTHROPIC_DEFAULT_'+role.toUpperCase()+'_MODEL'];await fetch(process.env.ANTHROPIC_BASE_URL+path,{method:'POST',headers:{authorization:'Bearer source-fixture'},body:JSON.stringify({model,messages:[],max_tokens:9999})});console.log('DONE')}catch(e){console.log('ERROR')}})
`);await chmod(executable,0o755);vi.stubEnv('MATOU_CLAUDE_COMMAND',executable)
 let output=''
 const session=await PtySession.create({sessionId:'roles',executionContextId:'context',cwd:root,dataRoot:root,cols:100,rows:24,profile:'claude-code',settingsPath,model:prepared.resolveMainModel('default'),contextWindow:prepared.contextWindow,modelRouting:{preferences:prefs,store,roleSnapshot:prepared.snapshot,releaseTargets:prepared.close},onOutput:data=>{output+=data}})
 disposals.push(()=>session.shutdownForRuntime({gracePeriodMs:100,hardKillWaitMs:1000}))
 await until(()=>output.includes('READY'))
 const captured=JSON.parse(await readFile(capture,'utf8'))
 expect(captured.argv).toContain('matou-role/sonnet')
 expect(captured.env.ANTHROPIC_DEFAULT_OPUS_MODEL).toBe('matou-role/opus')
 expect(captured.env.ANTHROPIC_DEFAULT_HAIKU_MODEL).toBe('matou-role/haiku')
 expect(captured.env.CLAUDE_CODE_MAX_OUTPUT_TOKENS).toBeUndefined()
 expect(captured.settings.modelPicker.replaceBuiltInOptions).toBe(true)
 expect(captured.settings.modelOverrides['claude-fable-5']).toBe('matou-role/opus-fallback')
 session.write(JSON.stringify({role:'opus',path:'/v1/messages'})+'\n')
 await until(()=>requests.length===1)
 session.write(JSON.stringify({role:'haiku',path:'/v1/messages/count_tokens'})+'\n')
 await until(()=>requests.length===2)
 expect(requests.map(r=>r.body.model)).toEqual(['same','same'])
 expect(requests.map(r=>r.headers['x-api-key'])).toEqual(['fixture-key','fixture-key'])
})
it('owns prepared subscription leases until exit, skips the old subscription projection, and releases on spawn failure', async () => {
 const root = await mkdtemp(join(tmpdir(),'role-pty-lease-')); disposals.push(()=>rm(root,{recursive:true,force:true}))
 const executable=join(root,'fixture.cjs');await writeFile(executable,'#!/usr/bin/env node\nconsole.log("READY");setInterval(()=>{},1000)\n');await chmod(executable,0o755)
 vi.stubEnv('MATOU_CLAUDE_COMMAND',executable)
 const store=new ProviderConfigStore(root)
 const sub=await store.adoptSubscription({name:'Fixture',modelMapping:{opus:'gpt',sonnet:'gpt',haiku:'gpt'},subscription:{provider:'codex',credentialRef:{name:'fixture.json'},account:{status:'ready'},modelMetadata:[{id:'gpt',contextWindow:200000}]}})
 await store.activate('claude-code',sub.id)
 let leases=0,oldProjection=0
 const accounts={prepareModelTarget:async(_id:string,model:string)=>{leases++;return{accountIdentity:'fixture-account',assertActive:()=>{},lease:{release:()=>{leases--}},baseUrl:'http://127.0.0.1:1234',headers:{authorization:'Bearer fixture'},model:`matou-chatgpt/${model}`,metadata:{id:model,contextWindow:200000}}},prepareLaunch:async()=>{oldProjection++;throw Error('old projection must not run')}} as unknown as import('../model-router/subscription-accounts').SubscriptionAccounts
 const prefs={contextWindows:{},compaction:{mode:'follow' as const,effort:'follow' as const},claudeRoleAssignments:{opus:{providerId:sub.id,model:'gpt'}}}
 const prepare=async()=>{const p=(await prepareClaudeRoleLaunch({preferences:prefs,store,accounts,env:{}}))!;return{p,options:{sessionId:'lease',executionContextId:'context',cwd:root,dataRoot:root,cols:80,rows:24,profile:'claude-code' as const,model:p.resolveMainModel(p.sourceModel),contextWindow:p.contextWindow,env:p.sourceEnvironment,subscription:{accounts,providerConfigId:sub.id},modelRouting:{preferences:prefs,store,roleSnapshot:p.snapshot,sourceConnection:p.sourceConnection,releaseTargets:p.close}}}}
 const live=await prepare();const session=await PtySession.create(live.options);disposals.push(()=>session.shutdownForRuntime({gracePeriodMs:100,hardKillWaitMs:1000}))
 expect(leases).toBe(1);expect(oldProjection).toBe(0)
 await session.shutdownForRuntime({gracePeriodMs:100,hardKillWaitMs:1000});expect(leases).toBe(0)
 vi.stubEnv('MATOU_CLAUDE_COMMAND',join(root,'missing-cli'))
 const failed=await prepare();const exited=await PtySession.create({...failed.options,sessionId:'failed'});disposals.push(()=>exited.shutdownForRuntime({gracePeriodMs:100,hardKillWaitMs:1000}));await exited.whenClosed();expect(leases).toBe(0)
 const cancelled=await prepare();await expect(PtySession.create({...cancelled.options,sessionId:'cancelled',modelRouting:{...cancelled.options.modelRouting,isClosed:()=>true}})).rejects.toThrow(/closed/i);expect(leases).toBe(0)
})
it('wires a running search request failure through the PTY gateway to subscription account state',async()=>{
 const {SubscriptionAccounts}=await import('../model-router/subscription-accounts')
 const root=await mkdtemp(join(tmpdir(),'search-auth-pty-'));disposals.push(()=>rm(root,{recursive:true,force:true}))
 const requests:any[]=[]
 const upstream=createServer(async(req,res)=>{const chunks:Buffer[]=[];for await(const c of req)chunks.push(c);requests.push({headers:req.headers,body:JSON.parse(Buffer.concat(chunks).toString())});res.writeHead(401);res.end('{"error":{"message":"private upstream diagnostic"}}')})
 await new Promise<void>(r=>upstream.listen(0,'127.0.0.1',r));disposals.push(async()=>{upstream.closeAllConnections();await new Promise<void>(r=>upstream.close(()=>r()))})
 const base=`http://127.0.0.1:${(upstream.address() as {port:number}).port}`
 const store=new ProviderConfigStore(root)
 const main=await store.upsert({cli:'claude-code',name:'Main',endpoint:'http://127.0.0.1:1',apiKey:'fixture-main',model:'sonnet',modelMapping:{opus:'same',sonnet:'same',haiku:'same'}});await store.activate('claude-code',main.id)
 const search=await store.adoptSubscription({name:'Search account',modelMapping:{opus:'gpt',sonnet:'gpt',haiku:'gpt'},subscription:{provider:'codex',credentialRef:{name:'search.json',authIndex:'current'},account:{status:'ready'},modelMetadata:[{id:'gpt',contextWindow:8000}]}})
 const accounts=new SubscriptionAccounts({dataRoot:root,store,manager:{onStateChange:()=>()=>{}} as unknown as import('../model-router/manager').ModelRouterManager,openAuthorizationUrl:async()=>{}});disposals.push(()=>accounts.close())
 vi.spyOn(accounts,'prepareModelTarget').mockResolvedValue({targetSignal:new AbortController().signal,accountIdentity:'["search.json","current"]',assertActive:()=>{},baseUrl:base,headers:{authorization:'Bearer fixture-search'},model:'matou-chatgpt/gpt',metadata:{id:'gpt',contextWindow:8000},lease:{connection:{baseUrl:base,clientKey:'fixture',managementKey:'fixture',pid:1},release:()=>{}}})
 const preferences={contextWindows:{},compaction:{mode:'follow' as const,effort:'follow' as const},claudeWebSearch:{providerId:search.id,model:'gpt'}}
 const prepared=(await prepareClaudeRoleLaunch({preferences,store,accounts,env:{}}))!
 const executable=join(root,'fixture.cjs')
 await writeFile(executable,`#!/usr/bin/env node
fetch(process.env.ANTHROPIC_BASE_URL+'/v1/messages',{method:'POST',body:JSON.stringify({model:process.env.ANTHROPIC_DEFAULT_HAIKU_MODEL,messages:[{role:'user',content:'local query'}],tools:[{type:'web_search_20250305',name:'web_search'}]})}).then(async r=>{console.log('RESULT '+r.status+' '+await r.text());setInterval(()=>{},1000)})
`);await chmod(executable,0o755);vi.stubEnv('MATOU_CLAUDE_COMMAND',executable)
 let output=''
 const session=await PtySession.create({sessionId:'search-auth',executionContextId:'context',cwd:root,dataRoot:root,cols:100,rows:24,profile:'claude-code',model:prepared.resolveMainModel('default'),modelRouting:{preferences,store,accounts,roleSnapshot:prepared.snapshot,releaseTargets:prepared.close},onOutput:data=>{output+=data}});disposals.push(()=>session.shutdownForRuntime({gracePeriodMs:100,hardKillWaitMs:1000}))
 await until(()=>output.includes('RESULT 401'))
 expect(requests).toHaveLength(1);expect(requests[0].body.model).toBe('matou-chatgpt/gpt');expect(requests[0].headers.authorization).toBe('Bearer fixture-search')
 expect(output).toContain('网页搜索 · Search account');expect(output).not.toContain('private upstream diagnostic')
 const accountStatus=(await accounts.snapshot()).providers['claude-code'].find(p=>p.id===search.id)!.subscription!.account.status
 expect(accountStatus).toBe('needs-sign-in')
 const evidenceRoot=resolve(import.meta.dirname,'../../../../node_modules/.cache/search-auth');await mkdir(evidenceRoot,{recursive:true})
 await writeFile(join(evidenceRoot,'pty-auth.json'),JSON.stringify({requests,terminalOutput:output,accountStatus},null,2))
})
