import { mkdir } from 'node:fs/promises'
import { resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou, restartMatouGracefully } from './matou-fixture'
const evidence = process.env.MATOU_E2E_ACCOUNTS_EVIDENCE ?? resolve(import.meta.dirname, '../../docs/acceptance/evidence/claude-accounts')
const fixturePath = resolve(import.meta.dirname, 'fixtures/claude-accounts-engine.py')
const env = (scenario: string) => ({ MATOU_E2E_ACCOUNTS_BRIDGE: fixturePath, MATOU_E2E_ACCOUNTS_SCENARIO: scenario, MATOU_E2E_DISPLAY: 'primary' })
test.beforeAll(async () => { await mkdir(evidence, { recursive: true }) })

test('existing login onboarding, save, add, rename, remove, and preference persistence', async () => {
  let f = await launchMatou({ env: env('detected') })
  try {
    const p = f.page
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    const panel = p.getByRole('dialog', { name: 'Claude 账户', exact: true })
    await expect(panel).toContainText('发现本机已有 Claude 登录')
    await p.screenshot({ path: `${evidence}/01-detected-electron.png` })
    await panel.getByRole('button', { name: '保存当前账户', exact: true }).click()
    await panel.getByLabel('账户名称').fill('工作账户')
    await panel.getByRole('button', { name: '保存账户', exact: true }).click()
    await expect(panel).toContainText('账户已保存')
    await panel.getByRole('button', { name: '返回账户列表', exact: true }).last().click()
    await expect(panel.locator('.ca-account')).toHaveCount(1)
    await p.screenshot({ path: `${evidence}/02-single-electron.png` })
    await panel.getByRole('button', { name: '添加账户', exact: false }).click()
    await expect(panel).toContainText('不沿用原浏览器的登录')
    await p.screenshot({ path: `${evidence}/10-private-login-guidance-electron.png` })
    await panel.getByRole('button', { name: '打开独立隐私窗口', exact: true }).click()
    await expect(panel).toContainText('在独立窗口登录另一个账户')
    await expect(panel.getByLabel('账户名称')).toBeVisible()
    await panel.getByLabel('账户名称').fill('个人账户')
    await panel.getByRole('button', { name: '保存账户', exact: true }).click()
    await expect(panel).toContainText('账户已保存')
    await expect(p.locator('.ca-trigger')).toContainText('默认：工作账户')
    await panel.getByRole('button', { name: '返回账户列表', exact: true }).last().click()
    await panel.getByRole('button', { name: '管理账户', exact: true }).click()
    const manage = p.getByRole('dialog', { name: '管理账户', exact: true })
    await manage.locator('.ca-manage-row').filter({ hasText: '个人账户' }).getByRole('button', { name: '编辑' }).click()
    await p.getByLabel('账户名称').fill('备用账户')
    await p.getByRole('button', { name: '保存账户', exact: true }).click()
    await expect(manage).toContainText('备用账户')
    await p.screenshot({ path: `${evidence}/03-manage-electron.png` })
    f = await restartMatouGracefully(f, { env: env('detected') })
    await f.page.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await expect(f.page.locator('.ca-account')).toHaveCount(2)
    await expect(f.page.locator('.ca-panel')).toContainText('备用账户')
    await f.page.getByRole('button', { name: '管理账户', exact: true }).click()
    await f.page.locator('.ca-manage-row').filter({ hasText: '备用账户' }).getByRole('button', { name: '编辑' }).click()
    await f.page.getByRole('button', { name: '移除账户…', exact: true }).click()
    await expect(f.page.locator('.ca-panel')).toContainText('本机当前登录与项目文件保留')
    await f.page.getByRole('button', { name: '确认移除', exact: true }).click()
    await expect(f.page.locator('.ca-manage-row')).toHaveCount(1)
  } finally { await f.close() }
})

test('switch scope, real main/runtime coordination, keyboard, compact window and English', async () => {
  const f = await launchMatou({ env: { ...env('multiple'), MATOU_LOCALE: '' } })
  await f.page.evaluate(() => window.matouDesktop.setLocalePreference('zh-CN'))
  try {
    const p = f.page
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await expect(p.locator('.ca-account')).toHaveCount(2)
    await p.screenshot({ path: `${evidence}/04-multiple-electron.png` })
    await p.locator('.ca-account').filter({ hasText: '个人账户' }).getByRole('button', { name: '切换', exact: true }).click()
    await expect(p.locator('.ca-panel')).toContainText('不是只切换当前卡片')
    await p.getByLabel('已了解范围，下次空闲时直接切换').check()
    await p.screenshot({ path: `${evidence}/05-scope-electron.png` })
    await p.getByRole('button', { name: '确认切换', exact: true }).click()
    await expect(p.locator('.ca-trigger')).toContainText('默认：个人账户')
    await expect(p.locator('.ca-panel')).toContainText('已切换账户')
    await p.keyboard.press('Escape')
    await expect(p.locator('.ca-panel')).toHaveCount(0)
    await expect(p.locator('.ca-trigger')).toBeFocused()
    await p.keyboard.press('Enter')
    await expect(p.locator('.ca-panel')).toBeVisible()
    await f.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]!.setSize(820, 560))
    const box = await p.locator('.ca-panel').boundingBox()
    expect(box!.height).toBeLessThanOrEqual(491)
    await p.screenshot({ path: `${evidence}/06-compact-electron.png` })
    await p.evaluate(() => window.matouDesktop.setLocalePreference('en'))
    await expect(p.getByRole('dialog', { name: 'Claude accounts', exact: true })).toBeVisible()
    await expect(p.locator('.ca-panel')).toContainText('Account switched')
    await p.screenshot({ path: `${evidence}/07-english-electron.png` })
  } finally { await f.close() }
})

test('empty onboarding, login cancellation, close and return to in-progress flow', async () => {
  const f = await launchMatou({ env: env('empty') })
  try {
    const p = f.page
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await expect(p.getByRole('button', { name: '添加第一个账户' })).toBeEnabled()
    await p.screenshot({ path: `${evidence}/08-first-use-electron.png` })
    await p.getByRole('button', { name: '添加第一个账户' }).click()
    await p.getByRole('button', { name: '打开独立隐私窗口' }).click()
    await p.getByRole('button', { name: '关闭账户面板' }).click()
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await expect(p.getByLabel('账户名称')).toBeVisible()
    await p.getByRole('button', { name: '放弃本次添加' }).click()
    await expect(p.getByRole('button', { name: '添加第一个账户' })).toBeVisible()
  } finally { await f.close() }
})

test('account panel stays responsive with three live terminal cards producing output', async () => {
  const f = await launchMatou({ env: env('multiple') })
  try {
    const p = f.page
    await p.getByRole('button', { name: '横向新增 Shell', exact: true }).click()
    await p.getByRole('button', { name: '横向新增 Shell', exact: true }).click()
    await expect(p.locator('.terminal-surface[data-pid]')).toHaveCount(3)
    const inputs = p.locator('.terminal-surface .xterm-helper-textarea')
    for (let i = 0; i < 3; i++) {
      await inputs.nth(i).focus()
      await inputs.nth(i).pressSequentially("for i in {1..120}; do echo 'Matou account fixture output'; sleep 0.03; done")
      await inputs.nth(i).press('Enter')
    }
    const start = Date.now()
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await expect(p.locator('.ca-panel')).toBeVisible()
    expect(Date.now() - start).toBeLessThan(1000)
    await expect(p.locator('.ca-account')).toHaveCount(2)
    await p.screenshot({ path: `${evidence}/09-three-live-terminals-electron.png` })
    await p.keyboard.press('Escape')
    await expect(p.locator('.ca-trigger')).toBeFocused()
    await expect(p.locator('.terminal-surface[data-pid]')).toHaveCount(3)
  } finally { await f.close() }
})

for (const scenario of ['browser-missing', 'browser-closed']) {
  test(`private sign-in ${scenario} keeps the flow retryable`, async () => {
    const f = await launchMatou({ env: env(scenario) })
    try {
      const p = f.page
      await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
      await p.getByRole('button', { name: '添加第一个账户' }).click()
      await p.getByRole('button', { name: '打开独立隐私窗口' }).click()
      await expect(p.getByRole('alert')).toContainText(scenario === 'browser-missing' ? 'Chrome、Edge 或 Brave' : '登录窗口已关闭')
      await expect(p.getByRole('button', { name: '打开独立隐私窗口' })).toBeEnabled()
      await expect(p.getByLabel('账户名称')).toHaveCount(0)
    } finally { await f.close() }
  })
}

test('duplicate sign-in offers a fresh login instead of silently adding the old account', async () => {
  const f = await launchMatou({ env: env('multiple') })
  try {
    const p = f.page
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await p.getByRole('button', { name: '添加账户', exact: false }).click()
    await p.getByRole('button', { name: '打开独立隐私窗口' }).click()
    await expect(p.locator('.ca-panel')).toContainText('这个账户已经添加过')
    await p.getByRole('button', { name: '登录其他账户', exact: true }).click()
    await expect(p.locator('.ca-panel')).toContainText('这个账户已经添加过')
    await expect(p.getByRole('button', { name: '保存账户', exact: true })).toBeEnabled()
  } finally { await f.close() }
})

for (const scenario of ['manual', 'manual-failed']) {
  test(`authorization-code ${scenario}: paste, validate, submit and recover`, async () => {
    const f = await launchMatou({ env: env(scenario) })
    try {
      const p = f.page
      await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
      await p.getByRole('button', { name: '添加第一个账户' }).click()
      await p.getByRole('button', { name: '打开独立隐私窗口' }).click()
      await expect(p.getByLabel('授权码', { exact: true })).toBeEnabled()
      await expect(p.locator('.ca-panel')).toContainText('隐私窗口由你自行关闭')
      await p.getByLabel('授权码', { exact: true }).fill('https://example.com')
      await p.getByRole('button', { name: '完成登录', exact: true }).click()
      await expect(p.getByRole('alert')).toContainText('完整授权码')
      await p.getByLabel('授权码', { exact: true }).fill('fixture_code_123456#fixture_state_123456')
      await p.getByRole('button', { name: '完成登录', exact: true }).click()
      if (scenario === 'manual') {
        await expect(p.getByLabel('账户名称')).toBeVisible()
        await p.getByLabel('账户名称').fill('新账户')
        await p.getByRole('button', { name: '保存账户', exact: true }).click()
        await expect(p.locator('.ca-panel')).toContainText('账户已保存')
        await expect(p.locator('.ca-trigger')).not.toContainText('默认：新账户')
      } else {
        await expect(p.getByRole('alert')).toContainText('登录尚未完成')
        await expect(p.getByRole('button', { name: '打开独立隐私窗口' })).toBeEnabled()
        await expect(p.getByLabel('授权码', { exact: true })).toHaveCount(0)
      }
    } finally { await f.close() }
  })
}

test('manual login can close panel, return, cancel and retry with a fresh attempt', async () => {
  const f = await launchMatou({ env: env('manual') })
  try {
    const p = f.page
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await p.getByRole('button', { name: '添加第一个账户' }).click()
    await p.getByRole('button', { name: '打开独立隐私窗口' }).click()
    await expect(p.getByLabel('授权码', { exact: true })).toBeEnabled()
    const first = (await p.evaluate(() => window.matouDesktop.getClaudeAccounts())).loginAttemptId!
    await p.getByLabel('授权码', { exact: true }).fill('unsent_sensitive_draft')
    await p.getByRole('button', { name: '关闭账户面板' }).click()
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await expect(p.getByLabel('授权码', { exact: true })).toHaveValue('')
    await f.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]!.setSize(820, 560))
    await expect(p.getByRole('button', { name: '完成登录', exact: true })).toBeInViewport()
    await expect(p.getByRole('button', { name: '取消本次登录', exact: true })).toBeInViewport()
    await p.screenshot({ path: `${evidence}/11-manual-code-compact-electron.png`, animations: 'disabled' })
    await p.getByRole('button', { name: '取消本次登录' }).click()
    await expect(p.getByRole('button', { name: '打开独立隐私窗口' })).toBeEnabled()
    await p.getByRole('button', { name: '打开独立隐私窗口' }).click()
    await expect(p.getByLabel('授权码', { exact: true })).toBeEnabled()
    const second = (await p.evaluate(() => window.matouDesktop.getClaudeAccounts())).loginAttemptId!
    expect(second).not.toBe(first)
    await p.evaluate(id => window.matouDesktop.claudeAccountCommand({ type: 'submit-login-code', attemptId: id, code: 'fixture_code_123456#fixture_state_123456' }), first)
    await expect(p.getByRole('alert')).toContainText('这次登录已结束')
    await p.getByLabel('授权码', { exact: true }).fill('fixture_code_123456#fixture_state_123456')
    await p.getByLabel('授权码', { exact: true }).press('Enter')
    await expect(p.getByLabel('账户名称')).toBeVisible()
  } finally { await f.close() }
})

for (const scenario of ['reauth-auto', 'reauth-manual']) {
  test(`${scenario}: renew original account, persist pending application, apply explicitly without restarting terminals`, async () => {
    let f = await launchMatou({ env: env(scenario) })
    try {
      let p = f.page
      await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
      await p.getByRole('button', { name: '重新登录', exact: true }).click()
      await expect(p.locator('.ca-panel')).toContainText('重新登录「工作账户」')
      await expect(p.locator('.ca-panel')).toContainText('现有对话保持运行')
      await p.screenshot({ path: `${evidence}/12-reauth-intro-electron.jpg`, animations: 'disabled' })
      const pidBefore = await p.locator('.terminal-surface').first().getAttribute('data-pid')
      await p.getByRole('button', { name: '打开隐私窗口重新登录' }).click()
      if (scenario === 'reauth-manual') {
        await expect(p.locator('.ca-panel')).toContainText('请使用 wo•••@example.com 完成授权')
        await p.getByLabel('授权码', { exact: true }).fill('fixture_code_123456#fixture_state_123456')
        await p.getByLabel('授权码', { exact: true }).press('Enter')
      }
      await expect(p.getByLabel('账户名称')).toHaveValue('工作账户')
      await expect(p.getByLabel('账户名称')).toHaveAttribute('readonly', '')
      await p.getByRole('button', { name: '保存新登录', exact: true }).click()
      await expect(p.getByRole('button', { name: '应用到本机默认登录', exact: true })).toBeVisible()
      await expect(p.locator('.terminal-surface').first()).toHaveAttribute('data-pid', pidBefore!)
      let state = await p.evaluate(() => window.matouDesktop.getClaudeAccounts())
      expect(state).toMatchObject({ currentId: 'fixture-a', renewedIds: ['fixture-a'], pendingSessionIds: [] })
      expect(state.accounts).toHaveLength(2)
      f = await restartMatouGracefully(f, { env: env(scenario) }); p = f.page
      await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
      await expect(p.locator('.ca-panel')).toContainText('还需应用到本机默认登录')
      await p.getByRole('button', { name: '继续处理', exact: true }).click()
      await f.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]!.setSize(820, 560))
      await expect(p.getByRole('button', { name: '应用到本机默认登录', exact: true })).toBeInViewport()
      await p.screenshot({ path: `${evidence}/13-reauth-apply-electron.png`, animations: 'disabled' })
      const pidAfterRestart = await p.locator('.terminal-surface').first().getAttribute('data-pid')
      await p.getByRole('button', { name: '应用到本机默认登录', exact: true }).click()
      await expect(p.locator('.ca-panel')).toContainText('登录已更新')
      await expect(p.locator('.terminal-surface').first()).toHaveAttribute('data-pid', pidAfterRestart!)
      state = await p.evaluate(() => window.matouDesktop.getClaudeAccounts())
      expect(state.renewedIds).toEqual([])
      expect(state.currentId).toBe('fixture-a')
    } finally { await f.close() }
  })
}

test('reauth wrong identity keeps original account and offers a targeted retry', async () => {
  const f = await launchMatou({ env: env('reauth-wrong') })
  try {
    const p = f.page
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await p.locator('.ca-account').filter({ hasText: '工作账户' }).getByRole('button', { name: '重新登录', exact: true }).click()
    await p.getByRole('button', { name: '打开隐私窗口重新登录' }).click()
    await expect(p.getByRole('alert')).toContainText('登录的账户与选择的账户不一致')
    await expect(p.locator('.ca-panel')).toContainText('重新登录「工作账户」')
    await expect(p.getByRole('button', { name: '重新打开隐私窗口' })).toBeEnabled()
    const state = await p.evaluate(() => window.matouDesktop.getClaudeAccounts())
    expect(state.currentId).toBe('fixture-a'); expect(state.renewedIds).toEqual([])
    expect(state.draft).toBeUndefined()
  } finally { await f.close() }
})

test('renewing a non-default account preserves default and guides an explicit later switch', async () => {
  const f = await launchMatou({ env: env('reauth-auto') })
  try {
    const p = f.page
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await p.getByRole('button', { name: '管理账户', exact: true }).click()
    await p.locator('.ca-manage-row').filter({ hasText: '个人账户' }).getByRole('button', { name: '编辑' }).click()
    await p.getByRole('button', { name: '重新登录', exact: true }).click()
    await expect(p.locator('.ca-panel')).toContainText('重新登录「个人账户」')
    await p.getByRole('button', { name: '打开隐私窗口重新登录' }).click()
    await p.getByRole('button', { name: '保存新登录', exact: true }).click()
    await expect(p.locator('.ca-panel')).toContainText('当前默认账户保持不变')
    await expect(p.getByRole('button', { name: '应用到本机默认登录', exact: true })).toHaveCount(0)
    await expect(p.getByRole('button', { name: '切换到这个账户…', exact: true })).toBeVisible()
    const state = await p.evaluate(() => window.matouDesktop.getClaudeAccounts())
    expect(state.currentId).toBe('fixture-a'); expect(state.renewedIds).toEqual(['fixture-b'])
    expect(state.accounts).toHaveLength(2)
  } finally { await f.close() }
})
