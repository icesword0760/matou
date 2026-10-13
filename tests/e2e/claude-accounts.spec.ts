import { mkdir } from 'node:fs/promises'
import { resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou, restartMatou } from './matou-fixture'
const evidence = resolve(import.meta.dirname, '../../docs/acceptance/evidence/claude-accounts')
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
    await panel.getByRole('button', { name: '打开浏览器登录', exact: true }).click()
    await expect(panel).toContainText('在浏览器中完成登录')
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
    f = await restartMatou(f, { env: env('detected') })
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
    await p.locator('.ca-account').filter({ hasText: '个人账户' }).click()
    await expect(p.locator('.ca-panel')).toContainText('不是只切换当前卡片')
    await p.getByLabel('已了解范围，下次空闲时直接切换').check()
    await p.screenshot({ path: `${evidence}/05-scope-electron.png` })
    await p.getByRole('button', { name: '确认切换', exact: true }).click()
    await expect(p.locator('.ca-trigger')).toContainText('默认：个人账户')
    await expect(p.locator('.ca-panel')).toContainText('本机默认账户已更新')
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
    await expect(p.locator('.ca-panel')).toContainText('Device default updated')
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
    await p.getByRole('button', { name: '打开浏览器登录' }).click()
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
