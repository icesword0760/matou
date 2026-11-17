import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
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
