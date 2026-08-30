import { expect, test } from '@playwright/test'

import {
  activeSurface, launchSessionCanvas, terminalCommand, visibleSurfaces, waitForShell
} from './fixtures/session-canvas-fixture'

test.describe('session canvas basics with real PTYs', () => {
  test.setTimeout(60_000)

  test('starts with a focused Shell, creates another canvas directly, and appends Shell siblings', async () => {
    const fixture = await launchSessionCanvas()
    try {
      const initial = activeSurface(fixture.page)
      await waitForShell(initial)
      await expect(initial.locator('.xterm-helper-textarea')).toBeFocused()
      await terminalCommand(initial, 'printf "CANVAS_INITIAL_OK\\n"')
      await expect(initial.locator('.xterm-rows')).toContainText('CANVAS_INITIAL_OK')

      await fixture.page.getByRole('button', { name: '新建页签' }).click()
      await expect(fixture.page.getByRole('tab')).toHaveCount(2)
      await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
      const secondCanvasShell = activeSurface(fixture.page)
      const originalSecondCanvasSessionId = await secondCanvasShell.getAttribute('data-session-id')
      await waitForShell(secondCanvasShell)
      await expect(secondCanvasShell.locator('.xterm-helper-textarea')).toBeFocused()
      await terminalCommand(secondCanvasShell, 'pwd')
      await expect(secondCanvasShell.locator('.xterm-rows')).toContainText(fixture.workspaceDirectory)

      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      const appended = activeSurface(fixture.page)
      await expect.poll(async () => appended.getAttribute('data-session-id'))
        .toBe(await visibleSurfaces(fixture.page).last().getAttribute('data-session-id'))
      await expect(appended.locator('.xterm-helper-textarea')).toBeFocused()
      await terminalCommand(appended, 'printf "SIBLING_FOCUS_OK\\n"')
      await expect(appended.locator('.xterm-rows')).toContainText('SIBLING_FOCUS_OK')
      await expect(fixture.page.locator(
        `.terminal-surface[data-session-id="${originalSecondCanvasSessionId}"] .xterm-rows`
      )).not.toContainText('SIBLING_FOCUS_OK')
    } finally {
      await fixture.close()
    }
  })

  test('isolates both Runtime data and the Electron profile under one temporary run root', async () => {
    const fixture = await launchSessionCanvas()
    try {
      expect(fixture.rootDirectory).toMatch(/^\/tmp\/matou-e2e-/)
      const paths = await fixture.app.evaluate(({ app }) => ({ userData: app.getPath('userData') }))
      expect(paths.userData).toBe(fixture.electronUserDataDirectory)
      expect(fixture.dataDirectory.startsWith(fixture.rootDirectory)).toBe(true)
      expect(fixture.workspaceDirectory.startsWith(fixture.rootDirectory)).toBe(true)
    } finally {
      await fixture.close()
    }
  })

  test('keeps terminal history scrollbars narrow and visually separate from card shadows', async () => {
    const fixture = await launchSessionCanvas()
    try {
      const surface = activeSurface(fixture.page)
      await waitForShell(surface)
      await terminalCommand(surface, 'for i in {1..120}; do echo "SCROLLBAR_$i"; done')
      await expect(surface.locator('.xterm-rows')).toContainText('SCROLLBAR_120')
      await surface.hover()

      const scrollbar = await surface.locator('.xterm-viewport').evaluate((element) => ({
        width: getComputedStyle(element, '::-webkit-scrollbar').width,
        track: getComputedStyle(element, '::-webkit-scrollbar-track').backgroundColor,
        thumb: getComputedStyle(element, '::-webkit-scrollbar-thumb').backgroundColor
      }))

      expect(parseFloat(scrollbar.width)).toBeLessThanOrEqual(6)
      expect(scrollbar.track).toBe('rgba(0, 0, 0, 0)')
      expect(scrollbar.thumb).not.toBe('rgb(221, 221, 221)')
    } finally {
      await fixture.close()
    }
  })
})
