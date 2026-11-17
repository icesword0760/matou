/** Browser actions use OpenCLI exclusively. Vite mounts the production settings
 * component; /rpc is only a transport to a real RuntimeServer + SQLite/store.
 * This is a local browser/runtime E2E, not packaged Electron acceptance. */
import { test, expect } from '@playwright/test'
import { EventEmitter } from 'node:events'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { createServer, type Server } from 'node:http'
import { mkdir, writeFile, readFile, chmod } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { createRequire } from 'node:module'
import { createServer as createViteServer } from 'vite'
import { PROTOCOL_VERSION, type RpcMethod, type RuntimeMessage } from '../../packages/contracts/src/index'
import type { PortMessageEvent, RuntimePort } from '../../apps/runtime/src/runtime-server'
import { build } from 'tsup'

const exec = promisify(execFile)
const repo = resolve(import.meta.dirname,'../..')
const evidence = resolve(repo,'docs/prd/model-provider-routing/evidence')
const sessionName = `matou-routing-${process.pid}`
const delay=(ms:number)=>new Promise(r=>setTimeout(r,ms))
async function until(check:()=>boolean|Promise<boolean>,timeout=15000) { const end=Date.now()+timeout;while(!await check()){if(Date.now()>end)throw Error('fixture timeout');await delay(30)} }
class BridgePort extends EventEmitter implements RuntimePort {
  readonly sent:RuntimeMessage[]=[]
  postMessage(message:RuntimeMessage){this.sent.push(message);this.emit('reply',message)}
  start(){}
  close(){}
  receive(data:unknown){this.emit('message',{data} satisfies PortMessageEvent)}
  async rpc(method:RpcMethod,payload:unknown={}) {
    const requestId=crypto.randomUUID()
    return new Promise<any>((resolve,reject)=>{
      const timer=setTimeout(()=>{this.off('reply',reply);reject(Error(`RPC timeout: ${method}`))},15000)
      const reply=(m:RuntimeMessage)=>{if((m.type==='rpc.response'||m.type==='rpc.error')&&m.requestId===requestId){clearTimeout(timer);this.off('reply',reply);if(m.type==='rpc.error')reject(Error(m.message));else resolve(m.result)}}
      this.on('reply',reply)
      this.receive({type:'rpc.request',protocolVersion:PROTOCOL_VERSION,capability:'renderer',requestId,method,payload,deadlineAt:Date.now()+14000})
    })
  }
}

test('OpenCLI settings save/reopen → real runtime routes three targets, counts and compaction without fallback',async()=>{
  test.setTimeout(180000)
  await mkdir(evidence,{recursive:true})
  const root=resolve(repo,'.cache/model-provider-routing-e2e',String(process.pid));await mkdir(root,{recursive:true})
  // Isolate every credential/config reader, including offline native catalogs.
  const previous={HOME:process.env.HOME,CODEX_HOME:process.env.CODEX_HOME,CLAUDE_CONFIG_DIR:process.env.CLAUDE_CONFIG_DIR,MATOU_CLAUDE_COMMAND:process.env.MATOU_CLAUDE_COMMAND,MATOU_LOCALE:process.env.MATOU_LOCALE}
  process.env.HOME=root;process.env.CODEX_HOME=join(root,'codex');process.env.CLAUDE_CONFIG_DIR=join(root,'claude');process.env.MATOU_LOCALE='zh-CN'
  const upstreams:Server[]=[],requests:Array<{target:string;path:string;key:string;model:string}>=[]
  const opencliLog:Array<{args:string[];output:string}>=[]
  const cli=async(...args:string[])=>{
    const result=await exec('opencli',['browser',sessionName,...args],{timeout:25000,maxBuffer:4*1024*1024,env:{...process.env,HOME:previous.HOME}})
    opencliLog.push({args,output:result.stdout});return result.stdout
  }
  const choose=async(selector:string,value:string)=>{await cli('find','--css',selector);await cli('select',selector,value);expect(await cli('get','value',selector)).toContain(value)}
  const local=async(target:string)=>{
    const upstream=createServer(async(req,res)=>{const chunks=[];for await(const c of req)chunks.push(c);const body=JSON.parse(Buffer.concat(chunks).toString());requests.push({target,path:req.url!,key:String(req.headers['x-api-key']??''),model:body.model});res.setHeader('content-type','application/json');res.end(JSON.stringify({input_tokens:3,content:[],usage:{input_tokens:3,output_tokens:1}}))})
    upstreams.push(upstream);await new Promise<void>(r=>upstream.listen(0,'127.0.0.1',r));return `http://127.0.0.1:${(upstream.address() as {port:number}).port}`
  }
  const executable=join(root,'local-cli.cjs'),report=join(root,'cli-report.json')
  await writeFile(executable,`#!/usr/bin/env node
const fs=require('fs'),readline=require('readline');const metadata=process.argv.includes('--input-format');let model='claude-sonnet-local';
if(metadata){readline.createInterface({input:process.stdin}).on('line',line=>{try{const row=JSON.parse(line),q=row.request;let response={};if(q.subtype==='initialize')response={models:['default','opus','sonnet','haiku'].map(value=>({value,displayName:'Native '+value,resolvedModel:'claude-'+(value==='default'?'sonnet':value)+'-local'}))};if(q.subtype==='set_model')model='claude-'+(q.model.replace(/\\[1m\\]/i,'')==='default'?'sonnet':q.model.replace(/\\[1m\\]/i,''))+'-local';if(q.subtype==='get_context_usage')response={model,rawMaxTokens:200000};console.log(JSON.stringify({type:'control_response',response:{subtype:'success',request_id:row.request_id,response}}))}catch{}})}else{(async()=>{const results=[];for(const role of ['OPUS','SONNET','HAIKU'])for(const path of ['/v1/messages','/v1/messages/count_tokens']){const response=await fetch(process.env.ANTHROPIC_BASE_URL+path,{method:'POST',body:JSON.stringify({model:process.env['ANTHROPIC_DEFAULT_'+role+'_MODEL'],max_tokens:1000,messages:[]})});results.push({role,path,status:response.status})}const compact=await fetch(process.env.ANTHROPIC_BASE_URL+'/v1/messages',{method:'POST',headers:{'x-claude-code-compaction':'manual'},body:JSON.stringify({model:process.env.ANTHROPIC_DEFAULT_SONNET_MODEL,messages:[]})});results.push({compact:true,status:compact.status});const denied=await fetch(process.env.ANTHROPIC_BASE_URL+'/v1/messages',{method:'POST',body:JSON.stringify({model:'matou-role/invalid',messages:[]})});results.push({invalid:true,status:denied.status,body:await denied.text()});fs.writeFileSync(${JSON.stringify(report)},JSON.stringify({pid:process.pid,results}));console.log('LOCAL_DONE');setInterval(()=>{},1000)})().catch(e=>{fs.writeFileSync(${JSON.stringify(report)},JSON.stringify({error:String(e)}));process.exit(1)})}
`);await chmod(executable,0o755);process.env.MATOU_CLAUDE_COMMAND=executable
  const runtimeRequire=createRequire(resolve(repo,'apps/runtime/package.json'))
  const nodePty=runtimeRequire.resolve('node-pty')
  const entry=join(root,'runtime-entry.ts')
  await writeFile(entry,[['RuntimeServer','runtime-server'],['RuntimeDatabase','storage/database'],['MigrationRunner','storage/migration-runner'],['FOUNDATION_MIGRATIONS','storage/migrations'],['ProviderConfigStore','provider-config/provider-config-store'],['RuntimeSessionRegistry','session/runtime-session-registry']].map(([name,path])=>`export {${name}} from ${JSON.stringify(resolve(repo,'apps/runtime/src',path+'.ts'))}`).join('\n'))
  await build({entry:{fixture:entry},outDir:join(root,'bundle'),format:['cjs'],target:'node22',platform:'node',config:false,silent:true,noExternal:['@matou/contracts','@matou/domain','@xterm/headless','@xterm/addon-serialize','zod','https-proxy-agent','socks-proxy-agent'],esbuildPlugins:[{name:'local-pty',setup(builder){builder.onResolve({filter:/^node-pty$/},()=>({path:nodePty,external:true}))}}]})
  const {RuntimeServer,RuntimeDatabase,MigrationRunner,FOUNDATION_MIGRATIONS,ProviderConfigStore,RuntimeSessionRegistry}=createRequire(import.meta.url)(join(root,'bundle/fixture.cjs'))
  const db=RuntimeDatabase.open(join(root,'matou.sqlite'));await new MigrationRunner(db,FOUNDATION_MIGRATIONS).migrate()
  const providers=new ProviderConfigStore(root)
  const a=await providers.upsert({cli:'claude-code',name:'GPT local fixture',endpoint:await local('opus'),apiKey:'local-a',model:'gpt-local',modelMetadata:[{id:'gpt-local',displayName:'GPT local'}]})
  const b=await providers.upsert({cli:'claude-code',name:'DeepSeek local fixture',endpoint:await local('sonnet'),apiKey:'local-b',model:'deepseek-local',modelMetadata:[{id:'deepseek-local',displayName:'DeepSeek local'}]})
  const c=await providers.upsert({cli:'claude-code',name:'Default local fixture',endpoint:await local('haiku'),apiKey:'local-c',model:'sonnet',modelMapping:{opus:'opus-local',sonnet:'sonnet-local',haiku:'haiku-local'},modelMetadata:[{id:'opus-local',displayName:'Opus local'},{id:'sonnet-local',displayName:'Sonnet local'},{id:'haiku-local',displayName:'Haiku local'}]})
  await providers.activate('claude-code',c.id)
  const port=new BridgePort(),registry=new RuntimeSessionRegistry()
  const runtime=new RuntimeServer(port,root,db,undefined,undefined,registry,undefined,undefined,{providerConfigs:providers})
  port.receive({type:'protocol.hello',protocolVersion:PROTOCOL_VERSION,clientId:'opencli-local'})
  await until(()=>port.sent.some(m=>m.type==='protocol.ready'))
  const vite=await createViteServer({configFile:false,root:repo,resolve:{alias:{'react':resolve(repo,'apps/desktop/node_modules/react'),'react-dom':resolve(repo,'apps/desktop/node_modules/react-dom')}},server:{host:'127.0.0.1',port:0},appType:'custom',plugins:[(await import(createRequire(resolve(repo,'apps/desktop/package.json')).resolve('@vitejs/plugin-react'))).default()]})
  vite.middlewares.use(async(req,res,next)=>{
    if(req.url==='/rpc'&&req.method==='POST'){
      try{const chunks=[];for await(const c of req)chunks.push(c);const {method,payload}=JSON.parse(Buffer.concat(chunks).toString());const value=await port.rpc(method,payload);res.setHeader('content-type','application/json');res.end(JSON.stringify({value}))}catch(e){res.statusCode=500;res.end(JSON.stringify({error:String(e)}))}return
    }
    if(req.url?.split('?')[0]==='/routing-test'){
      const html=`<html><head><meta name="viewport" content="width=device-width,initial-scale=1"/><title>Local runtime acceptance</title></head><body style="margin:0"><div id="root"></div><script type="module" src="/.cache/model-provider-routing-e2e/${process.pid}/mount.tsx"></script></body></html>`
      res.setHeader('content-type','text/html');res.end(await vite.transformIndexHtml(req.url,html));return
    }next()
  })
  await writeFile(join(root,'mount.tsx'),`import React,{useState} from 'react';import {createRoot} from 'react-dom/client';import {ModelSwitchSettings} from '/apps/desktop/src/renderer/src/hierarchy/ModelSwitchSettings.tsx';import {LocaleProvider} from '/apps/desktop/src/renderer/src/i18n/LocaleProvider.tsx';import '/apps/desktop/src/renderer/src/hierarchy/hierarchy.css';const client={request:async(method,payload)=>{const response=await fetch('/rpc',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({method,payload})});const data=await response.json();if(!response.ok)throw Error(data.error);return data.value}};function App(){const[open,setOpen]=useState(true);return <LocaleProvider initialLocale="zh-CN">{open?<ModelSwitchSettings client={client} onClose={()=>setOpen(false)}/>:<button id="reopen-settings" onClick={()=>setOpen(true)}>设置</button>}</LocaleProvider>};createRoot(document.getElementById('root')).render(<App/>);`)
  await vite.listen();const address=vite.httpServer!.address() as {port:number};const url=`http://127.0.0.1:${address.port}/routing-test`
  try{
    await cli('open',url);await cli('wait','selector','#opus-provider');await cli('state')
    await cli('find','--css','#opus-provider');await cli('focus','#opus-provider')
    await cli('screenshot',join(evidence,'default.png'))
    await choose('#opus-provider',a.id);await choose('#opus-model','gpt-local')
    await choose('#sonnet-provider',b.id);await choose('#sonnet-model','deepseek-local')
    await cli('screenshot',join(evidence,'editing.png'))
    await cli('find','--css','.model-allocation__actions .primary');await cli('focus','.model-allocation__actions .primary')
    const beforeSave=JSON.parse(await cli('eval',`(()=>({scroll:document.querySelector('.model-settings__main')?.scrollTop}))()`))
    await cli('click','.model-allocation__actions .primary')
    await cli('wait','text','模型分配已保存，新会话将使用此配置')
    const focus=await cli('eval',`(()=>({focus:document.activeElement?.textContent,disabled:document.activeElement?.getAttribute('aria-disabled'),scroll:document.querySelector('.model-settings__main')?.scrollTop}))()`)
    expect(focus).toContain('保存分配');expect(focus).toContain('true');expect(JSON.parse(focus).scroll).toBe(beforeSave.scroll)
    await cli('screenshot',join(evidence,'saved.png'))
    await cli('state');await cli('find','--css','.model-settings__heading button');await cli('click','.model-settings__heading button')
    await cli('state');await cli('click','#reopen-settings');await cli('wait','selector','#opus-provider');await cli('state')
    expect(await cli('get','value','#opus-provider')).toContain(a.id);expect(await cli('get','value','#sonnet-model')).toContain('deepseek-local')
    const preferences=await port.rpc('agent-model-settings.get')
    expect(preferences.preferences.claudeRoleAssignments).toEqual({opus:{providerId:a.id,model:'gpt-local'},sonnet:{providerId:b.id,model:'deepseek-local'}})
    // Actual runtime launch consumes the settings saved by the browser, not a
    // UI mock. Default Haiku is a third independent local endpoint.
    db.run('INSERT INTO workspaces(id,name,root_directory,created_at,updated_at)VALUES(?,?,?,?,?)','w','Local',root,1,1)
    db.run("INSERT INTO execution_contexts(id,workspace_id,kind,cwd,created_at)VALUES('ctx','w','plain-directory',?,1)",root)
    db.run("INSERT INTO tasks(id,workspace_id,execution_context_id,title,status,created_at,updated_at)VALUES('task','w','ctx','Local','active',1,1)")
    db.run("INSERT INTO sessions(id,task_id,execution_context_id,kind,status,title,created_at,updated_at,last_activity_at)VALUES('routed','task','ctx','claude-code','exited','Local',1,1,1)")
    port.receive({type:'terminal.spawn',protocolVersion:PROTOCOL_VERSION,sessionId:'routed',executionContextId:'ctx',profile:'claude-code',cols:80,rows:24})
    await until(async()=>{try{return Boolean(JSON.parse(await readFile(report,'utf8')).results)}catch{return false}})
    const result=JSON.parse(await readFile(report,'utf8'));expect(result.results.filter((r:any)=>!r.invalid).every((r:any)=>r.status===200)).toBe(true)
    expect(result.results.find((r:any)=>r.invalid).status).toBe(502)
    expect(requests).toHaveLength(7)
    expect(requests.filter(r=>r.target==='opus').map(r=>[r.model,r.key])).toEqual([['gpt-local','local-a'],['gpt-local','local-a']])
    expect(requests.filter(r=>r.target==='sonnet')).toHaveLength(3)
    expect(requests.filter(r=>r.target==='haiku').map(r=>[r.model,r.key])).toEqual([['haiku-local','local-c'],['haiku-local','local-c']])
    const pid=registry.get('routed')!.pid
    // Interleave compaction and the dedicated assignments patch; neither owns
    // the other's fields, and changing a setting never restarts this process.
    await Promise.all([port.rpc('agent-model-settings.set',{...preferences.preferences,compaction:{mode:'custom',providerId:a.id,model:'gpt-local',effort:'follow'}}),port.rpc('agent-model-settings.patch-assignments',{assignments:preferences.preferences.claudeRoleAssignments})])
    await choose('#sonnet-model','deepseek-local')
    await cli('find','--css','.model-allocation__actions button:first-child');await cli('click','.model-allocation__actions button:first-child')
    expect((await port.rpc('agent-model-settings.get')).preferences.claudeRoleAssignments).not.toEqual({})
    await cli('find','--css','.model-allocation__actions .primary');await cli('click','.model-allocation__actions .primary');await cli('wait','text','模型分配已保存')
    const restored=await port.rpc('agent-model-settings.get');expect(restored.preferences.claudeRoleAssignments).toEqual({});expect(restored.preferences.compaction.providerId).toBe(a.id)
    expect(registry.get('routed')!.pid).toBe(pid)
    // Confirm authoritative deletion is an inline retained-value error on reopen.
    await port.rpc('agent-model-settings.patch-assignments',{assignments:{opus:{providerId:a.id,model:'gpt-local'}}})
    await providers.upsert({id:a.id,cli:'claude-code',name:a.name,endpoint:a.endpoint,model:'gpt-local',modelMetadata:[{id:'replacement'}]})
    await cli('state');await cli('click','.model-settings__heading button');await cli('state');await cli('click','#reopen-settings');await cli('wait','text','模型已不可用')
    expect(await cli('get','value','#opus-model')).toContain('gpt-local');await cli('find','--css','#opus-model');await cli('focus','#opus-model');await cli('screenshot',join(evidence,'error-model.png'));await cli('screenshot',join(evidence,'narrow-560.png'),'--width','560','--height','1000')
    await writeFile(join(evidence,'requests.json'),JSON.stringify({requests,result,pid,persisted:restored.preferences},null,2))
  }finally{
    await writeFile(join(evidence,'opencli.json'),JSON.stringify(opencliLog,null,2))
    await cli('close').catch(()=>{});runtime.close();await registry.shutdownAll();await vite.close();db.close()
    for(const upstream of upstreams){upstream.closeAllConnections();await new Promise<void>(r=>upstream.close(()=>r()))}
    for(const [name,value]of Object.entries(previous)){if(value===undefined)delete process.env[name];else process.env[name]=value}
  }
})
