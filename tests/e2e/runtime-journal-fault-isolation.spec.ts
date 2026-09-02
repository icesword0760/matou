import { rename, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test, type Locator } from '@playwright/test'

import { launchMatou } from './matou-fixture'
import {
  terminalCommand, visibleSurfaces, waitForShell
} from './fixtures/session-canvas-fixture'

test.describe('real App Journal fault isolation with two live PTYs', () => {
  test.setTimeout(90_000)

  test('keeps card B interactive while card A recovers ENOSPC on its original PID', async () => {
    const root = `/tmp/matou-e2e-journal-fault-${process.pid}-${Date.now()}`
    const controlPath = join(root, 'journal-fault-control.json')
    const fixture = await launchMatou({
      root,
      env: { MATOU_E2E_JOURNAL_FAULT_CONTROL: controlPath }
    })
    try {
      await expectWindowOnColorLcd(fixture)
      const firstSurface = visibleSurfaces(fixture.page).first()
      await waitForShell(firstSurface)
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)

      const surfaceA = visibleSurfaces(fixture.page).first()
      const surfaceB = visibleSurfaces(fixture.page).last()
      await Promise.all([waitForShell(surfaceA), waitForShell(surfaceB)])
      const sessionA = await requiredAttribute(surfaceA, 'data-session-id')
      const sessionB = await requiredAttribute(surfaceB, 'data-session-id')
      const stableA = fixture.page.locator(`.terminal-surface[data-session-id="${sessionA}"]`)
      const stableB = fixture.page.locator(`.terminal-surface[data-session-id="${sessionB}"]`)
      const pidA = Number(await requiredAttribute(surfaceA, 'data-pid'))
      const pidB = Number(await requiredAttribute(surfaceB, 'data-pid'))
      expect(pidA).not.toBe(pidB)

      await terminalCommand(stableA, 'printf "A_READY\\n"')
      await expect(stableA.locator('.xterm-rows')).toContainText('A_READY')
      await terminalCommand(stableB, 'printf "B_READY\\n"')
      await expect(stableB.locator('.xterm-rows')).toContainText('B_READY')

      const paneA = paneFor(stableA)
      const paneB = paneFor(stableB)
      const inputA = stableA.locator('.xterm-helper-textarea')
      await stableA.click({ position: { x: 12, y: 12 } })
      await expect(paneA).toHaveAttribute('data-active', 'true')
      await inputA.focus()
      await inputA.pressSequentially(
        'printf "A_RETAINED_ENOSPC\\n"', { delay: 2 }
      )
      // Keep the pre-fault command on one terminal row so this readiness check
      // measures complete input delivery instead of xterm's visual line wrap.
      await expect(stableA.locator('.xterm-rows')).toContainText('A_RETAINED_ENOSPC')
      await setFaultControl(controlPath, { sessionId: sessionA, code: 'ENOSPC' })
      await fixture.page.keyboard.press('Enter')
      const fault = paneA.getByRole('status', { name: /终端记录写入异常/ })
      await expect(fault).toBeVisible()
      await expect(fault).toContainText('终端已暂停：输出记录写入失败')
      await expect(fault).toContainText('磁盘空间或存储配额不足')
      await expect(fault).toContainText('其他会话不受影响')
      await expect(fault.getByRole('button', { name: '重试写入' })).toBeVisible()
      await expectOverlayCoversCard(paneA, fault)
      await expect(paneB.locator('.storage-fault-overlay')).toHaveCount(0)

      await terminalCommand(stableB, 'printf "B_CONTINUES_WHILE_A_PAUSED\\n"')
      await expect(stableB.locator('.xterm-rows')).toContainText('B_CONTINUES_WHILE_A_PAUSED')
      await expect(fault).toBeVisible()

      await setFaultControl(controlPath, {})
      await fault.getByRole('button', { name: '重试写入' }).click()
      await expect(fault).toBeHidden()
      await expect(stableA).toHaveAttribute('data-pid', String(pidA))
      await expect(stableA.locator('.xterm-rows')).toContainText('A_RETAINED_ENOSPC')
      expect(await fixture.app.evaluate((_electron, pid) => {
        try { process.kill(pid, 0); return true } catch { return false }
      }, pidA)).toBe(true)

      await terminalCommand(stableA, 'printf "A_CONTINUES_AFTER_RETRY\\n"')
      await expect(stableA.locator('.xterm-rows')).toContainText('A_CONTINUES_AFTER_RETRY')
      await expect(stableB).toHaveAttribute('data-pid', String(pidB))
    } finally {
      await fixture.close()
    }
  })
})

function paneFor(surface: Locator): Locator {
  return surface.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
}

async function expectOverlayCoversCard(card: Locator, overlay: Locator): Promise<void> {
  const [cardBox, overlayBox] = await Promise.all([card.boundingBox(), overlay.boundingBox()])
  expect(cardBox).not.toBeNull()
  expect(overlayBox).not.toBeNull()
  for (const key of ['x', 'y', 'width', 'height'] as const) {
    expect(Math.abs(cardBox![key] - overlayBox![key])).toBeLessThanOrEqual(1)
  }
}

async function requiredAttribute(locator: Locator, name: string): Promise<string> {
  const value = await locator.getAttribute(name)
  if (!value) throw new Error(`${name} is missing`)
  return value
}

async function setFaultControl(path: string, value: unknown): Promise<void> {
  const temporary = `${path}.next`
  await writeFile(temporary, JSON.stringify(value))
  await rename(temporary, path)
}

async function expectWindowOnColorLcd(
  fixture: Awaited<ReturnType<typeof launchMatou>>
): Promise<void> {
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const window = BrowserWindow.getAllWindows().find((candidate) => candidate.isVisible())
    if (!window) return { internal: false, primary: true }
    const display = screen.getDisplayMatching(window.getBounds())
    return { internal: display.internal, primary: display.id === screen.getPrimaryDisplay().id }
  })).toEqual({ internal: true, primary: false })
}
