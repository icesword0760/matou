import { chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou } from './matou-fixture'

const evidence = resolve('docs/acceptance/evidence/global-egress-entry')

test('opens the global network check from Shell and preserves the official session label', async () => {
  test.setTimeout(90_000)
  // Keep macOS's control socket below its Unix-domain path length limit.
  const cache = resolve('.c5')
  await mkdir(cache, { recursive: true })
  const root = await mkdtemp(join(cache, 'g'))
  const fakeClaude = join(root, 'fake-claude.sh')
  await writeFile(fakeClaude, '#!/bin/sh\nwhile IFS= read -r line; do printf "%s\\n" "$line"; done\n')
  await chmod(fakeClaude, 0o755)
  await mkdir(evidence, { recursive: true })
  const fixture = await launchMatou({
    root,
    env: { MATOU_E2E_DISPLAY: 'primary', MATOU_CLAUDE_COMMAND: fakeClaude }
  })
  try {
    const page = fixture.page
    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1440, 900))
    const surface = page.locator('[data-testid="terminal-pane"] .terminal-surface').first()
    await expect.poll(async () => Number(await surface.getAttribute('data-pid'))).toBeGreaterThan(0)
    const shellInput = surface.locator('.xterm-helper-textarea')
    await shellInput.focus()
    await shellInput.pressSequentially("printf 'GLOBAL_%s\\n' 'EGRESS_SHELL_READY'")
    await shellInput.press('Enter')
    await expect(surface.locator('.xterm-rows')).toContainText('GLOBAL_EGRESS_SHELL_READY')
    const hud = page.locator('.shortcut-bar .status-info')
    await expect(hud).toHaveAttribute('data-hud-mode', 'shell')
    const entry = hud.getByRole('button', { name: '网络检查', exact: true })
    await expect(entry).toHaveText('出口正常')
    await expect(entry).toHaveAttribute('aria-expanded', 'false')
    await page.screenshot({ path: join(evidence, '01-shell-entry.png') })
    await entry.click()
    const panel = page.getByRole('dialog', { name: '网络检查' })
    await expect(panel).toBeVisible()
    const mantle = panel.getByRole('region', { name: 'Mantle' })
    await mantle.scrollIntoViewIfNeeded()
    await expect(mantle).toBeInViewport()
    await page.screenshot({ path: join(evidence, '02-shell-panel.png') })
    await entry.click()
    await expect(panel).toHaveCount(0)
    await entry.focus()
    await page.keyboard.press('Enter')
    await expect(panel).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(panel).toHaveCount(0)
    await expect(entry).toHaveAttribute('aria-expanded', 'false')
    await entry.click()
    await surface.locator('.xterm-screen').click()
    await expect(panel).toHaveCount(0)

    // Deterministic notice fixture, limited to this isolated Electron process.
    // The preceding screenshot uses the real desktop bridge and installed Mantle status.
    await fixture.app.evaluate(({ ipcMain }) => {
      ipcMain.removeHandler('matou:running-desktop-clients')
      ipcMain.handle('matou:running-desktop-clients', () => ['claude'])
    })
    await entry.click()
    await expect(panel.getByText('检测到 Claude Desktop 正在运行，它的出口不在保护范围')).toBeVisible()
    await mantle.scrollIntoViewIfNeeded()
    await expect(mantle).toBeInViewport()
    await expect(panel.getByText('检测到 Claude Desktop 正在运行，它的出口不在保护范围')).toBeInViewport()
    await page.screenshot({ path: join(evidence, '03-shell-client-notice-fixture.png') })
    await page.keyboard.press('Escape')

    const textarea = surface.locator('.xterm-helper-textarea')
    await textarea.focus()
    await textarea.pressSequentially('claude')
    await textarea.press('Enter')
    await expect(page.locator('.terminal-surface[data-profile="claude-code"]')).toHaveCount(1)
    await expect(hud).toHaveAttribute('data-hud-mode', 'agent')
    const official = hud.getByRole('button', { name: '🇺🇸 美国 · 出口正常', exact: true })
    await expect(official).toBeVisible()
    await expect(hud.locator('.egress-chip')).toHaveCount(1)
    await page.screenshot({ path: join(evidence, '04-official-entry.png') })
    await official.click()
    await expect(panel).toBeVisible()
    await expect(panel.getByRole('region', { name: 'Mantle' })).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(panel).toHaveCount(0)
  } finally {
    await fixture.close()
    await rm(root, { recursive: true, force: true })
  }
})
