import { createServer } from 'node:http'
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou, restartMatouGracefully } from './matou-fixture'

test('saves independent compaction and role effort, restores settings, and routes actual managed requests', async () => {
  test.setTimeout(100_000)
  const root = await mkdtemp('/tmp/matou-model-policy-ui-')
  const evidence = resolve('docs/acceptance/evidence/provider-settings-followup-2026-11-15')
  await mkdir(evidence, { recursive: true })
  const requests: Array<{ path: string; model: string; effort: string; key: string }> = []
  const server = createServer(async (req, res) => {
    if (req.method === 'GET') {
      res.setHeader('content-type', 'application/json')
      res.end(JSON.stringify({ data: [{ id: 'claude-summary-fixture', name: '摘要模型 A' }, { id: 'claude-summary-alternative', name: '摘要模型 B' }] }))
      return
    }
    const buffers = []; for await (const chunk of req) buffers.push(chunk)
    const message = JSON.parse(Buffer.concat(buffers).toString())
    requests.push({ path: req.url!, model: message.model, effort: message.output_config?.effort, key: String(req.headers['x-api-key'] ?? '') })
    res.setHeader('content-type', 'application/json'); res.end(JSON.stringify({ ok: true }))
  })
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve))
  const port = (server.address() as { port: number }).port
  const endpoint = `http://127.0.0.1:${port}`
  const capture = join(root, 'launch.json'), executable = join(root, 'capture.cjs')
  await mkdir(join(root, 'codex'), { recursive: true })
  await writeFile(join(root, 'codex/config.toml'), 'model = "gpt-fixture"\n')
  await writeFile(join(root, 'codex/models_cache.json'), JSON.stringify({ models: [{ slug: 'gpt-fixture', context_window: 272000, max_context_window: 872000 }] }))
  await writeFile(executable, `#!/usr/bin/env node
const fs=require('fs');const cp=require('child_process');const args=process.argv.slice(2);
if(args.includes('-p')) { const p=cp.spawn('claude',args,{env:process.env,stdio:'inherit'});p.on('exit',code=>process.exit(code??1)); }
else { const settings=JSON.parse(fs.readFileSync(args[args.indexOf('--settings')+1],'utf8')); const env={...process.env,...settings.env};
fs.writeFileSync(${JSON.stringify(capture)},JSON.stringify({window:settings.autoCompactWindow,model:args[args.indexOf('--model')+1],role:env.ANTHROPIC_DEFAULT_SONNET_MODEL}));
(async()=>{for(const compact of [false,true,false]){const r=await fetch(env.ANTHROPIC_BASE_URL+'/v1/messages?beta=true',{method:'POST',headers:{'content-type':'application/json','x-api-key':'SOURCE_FIXTURE',...(compact?{'x-claude-code-compaction':'manual'}:{})},body:JSON.stringify({model:env.ANTHROPIC_DEFAULT_SONNET_MODEL,output_config:{effort:'xhigh'},max_tokens:1024,messages:[{role:'user',content:'local fixture'}]})});if(!r.ok)console.log('FIXTURE_ERROR '+r.status);await r.text();}console.log('ROUTING_READY')})();setInterval(()=>{},1000); }
`)
  await chmod(executable, 0o755)
  const env = { MATOU_E2E_DISPLAY: 'primary', MATOU_CLAUDE_COMMAND: executable, CODEX_HOME: join(root, 'codex') }
  let fixture = await launchMatou({ env })
  try {
    let page = fixture.page
    await page.addInitScript(() => {
      const post = MessagePort.prototype.postMessage
      ;(globalThis as any).activationRequests = 0
      MessagePort.prototype.postMessage = function (message: any, ...args: any[]) {
        if (message?.type === 'rpc.request' && message.method === 'provider-config.activate') (globalThis as any).activationRequests++
        return post.call(this, message, ...args as [any])
      }
    })
    await page.reload()
    await page.getByRole('button', { name: '设置', exact: true }).click()
    const add = async (name: string, path: string, model: string, key: string, mapped: boolean) => {
      await page.getByRole('button', { name: '新增供应商' }).click()
      await page.getByRole('button', { name: /自定义服务/ }).click()
      await page.getByRole('button', { name: '下一步' }).click()
      const dialog = page.getByRole('dialog', { name: '配置 自定义服务' })
      await dialog.getByLabel('API 地址', { exact: true }).fill(endpoint + path)
      await dialog.getByLabel('API Key', { exact: true }).fill(key)
      await dialog.getByLabel('默认模型', { exact: true }).fill(model)
      await dialog.getByLabel('供应商名称', { exact: true }).fill(name)
      if (mapped) {
        for (const [role, effort] of [['opus', 'high'], ['sonnet', 'medium'], ['haiku', 'low']]) {
          const label = role![0]!.toUpperCase() + role!.slice(1)
          await dialog.getByLabel(`${label} 模型`, { exact: true }).selectOption({ label: '手动填写模型 ID…' })
          await dialog.getByLabel(`${label} 手动填写模型 ID`).fill(model)
          await dialog.getByLabel(`${label} 推理强度`).selectOption(effort!)
        }
        await dialog.screenshot({ path: join(evidence, 'model-provider-efforts.png') })
      }
      if (!mapped) {
        await dialog.getByRole('button', { name: '获取模型', exact: true }).click()
        await expect(dialog.getByRole('option', { name: /摘要模型 B/ })).toBeVisible()
      }
      await dialog.getByRole('button', { name: '仅保存', exact: true }).click()
      await expect(dialog).toHaveCount(0)
    }
    await add('本地工作模型', '/main', 'claude-main-fixture', 'SOURCE_FIXTURE', true)
    await add('本地摘要模型', '/summary', 'claude-summary-fixture', 'SUMMARY_FIXTURE', false)
    await page.getByRole('button', { name: '切换到 本地工作模型' }).click()
    await expect(page.getByText('默认供应商已切换', { exact: true })).toBeVisible()
    const mainCard = page.locator('.model-provider', { hasText: '本地工作模型' })
    await mainCard.getByRole('button', { name: '编辑' }).click()
    const editDialog = page.getByRole('dialog', { name: '编辑供应商' })
    await editDialog.getByRole('radio', { name: /^512K/ }).check()
    await editDialog.getByRole('button', { name: '保存并切换', exact: true }).click()
    await expect(editDialog).toHaveCount(0)
    await expect(mainCard.getByText('512K 窗口', { exact: true })).toBeVisible()
    await expect.poll(() => page.evaluate(() => (globalThis as any).activationRequests)).toBe(2)

    const settings = page.getByRole('region', { name: '对话压缩模型设置' })
    await settings.getByRole('radio', { name: /^独立配置/ }).check()
    await settings.getByLabel('压缩供应商').selectOption({ label: '本地摘要模型' })
    const modelSelect = settings.getByRole('combobox', { name: '压缩模型', exact: true })
    await expect(modelSelect.locator('option')).toHaveText(['请选择压缩模型', '摘要模型 A', '摘要模型 B', '手动填写模型 ID…'])
    await modelSelect.selectOption({ label: '摘要模型 B' })
    await expect(modelSelect.locator('option:checked')).toHaveText('摘要模型 B')
    await modelSelect.selectOption({ label: '摘要模型 A' })
    await settings.getByLabel('推理强度', { exact: true }).selectOption('low')
    await settings.getByRole('button', { name: '保存', exact: true }).click()
    await expect(settings.getByRole('status')).toContainText('已保存')
    await page.screenshot({ path: join(evidence, 'model-settings-configured.png') })
    fixture = await restartMatouGracefully(fixture, { env }); page = fixture.page
    await page.getByRole('button', { name: '设置', exact: true }).click()
    await expect(page.locator('.model-provider', { hasText: '本地工作模型' }).getByText('512K 窗口', { exact: true })).toBeVisible()
    await expect(page.getByRole('combobox', { name: '压缩模型', exact: true }).locator('option:checked')).toHaveText('摘要模型 A')
    await expect(page.getByLabel('推理强度', { exact: true })).toHaveValue('low')
    await page.locator('.model-settings__heading button').click()
    const terminal = page.locator('.scene-stage:not([hidden]) .terminal-surface').first()
    await expect(terminal).toHaveAttribute('data-pid', /^[1-9]\d*$/)
    const input = terminal.locator('.xterm-helper-textarea')
    await input.focus(); await input.pressSequentially('claude'); await input.press('Enter')
    await expect.poll(() => requests.length).toBe(3)
    expect(requests).toEqual([
      { path: '/main/v1/messages?beta=true', model: 'claude-main-fixture', effort: 'medium', key: 'SOURCE_FIXTURE' },
      { path: '/summary/v1/messages?beta=true', model: 'claude-summary-fixture', effort: 'low', key: 'SUMMARY_FIXTURE' },
      { path: '/main/v1/messages?beta=true', model: 'claude-main-fixture', effort: 'medium', key: 'SOURCE_FIXTURE' }
    ])
    const launch = JSON.parse(await readFile(capture, 'utf8'))
    expect(launch).toMatchObject({ window: 512000, role: 'matou-api/sonnet' })
    await writeFile(join(evidence, 'model-settings-routing.json'), JSON.stringify({ boundary: 'real Electron UI/runtime/PTY; local synthetic HTTP provider; no external model calls', launch, requests }, null, 2))
  } finally { await fixture.close(); server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); await rm(root, { recursive: true, force: true }) }
})
