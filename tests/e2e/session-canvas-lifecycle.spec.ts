import { expect, test } from '@playwright/test'

import { restartMatou, restartMatouGracefully } from './matou-fixture'
import {
  activeSurface, launchSessionCanvas, terminalCommand, visibleSurfaces
} from './fixtures/session-canvas-fixture'

test.describe('session canvas lifecycle', () => {
  test.setTimeout(60_000)

  test('keeps a detached Session as history and reopens a continuation after the native window closes', async () => {
    const fixture = await launchSessionCanvas()
    try {
      const original = activeSurface(fixture.page)
      const sessionId = await original.getAttribute('data-session-id')
      await original.click({ button: 'right', position: { x: 20, y: 60 } })
      const detachItem = fixture.page.getByRole('menuitem', { name: '↗ 独立窗口' })
      await detachItem.focus()
      await fixture.page.keyboard.press('Enter')
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      await expect(fixture.page.getByTestId('detached-placeholder')).toBeVisible()
      const detached = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      await expect(detached.locator(`.terminal-surface[data-session-id="${sessionId}"]`)).toBeVisible()
      await detached.close()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)
      await expect(fixture.page.getByTestId('detached-placeholder')).toHaveCount(0)
      await expect(fixture.page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)).toHaveCount(0)
      await expect(fixture.page.locator('.historical-session-card')).toContainText('Shell 已结束')

      await fixture.page.getByRole('button', { name: '重新打开 Shell' }).click()
      await expect(activeSurface(fixture.page)).toHaveAttribute('data-session-id', /.+/)
      await expect(activeSurface(fixture.page)).not.toHaveAttribute('data-session-id', sessionId!)
    } finally {
      await fixture.close()
    }
  })

  test('replays prior terminal output and preserves sibling membership after a full restart', async () => {
    let fixture = await launchSessionCanvas()
    try {
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      // Session creation is an authoritative Runtime command. Under full-suite
      // load the projection can arrive after the button click has resolved;
      // wait for the requested sibling before resolving the active Session.
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      await terminalCommand(activeSurface(fixture.page), 'printf "RESTART_JOURNAL_OK\\n"')
      await expect(activeSurface(fixture.page).locator('.xterm-rows')).toContainText('RESTART_JOURNAL_OK')
      fixture = await restartMatou(fixture)
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      await expect(visibleSurfaces(fixture.page).filter({ hasText: 'RESTART_JOURNAL_OK' })).toHaveCount(1)

      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      const dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      await expect(dag.locator('.dag-node-card').filter({ hasText: 'RESTART_JOURNAL_OK' })).toHaveCount(1)
    } finally {
      await fixture.close()
    }
  })

  test('keeps completed Shell output and idle state after a graceful app quit', async () => {
    let fixture = await launchSessionCanvas()
    try {
      const first = activeSurface(fixture.page)
      const firstId = await first.getAttribute('data-session-id')
      await terminalCommand(first, "printf 'GRACEFUL_FIRST\\n'")
      await expect.poll(() => occurrences(first, 'GRACEFUL_FIRST')).toBeGreaterThanOrEqual(2)

      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      const second = activeSurface(fixture.page)
      const secondId = await second.getAttribute('data-session-id')
      await terminalCommand(second, "printf 'GRACEFUL_SECOND\\n'")
      await expect.poll(() => occurrences(second, 'GRACEFUL_SECOND')).toBeGreaterThanOrEqual(2)

      fixture = { ...await restartMatouGracefully(fixture), nonGitDirectory: fixture.nonGitDirectory }
      const restored = visibleSurfaces(fixture.page)
      await expect(restored).toHaveCount(2)
      const restoredFirst = fixture.page.locator(`.terminal-surface[data-session-id="${firstId}"]`)
      const restoredSecond = fixture.page.locator(`.terminal-surface[data-session-id="${secondId}"]`)
      await expect(restoredFirst.locator('.xterm-rows')).toContainText('GRACEFUL_FIRST')
      await expect(restoredSecond.locator('.xterm-rows')).toContainText('GRACEFUL_SECOND')
      await expect(restoredFirst.locator('.xterm-rows')).not.toContainText('上次命令已中断')
      await expect(restoredSecond.locator('.xterm-rows')).not.toContainText('上次命令已中断')
    } finally {
      await fixture.close()
    }
  })
})

async function occurrences(surface: ReturnType<typeof activeSurface>, marker: string): Promise<number> {
  return ((await surface.locator('.xterm-rows').textContent()) ?? '').split(marker).length - 1
}
