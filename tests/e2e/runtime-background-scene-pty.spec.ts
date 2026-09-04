import { access, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test } from '@playwright/test'

import {
  expectVisibleWindowsOnPrimaryDisplay,
  launchMatou,
  primaryAcceptanceDisplayRequested
} from './matou-fixture'
import {
  activeSurface, terminalCommand, waitForShell
} from './fixtures/session-canvas-fixture'

test.describe('real PTY continuity across background Scenes', () => {
  test.setTimeout(90_000)

  test('retains a background Scene output stream and resumes its original PID', async () => {
    const fixture = await launchMatou()
    const backgroundRelease = join(fixture.rootDirectory, 'scene-a-background.release')
    const backgroundDone = join(fixture.rootDirectory, 'scene-a-background.done')
    try {
      await expectAllVisibleWindowsOnColorLcd(fixture)
      const tabs = fixture.page.locator('.scene-tabs > [data-scene-id]')
      await expect(tabs).toHaveCount(1)
      const sceneAId = await requiredAttribute(tabs.first(), 'data-scene-id')
      const surfaceA = activeSurface(fixture.page)
      await waitForShell(surfaceA)
      const sessionA = await requiredAttribute(surfaceA, 'data-session-id')
      const pidA = Number(await requiredAttribute(surfaceA, 'data-pid'))
      await surfaceA.locator('.xterm').evaluate((element) => {
        element.setAttribute('data-e2e-cache-probe', 'scene-a-warm-model')
      })

      await terminalCommand(surfaceA, backgroundCommand(backgroundRelease, backgroundDone))
      await expect(surfaceA.locator('.xterm-rows')).toContainText('A_BACKGROUND_01')

      await fixture.page.getByRole('button', { name: '新建页签' }).click()
      await expect(tabs).toHaveCount(2)
      const sceneBId = await requiredAttribute(tabs.last(), 'data-scene-id')
      expect(sceneBId).not.toBe(sceneAId)
      const surfaceB = activeSurface(fixture.page)
      await waitForShell(surfaceB)
      const sessionB = await requiredAttribute(surfaceB, 'data-session-id')
      const pidB = Number(await requiredAttribute(surfaceB, 'data-pid'))
      expect(sessionB).not.toBe(sessionA)
      expect(pidB).not.toBe(pidA)
      await expect(fixture.page.locator(`.terminal-surface[data-session-id="${sessionA}"]`))
        .toHaveCount(0)

      await terminalCommand(surfaceB, 'printf "B_INTERACTIVE_WHILE_A_RUNS\\n"')
      await expect(surfaceB.locator('.xterm-rows')).toContainText('B_INTERACTIVE_WHILE_A_RUNS')
      await expect(access(backgroundDone).then(() => true, () => false)).resolves.toBe(false)

      await writeFile(backgroundRelease, 'release')
      await expect.poll(() => access(backgroundDone).then(() => true, () => false)).toBe(true)
      expect(await processExists(fixture, pidA)).toBe(true)
      await expectAllVisibleWindowsOnColorLcd(fixture)

      await fixture.page.locator(`[data-scene-id="${sceneAId}"]`).getByRole('tab').click()
      const restoredA = activeSurface(fixture.page)
      await expect(restoredA).toHaveAttribute('data-session-id', sessionA)
      await expect(restoredA).toHaveAttribute('data-pid', String(pidA))
      await expect(restoredA.locator('.xterm'))
        .toHaveAttribute('data-e2e-cache-probe', 'scene-a-warm-model')
      const restoredPane = restoredA.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
      await expect(restoredPane).not.toHaveAttribute('aria-busy', 'true')
      await expect(restoredPane.locator('.session-recovery-overlay')).toHaveCount(0)
      await expect.poll(async () => restoredA.locator('.xterm-rows').textContent())
        .toContain('A_BACKGROUND_12')
      const restoredText = await restoredA.locator('.xterm-rows').textContent()
      for (let index = 1; index <= 12; index += 1) {
        expect(restoredText).toContain(`A_BACKGROUND_${String(index).padStart(2, '0')}`)
      }

      await terminalCommand(restoredA, 'printf "A_INPUT_AFTER_RETURN\\n"')
      await expect(restoredA.locator('.xterm-rows')).toContainText('A_INPUT_AFTER_RETURN')
      expect(await processExists(fixture, pidA)).toBe(true)
      expect(await processExists(fixture, pidB)).toBe(true)
      await expectAllVisibleWindowsOnColorLcd(fixture)
    } finally {
      await fixture.close()
    }
  })
})

function backgroundCommand(releasePath: string, donePath: string): string {
  return [
    'printf "A_BACKGROUND_01\\n"',
    `while [ ! -f ${shellQuote(releasePath)} ]`,
    'do sleep 0.1',
    'done',
    'i=2',
    'while [ "$i" -le 12 ]',
    'do printf "A_BACKGROUND_%02d\\n" "$i"',
    'sleep 0.1',
    'i=$((i + 1))',
    'done',
    `printf done > ${shellQuote(donePath)}`
  ].join('; ')
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`
}

async function requiredAttribute(
  locator: import('@playwright/test').Locator,
  name: string
): Promise<string> {
  const value = await locator.getAttribute(name)
  if (!value) throw new Error(`${name} is missing`)
  return value
}

async function processExists(
  fixture: Awaited<ReturnType<typeof launchMatou>>,
  pid: number
): Promise<boolean> {
  return fixture.app.evaluate((_electron, candidatePid) => {
    try { process.kill(candidatePid, 0); return true } catch { return false }
  }, pid)
}

async function expectAllVisibleWindowsOnColorLcd(
  fixture: Awaited<ReturnType<typeof launchMatou>>
): Promise<void> {
  if (primaryAcceptanceDisplayRequested()) {
    await expectVisibleWindowsOnPrimaryDisplay(fixture)
    return
  }
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    const windows = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
      .map((window) => {
        const display = screen.getDisplayMatching(window.getBounds())
        return { internal: display.internal, primary: display.id === primary.id }
      })
    return {
      primaryLabel: primary.label,
      primaryInternal: primary.internal,
      windowCount: windows.length,
      allWindowsInternalSecondary: windows.length > 0 &&
        windows.every(({ internal, primary: isPrimary }) => internal && !isPrimary)
    }
  })).toEqual({
    primaryLabel: 'XV272U',
    primaryInternal: false,
    windowCount: 1,
    allWindowsInternalSecondary: true
  })
}
