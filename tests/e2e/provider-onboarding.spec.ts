import { createServer } from 'node:http'
import { mkdir, readFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou } from './matou-fixture'

const evidence = resolve('docs/acceptance/evidence/provider-onboarding')
test('preset and custom onboarding, errors, save-only, activate, edit and compact layout', async () => {
  test.setTimeout(90_000)
  let failed = true
  const requests: string[] = []
  const server = createServer((req,res) => {
    requests.push(req.url ?? '')
    res.writeHead(failed ? 401 : 200, { 'Content-Type':'application/json' })
    res.end(JSON.stringify(failed ? {error:'fixture auth error'} : {data:[{id:'fixture-model-a'},{id:'fixture-model-b'}]}))
  })
  await new Promise<void>(r => server.listen(0,'127.0.0.1',r))
  const address = server.address() as {port:number}
  const fixture = await launchMatou({env:{MATOU_E2E_DISPLAY:'primary'}})
  try {
    await mkdir(evidence,{recursive:true})
    await fixture.app.evaluate(({BrowserWindow})=>BrowserWindow.getAllWindows()[0]!.setSize(1280,900))
    await fixture.page.getByRole('button',{name:'设置',exact:true}).click()
    await expect(fixture.page.getByRole('heading',{name:'服务供应商'})).toBeVisible()
    await fixture.page.getByRole('button',{name:'新增供应商'}).click()
    let dialog = fixture.page.getByRole('dialog',{name:'添加供应商'})
    await expect(dialog.getByRole('searchbox')).toBeFocused()
    await dialog.getByRole('button',{name:/DeepSeek/}).click()
    await dialog.screenshot({path:join(evidence,'01-presets.png')})
    await dialog.getByRole('button',{name:'下一步'}).click()
    dialog = fixture.page.getByRole('dialog',{name:'配置 DeepSeek'})
    await expect(dialog.getByLabel('API Key',{exact:true})).toBeFocused()
    await expect(dialog.getByLabel('默认模型',{exact:true})).toHaveValue('deepseek-v4-pro')
    await dialog.screenshot({path:join(evidence,'02-preset-form.png')})
    await dialog.getByRole('button',{name:'更换',exact:true}).click()
    dialog = fixture.page.getByRole('dialog',{name:'添加供应商'})
    await dialog.getByRole('button',{name:/自定义服务/}).click()
    await dialog.getByRole('button',{name:'下一步'}).click()
    dialog = fixture.page.getByRole('dialog',{name:'配置 自定义服务'})
    await dialog.getByLabel('API Key',{exact:true}).fill('SYNTHETIC_LOCAL_TOKEN')
    await dialog.getByLabel('API 地址',{exact:true}).fill(`http://127.0.0.1:${address.port}/v1`)
    await dialog.getByLabel('供应商名称',{exact:true}).fill('本地验收服务')
    await dialog.getByRole('button',{name:'获取模型',exact:true}).click()
    await expect(dialog).toContainText('密钥未通过验证')
    await expect(dialog.getByLabel('API Key',{exact:true})).toHaveValue('SYNTHETIC_LOCAL_TOKEN')
    await dialog.screenshot({path:join(evidence,'03-discovery-error.png')})
    failed = false
    await dialog.getByRole('button',{name:'获取模型',exact:true}).click()
    await dialog.getByRole('option',{name:'fixture-model-b'}).click()
    await dialog.getByRole('button',{name:'仅保存',exact:true}).click()
    await expect(dialog).toHaveCount(0)
    await expect(fixture.page.getByRole('button',{name:'切换到 本地验收服务'})).toBeEnabled()
    let disk = JSON.parse(await readFile(join(fixture.dataDirectory,'provider-config.json'),'utf8'))
    expect(disk.activeProviderIds['claude-code']).toBe('anthropic-official')
    await fixture.page.getByRole('button',{name:'切换到 本地验收服务'}).click()
    await expect(fixture.page.getByText('默认供应商已切换',{exact:true})).toBeVisible()
    await expect(fixture.page.getByRole('button',{name:'本地验收服务 当前配置'})).toBeDisabled()
    await fixture.page.locator('.model-settings__main').screenshot({path:join(evidence,'04-switched.png')})
    disk = JSON.parse(await readFile(join(fixture.dataDirectory,'provider-config.json'),'utf8'))
    expect(disk.activeProviderIds['claude-code']).toBe(disk.providers['claude-code'].find((p: {name:string})=>p.name==='本地验收服务').id)
    expect(requests).toEqual(['/v1/models','/v1/models'])
    // Reopen: persisted status and retained secret, not a transient success label.
    await fixture.page.getByRole('button',{name:'关闭设置'}).click()
    await fixture.page.getByRole('button',{name:'设置',exact:true}).click()
    await expect(fixture.page.getByRole('button',{name:'本地验收服务 当前配置'})).toBeDisabled()
    await fixture.page.getByRole('button',{name:'新增供应商'}).click()
    await fixture.page.getByRole('button',{name:/DeepSeek/}).click()
    await fixture.page.getByRole('button',{name:'下一步'}).click()
    await fixture.app.evaluate(({BrowserWindow})=>BrowserWindow.getAllWindows()[0]!.setSize(960,650))
    dialog = fixture.page.getByRole('dialog',{name:'配置 DeepSeek'})
    await expect(dialog.getByRole('button',{name:'添加并使用'})).toBeInViewport()
    await dialog.screenshot({path:join(evidence,'05-compact.png')})
    await dialog.getByLabel('API Key',{exact:true}).fill('DRAFT_ONLY')
    await fixture.page.keyboard.press('Escape')
    await expect(dialog).toContainText('有尚未保存的修改')
    await dialog.getByRole('button',{name:'放弃修改'}).click()
    await expect(dialog).toHaveCount(0)
  } finally { await fixture.close(); await new Promise<void>(r=>server.close(()=>r())) }
})

test('provider onboarding stays responsive while three terminal processes produce output', async () => {
  const f = await launchMatou({env:{MATOU_E2E_DISPLAY:'primary'}})
  try {
    const p = f.page
    await p.getByRole('button',{name:'横向新增 Shell',exact:true}).click()
    await p.getByRole('button',{name:'横向新增 Shell',exact:true}).click()
    await expect(p.locator('.terminal-surface[data-pid]')).toHaveCount(3)
    const before = await p.locator('.terminal-surface[data-pid]').evaluateAll(nodes=>nodes.map(n=>`${n.getAttribute('data-session-id')}:${n.getAttribute('data-pid')}`).sort())
    for (let i = 0; i < 3; i++) {
      const input = p.locator('.terminal-surface .xterm-helper-textarea').nth(i)
      await input.focus()
      await input.pressSequentially("for i in {1..200}; do echo 'Matou provider fixture'; sleep 0.03; done")
      await input.press('Enter')
    }
    await p.getByRole('button',{name:'设置',exact:true}).click()
    const started = Date.now()
    await p.getByRole('button',{name:'新增供应商'}).click()
    await expect(p.getByRole('searchbox')).toBeFocused()
    expect(Date.now()-started).toBeLessThan(1000)
    await p.getByRole('button',{name:/DeepSeek/}).click()
    await p.getByRole('button',{name:'下一步'}).click()
    const dialog = p.getByRole('dialog',{name:'配置 DeepSeek'})
    await expect(dialog.getByLabel('API Key',{exact:true})).toBeFocused()
    await p.keyboard.press('Escape')
    await expect(dialog).toHaveCount(0)
    await p.getByRole('button',{name:'关闭设置'}).click()
    expect(await p.locator('.terminal-surface[data-pid]').evaluateAll(nodes=>nodes.map(n=>`${n.getAttribute('data-session-id')}:${n.getAttribute('data-pid')}`).sort())).toEqual(before)
  } finally { await f.close() }
})
