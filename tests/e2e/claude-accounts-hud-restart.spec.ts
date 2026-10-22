import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou } from './matou-fixture'
const evidence=process.env.MATOU_E2E_ACCOUNTS_EVIDENCE ?? resolve('docs/acceptance/evidence/claude-accounts-hud-restart')
async function setup(scenario = 'usage-normal') {
  const root=await mkdtemp('/tmp/matou-accounts-hud-')
  const f=await launchMatou({root,env:{MATOU_LOCALE:'',MATOU_E2E_DISPLAY:'primary',MATOU_E2E_ACCOUNTS_BRIDGE:resolve('tests/e2e/fixtures/claude-accounts-engine.py'),MATOU_E2E_ACCOUNTS_SCENARIO:scenario,MATOU_CLAUDE_COMMAND:resolve('tests/e2e/fixtures/account-restart-provider.py'),MATOU_ACCOUNT_RESTART_FIXTURE:root,MATOU_CLAUDE_PROJECTS_ROOT:join(root,'projects'),CLAUDE_CONFIG_DIR:'',CLAUDE_SECURESTORAGE_CONFIG_DIR:'',ANTHROPIC_API_KEY:'',ANTHROPIC_AUTH_TOKEN:'',CLAUDE_CODE_OAUTH_TOKEN:''}})
  await f.page.evaluate(()=>window.matouDesktop.setLocalePreference('zh-CN'))
  await f.app.evaluate(({BrowserWindow})=>BrowserWindow.getAllWindows()[0]!.setSize(1400,850))
  await mkdir(evidence,{recursive:true})
  return f
}
async function startClaude(f: Awaited<ReturnType<typeof setup>>) {
  const input=f.page.locator('.terminal-surface .xterm-helper-textarea').last()
  await input.focus();await input.pressSequentially('claude');await input.press('Enter')
  await expect(f.page.locator('.status-info')).toHaveAttribute('data-hud-mode','agent')
  await expect(f.page.locator('.ca-trigger-label')).toHaveText('工作账户')
  await expect(f.page.locator('.status-info__primary')).not.toContainText('默认账户')
  await expect(f.page.locator('.status-usage')).toContainText(['5h 38%','Weekly 62%'])
}
async function switchAccount(f:Awaited<ReturnType<typeof setup>>) {
  const p=f.page;await p.getByRole('button',{name:'Claude 账户',exact:true}).click()
  await p.locator('.ca-account').filter({hasText:'个人账户'}).getByRole('button',{name:'切换',exact:true}).click()
  await expect(p.getByRole('button',{name:'立即切换',exact:true})).toBeEnabled()
  await expect(p.locator('.ca-panel')).not.toContainText('任务结束后')
  await p.getByRole('button',{name:'立即切换',exact:true}).click()
  await expect(p.locator('.ca-switch-feedback')).toContainText('已切换账户，现有会话通常会在约 30 秒内使用新账户，或者选择立即重启并接续。')
}
test('cached quota has no separate last-known field in the desktop HUD', async () => {
  const f = await setup()
  try {
    await startClaude(f)
    const primary = f.page.locator('.status-info__primary')
    await expect(primary).not.toContainText(/上次数据|待更新|待输入|使用后更新用量/)
    await expect(f.page.locator('.status-usage').first()).toContainText(/5h 38% · (?:\d{4}\/)?10\/14 \d{2}:\d{2}/)
    await expect(f.page.locator('.status-usage').first()).toHaveAttribute('title', /上次记录|重置时间/)
    await f.page.screenshot({ path: join(evidence, 'hud-no-cache-field.png') })
    await f.page.evaluate(() => window.matouDesktop.setLocalePreference('en'))
    await expect(primary).not.toContainText(/Last known|Pending|Usage updates after use/)
    await expect(f.page.locator('.status-usage').first()).toHaveAttribute('title', /Reset time reached/)
    await expect(f.page.locator('.status-usage')).toContainText(['5h 38%', 'Weekly 62%'])
  } finally { await f.close() }
})
test('switch synchronizes HUD without restarting, cancel is inert, confirm restarts all Claude not shells',async()=>{
  test.setTimeout(120_000)
  const f=await setup();const p=f.page
  try {
    await startClaude(f)
    await p.getByRole('button',{name:'横向新增 Shell',exact:true}).click();await expect(p.locator('.terminal-surface .xterm-helper-textarea')).toHaveCount(2);await startClaude(f)
    await p.getByRole('button',{name:'横向新增 Shell',exact:true}).click()
    await expect(p.locator('.terminal-surface[data-pid]')).toHaveCount(3)
    const before=await p.locator('.terminal-surface').evaluateAll(nodes=>nodes.map(n=>({id:n.getAttribute('data-session-id'),pid:n.getAttribute('data-pid')})))
    // Selecting an existing card needs a pointer event: its textarea can retain
    // DOM focus while a newly created Shell is the active canvas session.
    await p.locator('.terminal-surface .xterm-screen').nth(0).click()
    await expect(p.locator('.status-info')).toHaveAttribute('data-session-id', before[0]!.id!)
    await expect(p.locator('.ca-trigger-label')).toHaveText('工作账户')
    // Both tasks are already working BEFORE the switch, not only before restart.
    await writeFile(join(f.rootDirectory,'busy'),'yes')
    await expect.poll(async()=>{const s=await p.evaluate(()=>window.matouDesktop.claudeAccountCommand({type:'refresh-sessions'}));return s.sessions.filter(x=>x.busy).length}).toBe(2)
    await switchAccount(f)
    await expect(p.locator('.ca-trigger-label')).toHaveText('个人账户')
    await expect(p.locator('.status-usage')).toContainText(['5h 12%','Weekly 24%'])
    await expect(p.locator('.status-info')).toContainText('72%')
    await p.screenshot({path:join(evidence,'switched-hud.png')})
    // Stop reading quotas in the panel, but keep a HUD consumer alive.
    await p.getByRole('button',{name:'关闭账户面板'}).click()
    await expect(p.locator('.status-usage')).toContainText(['5h 12%','Weekly 24%'])
    await p.getByRole('button',{name:'Claude 账户',exact:true}).click()
    await p.getByRole('button',{name:'立即重启',exact:true}).click()
    const confirm=p.getByRole('alertdialog',{name:'确定重启所有正在运行的 Claude 会话窗口吗？'})
    await expect(confirm).toContainText('这会造成进行中任务的中断。')
    await p.screenshot({path:join(evidence,'restart-confirm.png')})
    await confirm.getByRole('button',{name:'取消',exact:true}).click();await expect(confirm).toHaveCount(0)
    await expect(p.locator('.ca-panel')).toBeVisible()
    await expect(p.getByRole('button',{name:'立即重启',exact:true})).toBeFocused()
    expect(await p.locator('.terminal-surface').evaluateAll(nodes=>nodes.map(n=>({id:n.getAttribute('data-session-id'),pid:n.getAttribute('data-pid')})))).toEqual(before)
    expect((await readFile(join(f.rootDirectory,'runs.jsonl'),'utf8')).trim().split('\n')).toHaveLength(2)
    // Explicit warning allows restart even while both Claude tasks are busy.
    await writeFile(join(f.rootDirectory,'busy'),'yes')
    await expect.poll(async()=>{const s=await p.evaluate(()=>window.matouDesktop.claudeAccountCommand({type:'refresh-sessions'}));return s.sessions.filter(x=>x.busy).length}).toBe(2)
    await p.getByRole('button',{name:'立即重启',exact:true}).click();await confirm.getByRole('button',{name:'确定',exact:true}).click()
    await expect(p.locator('.ca-restart-result')).toContainText('已重启并接续 2 个正在运行的 Claude 会话。',{timeout:60_000})
    const after=await p.locator('.terminal-surface').evaluateAll(nodes=>nodes.map(n=>({id:n.getAttribute('data-session-id'),pid:n.getAttribute('data-pid')})))
    expect(after.map(x=>x.id)).toEqual(before.map(x=>x.id));expect(after[2]).toEqual(before[2]);expect(after[0]!.pid).not.toEqual(before[0]!.pid);expect(after[1]!.pid).not.toEqual(before[1]!.pid)
    const runs=(await readFile(join(f.rootDirectory,'runs.jsonl'),'utf8')).trim().split('\n').map(s=>JSON.parse(s))
    expect(runs).toHaveLength(4);for(const r of runs.slice(2))expect(r.args).toContain('--resume')
    await p.screenshot({path:join(evidence,'restart-completed.png')})
  }finally{await f.close()}
})
test('failed Claude startup reports failure rather than global success; account stays switched',async()=>{
  test.setTimeout(90_000);const f=await setup();const p=f.page
  try {
    await startClaude(f);await switchAccount(f);await writeFile(join(f.rootDirectory,'fail-restart'),'yes')
    await p.getByRole('button',{name:'立即重启',exact:true}).click()
    await p.getByRole('alertdialog').getByRole('button',{name:'确定',exact:true}).click()
    await expect(p.locator('.ca-restart-result')).toContainText('1 个未完成',{timeout:60_000})
    await expect(p.locator('.ca-trigger')).toContainText('个人账户')
    await p.screenshot({path:join(evidence,'restart-failed.png')})
  }finally{await f.close()}
})

test('a detached hidden Claude session restarts once along with the main window session',async()=>{
  test.setTimeout(120_000);const f=await setup();const p=f.page
  try {
    await startClaude(f)
    await p.getByRole('button',{name:'横向新增 Shell',exact:true}).click();await expect(p.locator('.terminal-surface .xterm-helper-textarea')).toHaveCount(2);await startClaude(f)
    await expect.poll(async()=>(await p.evaluate(()=>window.matouDesktop.claudeAccountCommand({type:'refresh-sessions'}))).sessions.filter(s=>s.runId).length).toBe(2)
    const detachedId=await p.locator('.terminal-surface').first().getAttribute('data-session-id')
    await p.locator('.terminal-pane-header').first().dispatchEvent('dragend',{screenX:-1,screenY:-1})
    await expect.poll(async()=>(await f.app.windows()).length).toBe(2)
    const detached=(await f.app.windows()).find(w=>w!==p)!
    await expect(detached.locator('.terminal-surface')).toHaveAttribute('data-pid',/\d+/)
    const oldPid=await detached.locator('.terminal-surface').getAttribute('data-pid')
    await expect.poll(async()=>(await p.evaluate(()=>window.matouDesktop.claudeAccountCommand({type:'refresh-sessions'}))).sessions.filter(s=>s.runId).length).toBe(2)
    const window=await f.app.browserWindow(detached);await window.evaluate(w=>w.hide())
    const main=await f.app.browserWindow(p);await main.evaluate(w=>{w.show();w.focus()})
    await p.locator('.terminal-surface .xterm-helper-textarea').last().focus()
    await writeFile(join(f.rootDirectory,'busy'),'yes')
    await expect.poll(async()=>(await p.evaluate(()=>window.matouDesktop.claudeAccountCommand({type:'refresh-sessions'}))).sessions.filter(s=>s.busy).length).toBe(2)
    await switchAccount(f)
    await expect(detached.locator('.terminal-surface')).toHaveAttribute('data-pid',oldPid!)
    await p.getByRole('button',{name:'立即重启',exact:true}).click();await p.getByRole('alertdialog').getByRole('button',{name:'确定',exact:true}).click()
    await expect(p.locator('.ca-restart-result')).toContainText('已重启并接续 2 个正在运行的 Claude 会话。',{timeout:60_000})
    await expect(detached.locator('.terminal-surface')).toHaveAttribute('data-session-id',detachedId!)
    await expect(detached.locator('.terminal-surface')).not.toHaveAttribute('data-pid',oldPid!)
    const runs=(await readFile(join(f.rootDirectory,'runs.jsonl'),'utf8')).trim().split('\n')
    expect(runs).toHaveLength(4)
  }finally{await f.close()}
})


test('busy switching supports cancel, compact English confirmation, and remembered direct switching', async () => {
  const f=await setup();const p=f.page
  try {
    await startClaude(f)
    await writeFile(join(f.rootDirectory,'busy'),'yes')
    await expect.poll(async()=>(await p.evaluate(()=>window.matouDesktop.claudeAccountCommand({type:'refresh-sessions'}))).sessions.some(s=>s.busy)).toBe(true)
    const pid=await p.locator('.terminal-surface').getAttribute('data-pid')
    await p.getByRole('button',{name:'Claude 账户',exact:true}).click()
    await p.locator('.ca-account').filter({hasText:'个人账户'}).getByRole('button',{name:'切换',exact:true}).click()
    await p.getByRole('button',{name:'暂不切换',exact:true}).click()
    expect((await p.evaluate(()=>window.matouDesktop.getClaudeAccounts())).currentId).toBe('fixture-a')
    await p.locator('.ca-account').filter({hasText:'个人账户'}).getByRole('button',{name:'切换',exact:true}).click()
    await p.getByLabel('已了解范围，下次直接切换').check()
    await p.evaluate(()=>window.matouDesktop.setLocalePreference('en'))
    await f.app.evaluate(({BrowserWindow})=>BrowserWindow.getAllWindows()[0]!.setSize(820,560))
    await expect(p.getByRole('button',{name:'Switch now',exact:true})).toBeInViewport()
    await expect(p.locator('.ca-panel')).not.toContainText('when idle')
    await p.screenshot({path:join(evidence,'immediate-switch-compact-en.jpg')})
    await p.getByRole('button',{name:'Switch now',exact:true}).click()
    await expect(p.locator('.ca-switch-feedback')).toContainText('Account switched')
    await expect(p.getByRole('button',{name:'Restart now',exact:true})).toBeVisible()
    await p.locator('.ca-account').filter({hasText:'工作账户'}).getByRole('button',{name:'Switch',exact:true}).click()
    await expect(p.locator('.ca-trigger-label')).toHaveText('工作账户')
    await expect(p.getByRole('button',{name:'Switch now',exact:true})).toHaveCount(0)
    await expect(p.locator('.terminal-surface')).toHaveAttribute('data-pid',pid!)
    expect((await readFile(join(f.rootDirectory,'runs.jsonl'),'utf8')).trim().split('\n')).toHaveLength(1)
  } finally { await f.close() }
})

for (const scenario of ['empty', 'usage-expired']) {
  test(`${scenario}: HUD uses a dash while account panel retains guidance`, async () => {
    const f = await setup(scenario)
    try {
      const input = f.page.locator('.terminal-surface .xterm-helper-textarea').last()
      await input.focus(); await input.pressSequentially('claude'); await input.press('Enter')
      await expect(f.page.locator('.status-info')).toHaveAttribute('data-hud-mode', 'agent')
      await expect(f.page.locator('.status-usage')).toHaveText('—')
      await expect(f.page.locator('.status-usage')).toHaveAttribute('title', scenario === 'empty' ? /添加账户后查看用量/ : /登录已失效/)
      await f.page.getByRole('button', {name:'Claude 账户', exact:true}).click()
      await expect(f.page.locator('.ca-panel')).toContainText(scenario === 'empty' ? '添加第一个账户' : '重新登录')
      await f.page.screenshot({path:join(evidence, `hud-minimal-${scenario}.png`), animations:'disabled'})
    } finally { await f.close() }
  })
}
