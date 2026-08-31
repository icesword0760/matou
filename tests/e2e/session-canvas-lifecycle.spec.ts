import { DatabaseSync } from 'node:sqlite'
import { join } from 'node:path'

import { expect, test } from '@playwright/test'

import { restartMatou, restartMatouGracefully } from './matou-fixture'
import {
  activeSurface, launchSessionCanvas, terminalCommand, visibleSurfaces
} from './fixtures/session-canvas-fixture'

test.describe('session canvas lifecycle', () => {
  test.setTimeout(60_000)

  test('returns a detached Session to the canvas as a live terminal when its window closes', async () => {
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
      const returned = fixture.page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
      await expect(returned).toHaveAttribute('data-pid', /\d+/)
      await expect(returned.locator('.xterm-helper-textarea')).toBeAttached()
      await expect(fixture.page.locator('.stopped-session-card')).toHaveCount(0)
    } finally {
      await fixture.close()
    }
  })

  test('removes one Session from the horizontal list and DAG without moving focus to a missing node', async () => {
    const fixture = await launchSessionCanvas()
    try {
      const parent = activeSurface(fixture.page)
      const parentId = await parent.getAttribute('data-session-id')
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      await expect(activeSurface(fixture.page)).not.toHaveAttribute('data-session-id', parentId!)
      const removable = activeSurface(fixture.page)
      const removableId = await removable.getAttribute('data-session-id')
      const pane = removable.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')

      await pane.getByRole('button', { name: /移出节点/ }).click()
      const dialog = fixture.page.getByRole('alertdialog')
      const centers = await Promise.all([
        dialog.boundingBox(),
        fixture.page.getByRole('region', { name: '会话画布' }).boundingBox()
      ])
      expect(centers[0]).not.toBeNull()
      expect(centers[1]).not.toBeNull()
      expect(Math.abs(
        (centers[0]!.x + centers[0]!.width / 2) -
        (centers[1]!.x + centers[1]!.width / 2)
      )).toBeLessThan(3)
      expect(Math.abs(
        (centers[0]!.y + centers[0]!.height / 2) -
        (centers[1]!.y + centers[1]!.height / 2)
      )).toBeLessThan(3)
      await fixture.page.getByRole('button', { name: '移除整个分支', exact: true }).click()

      await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
      await expect(activeSurface(fixture.page)).toHaveAttribute('data-session-id', parentId!)
      await expect(fixture.page.locator(`.terminal-surface[data-session-id="${removableId}"]`)).toHaveCount(0)

      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      const dag = (await fixture.app.windows()).find((page) => page !== fixture.page)!
      await expect(dag.locator(`.dag-node-card[data-session-id="${removableId}"]`)).toHaveCount(0)
      await expect(dag.locator(`.dag-node-card[data-session-id="${parentId}"]`)).toHaveCount(1)
    } finally {
      await fixture.close()
    }
  })

  test('restores completed command Blocks and preserves sibling membership after a full restart', async () => {
    let fixture = await launchSessionCanvas()
    try {
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      // Session creation is an authoritative Runtime command. Under full-suite
      // load the projection can arrive after the button click has resolved;
      // wait for the requested sibling before resolving the active Session.
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      const active = activeSurface(fixture.page)
      const activeId = await active.getAttribute('data-session-id')
      await terminalCommand(active, 'printf "RESTART_JOURNAL_OK\\n"')
      await expect(active.locator('.xterm-rows')).toContainText('RESTART_JOURNAL_OK')
      await waitForCompletedShellBlock(fixture.dataDirectory, activeId!, 'RESTART_JOURNAL_OK')
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
      await waitForCompletedShellBlock(fixture.dataDirectory, firstId!, 'GRACEFUL_FIRST')

      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      const second = activeSurface(fixture.page)
      const secondId = await second.getAttribute('data-session-id')
      await terminalCommand(second, "printf 'GRACEFUL_SECOND\\n'")
      await expect.poll(() => occurrences(second, 'GRACEFUL_SECOND')).toBeGreaterThanOrEqual(2)
      await waitForCompletedShellBlock(fixture.dataDirectory, secondId!, 'GRACEFUL_SECOND')

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

async function waitForCompletedShellBlock(
  dataDirectory: string,
  sessionId: string,
  commandMarker: string
): Promise<void> {
  await expect.poll(() => {
    const database = new DatabaseSync(join(dataDirectory, 'matou.sqlite'), { readOnly: true })
    try {
      return database.prepare(
        `SELECT COUNT(*) AS count
         FROM shell_history_blocks
         WHERE session_id = ? AND command_text LIKE ?`
      ).get(sessionId, `%${commandMarker}%`) as { count: number }
    } finally {
      database.close()
    }
  }).toEqual({ count: 1 })
}
