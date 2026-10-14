import { mkdir, readFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou } from './matou-fixture'
const evidence = resolve(process.env.MATOU_E2E_ACCOUNTS_EVIDENCE ?? 'docs/acceptance/evidence/claude-accounts-local')
const fixturePath = resolve('tests/e2e/fixtures/claude-accounts-engine.py')
const env = (scenario: string) => ({ MATOU_E2E_ACCOUNTS_BRIDGE: fixturePath, MATOU_E2E_ACCOUNTS_SCENARIO: scenario, MATOU_E2E_DISPLAY: 'primary' })
test.beforeAll(async () => { await mkdir(evidence, { recursive: true }) })
for (const scenario of ['usage-single', 'usage-normal', 'usage-expired']) {
  test(`${scenario}: cached quota and unchanged controls, no account-engine HTTP refresh`, async () => {
    const f = await launchMatou({ env: env(scenario) })
    try {
      const p = f.page
      await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
      const panel = p.getByRole('dialog', { name: 'Claude 账户', exact: true })
      await expect(panel.locator('.ca-account')).toHaveCount(scenario === 'usage-normal' ? 2 : 1)
      await expect(panel.getByRole('button', { name: '刷新用量' })).toBeEnabled()
      await panel.getByRole('button', { name: '刷新用量' }).click()
      await expect(panel.getByRole('button', { name: '刷新用量' })).toBeEnabled()
      const log = await readFile(join(f.electronUserDataDirectory, 'claude-accounts/actions-fixture.jsonl'), 'utf8')
      expect(log).not.toContain('"refresh"')
      await expect(panel.locator('.ca-footer')).toHaveCount(0)
      await expect(panel.getByRole('button', { name: '重新登录', exact: true })).toHaveCount(scenario === 'usage-expired' ? 1 : 0)
      if (scenario !== 'usage-expired') {
        await expect(panel).toContainText('38%')
        await expect(panel).toContainText('已到重置时间，待更新')
      }
      await panel.screenshot({ path: join(evidence, `${scenario}.png`) })
      if (scenario === 'usage-normal') {
        await expect(panel.locator('.ca-account').nth(1)).toContainText('上次数据')
        await f.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]!.setSize(820, 560))
        await expect(panel.getByRole('button', { name: '管理账户' })).toBeInViewport()
        await panel.screenshot({ path: join(evidence, 'compact-window.png') })
        await panel.getByRole('button', { name: '切换', exact: true }).click()
        await expect(p.locator('.ca-panel')).toContainText('不是只切换当前卡片')
        await expect(p.getByRole('button', { name: '立即切换', exact: true })).toBeVisible()
      }
    } finally { await f.close() }
  })
}
test('offline cache remains readable, navigation and manual local refresh stay responsive', async () => {
  const f = await launchMatou({ env: env('usage-network') })
  try {
    const p = f.page
    await p.getByRole('button', { name: 'Claude 账户', exact: true }).click()
    await expect(p.locator('.ca-panel')).toContainText('38%')
    await expect(p.locator('.ca-panel')).not.toContainText('用量更新失败')
    await p.getByRole('button', { name: '刷新用量' }).click()
    const start = Date.now()
    await p.getByRole('button', { name: '管理账户', exact: true }).click()
    await expect(p.getByRole('dialog', { name: '管理账户' })).toBeVisible()
    expect(Date.now() - start).toBeLessThan(1000)
  } finally { await f.close() }
})
