import { chmod, mkdtemp, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou } from './matou-fixture'
import { activeSurface, terminalCommand, waitForShell } from './fixtures/session-canvas-fixture'

test('shows cached provider text on every navigation frame with a full scrollback', async () => {
  test.setTimeout(90_000)
  const root = await mkdtemp('/tmp/matou-e2e-warm-first-frame-')
  const provider = join(root, 'provider.sh')
  await writeFile(join(root, '.zshrc'), "alias cc='claude'\n")
  await writeFile(provider, `#!/bin/sh
awk 'BEGIN { for (i=1; i<=10000; i++) printf "\\033[36m缓存会话 %05d\\033[0m 第一帧保留已显示内容，再补齐后台增量。\\r\\n", i }'
printf 'WARM_PROVIDER_READY\\r\\n'
stty raw -echo
cat
`)
  await chmod(provider, 0o755)
  const fixture = await launchMatou({ root, env: {
    SHELL: '/bin/zsh', ZDOTDIR: root, MATOU_CLAUDE_COMMAND: provider
  } })
  try {
    const { page } = fixture
    const initialTask = await page.locator('[data-testid^="task-"]').first().getAttribute('data-testid')
    const surface = activeSurface(page)
    await waitForShell(surface)
    await terminalCommand(surface, 'cc')
    await expect(surface).toHaveAttribute('data-profile', 'claude-code')
    await expect(surface.locator('.xterm-rows')).toContainText('WARM_PROVIDER_READY')
    const id = await surface.getAttribute('data-session-id')
    const pid = await surface.getAttribute('data-pid')
    await page.getByRole('button', { name: '在 matou_workspace 中新增事项' }).click()
    await waitForShell(activeSurface(page))
    const otherTask = await page.locator('[data-testid^="task-"]').filter({ hasText: '新事项' }).getAttribute('data-testid')
    for (let cycle = 0; cycle < 3; cycle++) {
      await page.evaluate((sessionId) => {
        const probe = { frames: [] as Array<{ time: number; text: boolean }>, stop: false }
        ;(window as any).__warmFirstFrame = probe
        const sample = () => {
          const card = document.querySelector(`.session-card-slot[data-session-id="${sessionId}"]`)
          if (card && card.getBoundingClientRect().width > 0) {
            probe.frames.push({ time: performance.now(), text: !!card.querySelector('.xterm-rows')?.textContent?.trim() })
          }
          if (!probe.stop) requestAnimationFrame(sample)
        }
        requestAnimationFrame(sample)
      }, id)
      await page.getByTestId(initialTask!).click()
      const restored = activeSurface(page)
      await expect(restored).toHaveAttribute('data-session-id', id!)
      await expect(restored).toHaveAttribute('data-pid', pid!)
      await expect(restored.locator('.xterm-rows')).toContainText('WARM_PROVIDER_READY')
      // Include delayed replay/resize frames, not only the immediate remount.
      await page.waitForTimeout(600)
      const frames = await page.evaluate(() => {
        const probe = (window as any).__warmFirstFrame
        probe.stop = true
        return probe.frames as Array<{ time: number; text: boolean }>
      })
      expect(frames.length).toBeGreaterThan(5)
      expect(frames.filter((frame) => !frame.text), `cycle ${cycle}: no empty cached-card frames`).toEqual([])
      console.log(`warm provider cycle ${cycle}: ${frames.length} frames, zero empty frames`)
      await page.getByTestId(otherTask!).click()
      await waitForShell(activeSurface(page))
    }
  } finally { await fixture.close() }
})
