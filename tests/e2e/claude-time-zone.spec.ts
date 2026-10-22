import { mkdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou, restartMatouGracefully } from './matou-fixture'

test('saves the Claude time zone from settings, persists across app restart and restores system default', async () => {
  test.setTimeout(60_000)
  const env = { MATOU_E2E_DISPLAY: 'primary' }
  let fixture = await launchMatou({ env })
  const evidence = resolve('docs/acceptance/evidence/claude-time-zone')
  try {
    await mkdir(evidence, { recursive: true })
    await fixture.page.getByRole('button', { name: '设置', exact: true }).click()
    await fixture.page.locator('.model-settings__nav').getByRole('button', { name: 'Claude Code', exact: true }).click()
    const select = fixture.page.getByRole('combobox', { name: '时区', exact: true })
    await expect(select).toHaveValue('system')
    await select.selectOption('America/Los_Angeles')
    await fixture.page.getByRole('button', { name: '保存', exact: true }).click()
    await expect(fixture.page.locator('.claude-time-zone-settings').getByRole('status')).toContainText('已保存，下次启动 Claude Code 时自动生效。')
    await fixture.page.locator('.model-settings').screenshot({ path: join(evidence, 'saved.png') })
    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]!.setSize(960, 650))
    await expect(fixture.page.getByRole('button', { name: '保存', exact: true })).toBeInViewport()
    await fixture.page.locator('.model-settings').screenshot({ path: join(evidence, 'compact.png') })
    fixture = await restartMatouGracefully(fixture, { env })
    await fixture.page.getByRole('button', { name: '设置', exact: true }).click()
    await fixture.page.locator('.model-settings__nav').getByRole('button', { name: 'Claude Code', exact: true }).click()
    await expect(fixture.page.getByRole('combobox', { name: '时区', exact: true })).toHaveValue('America/Los_Angeles')
    await fixture.page.getByRole('combobox', { name: '时区', exact: true }).selectOption('system')
    await fixture.page.getByRole('button', { name: '保存', exact: true }).click()
    await expect(fixture.page.locator('.claude-time-zone-settings').getByRole('status')).toContainText('已保存')
  } finally { await fixture.close() }
})
