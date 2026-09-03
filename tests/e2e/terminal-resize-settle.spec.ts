import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test } from '@playwright/test'

import {
  activeSurface, launchSessionCanvas, terminalCommand, visibleSurfaces, waitForShell
} from './fixtures/session-canvas-fixture'

test('sends one real PTY resize after a focused card width transition settles', async () => {
  test.setTimeout(60_000)
  const fixture = await launchSessionCanvas()
  try {
    await waitForShell(activeSurface(fixture.page))
    await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
    await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
    await expect(visibleSurfaces(fixture.page)).toHaveCount(3)

    const first = visibleSurfaces(fixture.page).first()
    const last = visibleSurfaces(fixture.page).last()
    await waitForShell(first)
    await waitForShell(last)

    const resizeLog = join(fixture.rootDirectory, 'terminal-resize.log')
    await terminalCommand(
      first,
      `: > '${resizeLog}'; trap 'printf "WINCH\\n" >> "${resizeLog}"' WINCH; printf 'RESIZE_TRAP_READY\\n'`
    )
    await expect(first.locator('.xterm-rows')).toContainText('RESIZE_TRAP_READY')
    await fixture.page.waitForTimeout(700)
    await writeFile(resizeLog, '')

    await last.click({ position: { x: 24, y: 90 } })
    await expect(last.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]'))
      .toHaveAttribute('data-active', 'true')
    await fixture.page.waitForTimeout(700)

    const resizeSignals = (await readFile(resizeLog, 'utf8'))
      .split('\n')
      .filter(Boolean)
    expect(resizeSignals).toEqual(['WINCH'])
  } finally {
    await fixture.close()
  }
})
