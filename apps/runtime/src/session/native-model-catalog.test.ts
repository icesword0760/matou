import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, expect, it, vi } from 'vitest'
let root: string | undefined
afterEach(async () => { vi.unstubAllEnvs(); vi.resetModules(); if (root) await rm(root, { recursive: true, force: true }) })
it('does not reuse stale context numbers after a rejected model switch', async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-catalog-test-'))
  const executable = join(root, 'native.cjs')
  await writeFile(executable, `#!/usr/bin/env node
const readline=require('readline');let model='claude-sonnet-fixture';let size=200000;
readline.createInterface({input:process.stdin}).on('line',line=>{const {request_id,request}=JSON.parse(line);let response={},ok=true;
if(request.subtype==='initialize')response={models:[{value:'sonnet',resolvedModel:'claude-sonnet-fixture',supportedEffortLevels:['low','high']},{value:'haiku',resolvedModel:'claude-haiku-fixture'}]};
if(request.subtype==='set_model'){if(request.model==='sonnet'){model='claude-sonnet-fixture';size=200000}else ok=false;response={model};}
if(request.subtype==='get_context_usage')response={model,rawMaxTokens:size};
console.log(JSON.stringify({type:'control_response',response:{subtype:ok?'success':'error',request_id,response}}));});
`)
  await chmod(executable, 0o755); vi.stubEnv('MATOU_CLAUDE_COMMAND', executable)
  const { nativeClaudeModels } = await import('./native-model-catalog')
  const result = await nativeClaudeModels()
  expect(result.find(model => model.id === 'sonnet')).toMatchObject({ contextWindow: 200000, resolvedModel: 'claude-sonnet-fixture' })
  expect(result.find(model => model.id === 'sonnet')).not.toHaveProperty('maxContextWindow')
  expect(result.some(model => model.id.includes('haiku'))).toBe(false)
})
