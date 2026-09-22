import { access, chmod, mkdtemp, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { expect, test } from '@playwright/test'
import { launchMatou, restartMatou, stopMatouPreservingData } from './matou-fixture'
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


test('uses the current provider screen when returning to a cached but empty card', async () => {
  test.setTimeout(90_000)
  const root = await mkdtemp('/tmp/matou-e2e-empty-cache-')
  const provider = join(root, 'provider.sh')
  const release = join(root, 'release')
  const done = join(root, 'done')
  await writeFile(provider, `#!/bin/sh
while [ ! -f '${release}' ]; do sleep 0.05; done
awk 'BEGIN { for (i=1; i<=20000; i++) printf "\\033[2J\\033[HOLD_PROGRESS_%05d runtime history should not be replayed from the start", i }'
printf '\\033[2J\\033[HCURRENT_PROVIDER_SCREEN'
touch '${done}'
stty raw -echo
cat
`)
  await chmod(provider, 0o755)
  const env = { MATOU_CLAUDE_COMMAND: provider }
  let fixture = await launchMatou({ root, env })
  try {
    const id = await activeSurface(fixture.page).getAttribute('data-session-id')
    await stopMatouPreservingData(fixture)
    const db = new DatabaseSync(join(fixture.dataDirectory, 'matou.sqlite'))
    try { db.prepare("UPDATE sessions SET kind = 'claude-code' WHERE id = ?").run(id!) }
    finally { db.close() }
    fixture = await restartMatou(fixture, { env })
    const { page } = fixture
    const task = await page.locator('[data-testid^="task-"]').first().getAttribute('data-testid')
    const original = activeSurface(page)
    await waitForShell(original)
    await expect(original).toHaveAttribute('data-profile', 'claude-code')
    const pid = await original.getAttribute('data-pid')
    await page.getByRole('button', { name: '在 matou_workspace 中新增事项' }).click()
    await waitForShell(activeSurface(page))
    await writeFile(release, 'go')
    await expect.poll(() => access(done).then(() => true, () => false)).toBe(true)
    // Let Runtime consume the synthetic redraw burst while this card is detached.
    await page.waitForTimeout(1000)
    await page.evaluate(() => {
      const probe = { obsoleteFrames: 0 }
      ;(window as any).__emptyCacheProbe = probe
      new MutationObserver(() => {
        if ([...document.querySelectorAll('.xterm-rows')].some((rows) => rows.textContent?.includes('OLD_PROGRESS_'))) {
          probe.obsoleteFrames++
        }
      }).observe(document.body, { subtree: true, childList: true, characterData: true })
    })
    const start = Date.now()
    await page.getByTestId(task!).click()
    const restored = activeSurface(page)
    await expect(restored).toHaveAttribute('data-pid', pid!)
    await expect(restored.locator('.xterm-rows')).toContainText('CURRENT_PROVIDER_SCREEN', { timeout: 1500 })
    expect(Date.now() - start).toBeLessThan(1500)
    await expect(restored.locator('.xterm-rows')).not.toContainText('OLD_PROGRESS_')
    expect(await page.evaluate(() => (window as any).__emptyCacheProbe.obsoleteFrames)).toBe(0)
    console.log(`empty provider cache restored in ${Date.now() - start} ms`)
  } finally { await fixture.close() }
})
