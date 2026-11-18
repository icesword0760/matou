// Isolated real-component browser checks. All providers and RPC responses are synthetic.
import assert from 'node:assert/strict'
import { writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { createServer } from 'vite'
import { chromium } from '@playwright/test'

const evidence = path.dirname(fileURLToPath(import.meta.url))
const root = path.resolve(evidence, '../../../../..')
const desktop = path.join(root, 'apps/desktop')
const fixtureId = path.join(desktop, '.cache/search-auth-browser-fixture.tsx')
const fixture = `
import React, {useState} from 'react'
import {createRoot} from 'react-dom/client'
import {ClaudeRoleAssignments} from '../src/renderer/src/hierarchy/ClaudeRoleAssignments'
import {LocaleProvider} from '../src/renderer/src/i18n/LocaleProvider'
import '../src/renderer/src/hierarchy/hierarchy.css'
const api={id:'api',cli:'claude-code',name:'Fixture API',endpoint:'https://fixture.invalid',model:'a',hasApiKey:true,modelMetadata:[{id:'a'},{id:'b'}]}
const initial={preferences:{contextWindows:{},compaction:{mode:'follow',effort:'follow'},claudeRoleAssignments:{opus:{providerId:'api',model:'a'},sonnet:{providerId:'api',model:'a'},haiku:{providerId:'api',model:'a'}}},providers:[{id:'api',name:'Fixture API',cli:'claude-code',compactionAvailable:true,models:[{id:'a',name:'a',fixedTokens:512000},{id:'b',name:'b',fixedTokens:512000}]}]}
window.patches=[]
function Fixture(){
 const [snapshot,setSnapshot]=useState(initial)
 const client={request:async(method,payload)=>{window.patches.push({method,payload});const preferences={...snapshot.preferences,claudeRoleAssignments:payload.assignments};if(payload.webSearch)preferences.claudeWebSearch=payload.webSearch;else delete preferences.claudeWebSearch;return {...snapshot,preferences}}}
 return <LocaleProvider initialLocale={new URLSearchParams(location.search).get('lang')==='en'?'en':'zh-CN'}><div id="scroll" style={{height:650,overflow:'auto',padding:20}}><ClaudeRoleAssignments client={client} providers={[api]} activeProviderId="api" snapshot={snapshot} accounts={{}} onSignIn={()=>{}} onRetry={()=>{}} onSaved={setSnapshot}/><div style={{height:120}}/></div></LocaleProvider>
}
createRoot(document.getElementById('root')).render(<Fixture/>)
`
const server = await createServer({
  root: desktop, configFile: false, esbuild: { jsx: 'automatic' },
  server: { host: '127.0.0.1', port: 0 },
  plugins: [{ name: 'search-auth-fixture',
    resolveId(id) { if (id === '/fixture.tsx') return fixtureId },
    load(id) { if (id === fixtureId) return fixture },
    configureServer(s) { s.middlewares.use((req,res,next)=>{
      if (req.url?.split('?')[0] !== '/') return next()
      res.setHeader('Content-Type','text/html');res.end('<!doctype html><div id="root"></div><script type="module" src="/fixture.tsx"></script>')
    }) }
  }]
})
let browser
try {
  await server.listen()
  const port = server.httpServer.address().port
  browser = await chromium.launch({headless:true})
  const page = await browser.newPage({viewport:{width:900,height:900}})
  const errors=[];page.on('pageerror',e=>errors.push(e.message))
  await page.goto(`http://127.0.0.1:${port}/`)
  await page.getByLabel('网页搜索 供应商').waitFor()
  const measure = () => page.locator('.model-allocation__row').evaluateAll(rows=>rows.map(row=>{
    const fields=[...row.querySelectorAll('.model-allocation__field')].map(el=>{const b=el.getBoundingClientRect();return {x:b.x,y:b.y,width:b.width}})
    return {columns:getComputedStyle(row).gridTemplateColumns,fields}
  }))
  const wide=await measure();assert.equal(wide.length,4)
  assert.ok(wide.every(row=>Math.abs(row.fields[0].y-row.fields[1].y)<1))
  assert.ok(Math.abs(wide[3].fields[0].width-wide[0].fields[0].width)<1)
  await page.setViewportSize({width:560,height:900})
  const narrow=await measure()
  assert.ok(narrow.every(row=>row.fields[1].y>row.fields[0].y))
  assert.ok(narrow.every(row=>Math.abs(row.fields[0].x-row.fields[1].x)<1))
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth))
  await page.getByLabel('网页搜索 供应商').selectOption('api')
  await page.getByLabel('网页搜索 模型').selectOption('b')
  const save=page.getByRole('button',{name:'保存分配'})
  await save.scrollIntoViewIfNeeded();await save.focus()
  const before=await page.locator('#scroll').evaluate(el=>el.scrollTop)
  await page.keyboard.press('Enter')
  await page.getByText('模型分配已保存，新会话将使用此配置').waitFor()
  const after=await page.locator('#scroll').evaluate(el=>el.scrollTop)
  assert.equal(after,before)
  assert.equal(await save.evaluate(el=>el===document.activeElement),true)
  const patches=await page.evaluate(()=>window.patches)
  assert.deepEqual(patches[0].payload.webSearch,{providerId:'api',model:'b'})
  await page.keyboard.press('Space')
  assert.equal(await page.evaluate(()=>window.patches.length),1)
  await page.goto(`http://127.0.0.1:${port}/?lang=en`)
  await page.getByLabel('Web search provider').waitFor()
  assert.equal(await page.getByText('Unassigned: search requests use the current tier provider').count(),1)
  assert.deepEqual(errors,[])
  const result={scope:'isolated production component with synthetic RPC; no desktop app or real account',wide,narrow,save:{scrollBefore:before,scrollAfter:after,focusRetained:true,patches},english:true,pageErrors:errors}
  await writeFile(path.join(evidence,'browser-check.json'),JSON.stringify(result,null,2)+'\n')
  console.log('PASS: 900px columns, 560px vertical layout, no horizontal overflow, save focus/scroll, single patch, English copy')
} finally { await browser?.close();await server.close() }
