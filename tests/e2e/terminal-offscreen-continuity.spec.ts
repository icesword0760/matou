import { execFileSync } from 'node:child_process'
import { access, chmod, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test, type Locator } from '@playwright/test'

import { readSessionFrames } from '../../apps/runtime/src/journal/segment-journal'
import {
  launchMatou,
  restartMatou,
  stopMatouPreservingData,
  type MatouFixture
} from './matou-fixture'
import { terminalCommand, waitForShell } from './fixtures/session-canvas-fixture'
import { seedScaleDatabase } from './scale/scale-database'

const SESSION_COUNT = 81
const TARGET_SESSION_ID = 'scale-sibling-00001'

test.describe('real foreground terminal continuity outside the carousel viewport', () => {
  test.setTimeout(3 * 60_000)

  test('keeps one established PTY and its VT history live while its card is virtualized', async () => {
    test.skip(process.platform !== 'darwin', 'the display-constrained terminal gate runs on macOS')
    assertAcceptanceDisplaysBeforeLaunch()
    let fixture: MatouFixture | undefined
    let targetPid = 0
    try {
      fixture = await launchMatou({ env: { MATOU_E2E_SCALE: '1' } })
      await expectOnlyColorLcdWindows(fixture)
      await stopMatouPreservingData(fixture)
      await seedScaleDatabase(fixture.dataDirectory, { siblingSessions: SESSION_COUNT })

      fixture = await restartMatou(fixture, { env: { MATOU_E2E_SCALE: '1' } })
      await expectOnlyColorLcdWindows(fixture)
      const carousel = fixture.page.getByRole('region', { name: '同级会话列表' })
      await expect(carousel).toHaveAttribute('data-total-sessions', String(SESSION_COUNT))
      await expect(carousel).toHaveAttribute('data-foreground-terminals', String(SESSION_COUNT))

      const target = terminalSurface(fixture, TARGET_SESSION_ID)
      await expect(target).toBeVisible()
      await waitForShell(target)
      targetPid = positiveInteger(await target.getAttribute('data-pid'), 'target PTY PID')
      const releasePath = join(fixture.rootDirectory, 'offscreen-output.release')
      const donePath = join(fixture.rootDirectory, 'offscreen-output.done')
      const scriptPath = join(fixture.rootDirectory, 'offscreen-output.sh')
      await writeFile(scriptPath, outputScript(releasePath, donePath))
      await chmod(scriptPath, 0o755)

      await terminalCommand(target, shellQuote(scriptPath))
      await expect(target.locator('.xterm-rows')).toContainText('VIEWPORT_OUTPUT_READY')

      await scrollToCarouselEdge(carousel, 'end')
      await expect(target).toHaveCount(0)
      expect(await processExists(fixture, targetPid)).toBe(true)
      await expectOnlyColorLcdWindows(fixture)

      await writeFile(releasePath, 'continue\n')
      await expect.poll(() => access(donePath).then(() => true, () => false), {
        message: 'the real PTY must finish producing output while its card is outside the DOM'
      }).toBe(true)
      await expect.poll(() => journalText(fixture!.dataDirectory, TARGET_SESSION_ID), {
        message: 'offscreen PTY output must reach the authoritative Journal before remount'
      }).toContain('VIEWPORT_OUTPUT_DONE')
      expect(await processExists(fixture, targetPid)).toBe(true)

      await scrollToCarouselEdge(carousel, 'start')
      const restored = terminalSurface(fixture, TARGET_SESSION_ID)
      await expect(restored).toBeVisible()
      await expect(restored).toHaveAttribute('data-pid', String(targetPid))
      const pane = restored.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
      await expect(pane.locator('.session-recovery-overlay')).toHaveCount(0)
      await expect(pane).not.toHaveAttribute('aria-busy', 'true')

      const rows = restored.locator('.xterm-rows')
      await expect(rows).toContainText('VIEWPORT_OUTPUT_READY')
      await expect(rows).toContainText('VIEWPORT_OUTPUT_01')
      await expect(rows).toContainText('VIEWPORT_OUTPUT_12')
      await expect(rows).toContainText('VIEWPORT_OUTPUT_DONE')
      const restoredText = await rows.textContent() ?? ''
      expect(occurrences(restoredText, 'VIEWPORT_OUTPUT_READY')).toBe(1)
      expect(occurrences(restoredText, 'VIEWPORT_OUTPUT_DONE')).toBe(1)

      for (let cycle = 1; cycle <= 3; cycle += 1) {
        const marker = `VIEWPORT_CYCLE_${cycle}_DONE`
        const cycleScript = join(fixture.rootDirectory, `offscreen-cycle-${cycle}.sh`)
        await writeFile(cycleScript, delayedMarkerScript(marker))
        await chmod(cycleScript, 0o755)
        await terminalCommand(terminalSurface(fixture, TARGET_SESSION_ID), shellQuote(cycleScript))
        await scrollToCarouselEdge(carousel, 'end')
        await expect(terminalSurface(fixture, TARGET_SESSION_ID)).toHaveCount(0)
        await expect.poll(() => journalText(fixture!.dataDirectory, TARGET_SESSION_ID), {
          message: `cycle ${cycle} output must remain durable while the terminal is virtualized`
        }).toContain(marker)
        expect(await processExists(fixture, targetPid)).toBe(true)

        await scrollToCarouselEdge(carousel, 'start')
        const returned = terminalSurface(fixture, TARGET_SESSION_ID)
        await expect(returned).toBeVisible()
        await expect(returned).toHaveAttribute('data-pid', String(targetPid))
        await expect(returned.locator('.xterm-rows')).toContainText(marker)
        const returnedText = await returned.locator('.xterm-rows').textContent() ?? ''
        expect(occurrences(returnedText, marker)).toBe(1)
        await expect(returned.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]'))
          .not.toHaveAttribute('aria-busy', 'true')
      }

      const finalSurface = terminalSurface(fixture, TARGET_SESSION_ID)
      await terminalCommand(finalSurface, "printf 'VIEWPORT_INPUT_AFTER_RETURN\\n'")
      await expect(finalSurface.locator('.xterm-rows')).toContainText('VIEWPORT_INPUT_AFTER_RETURN')
      expect(await processExists(fixture, targetPid)).toBe(true)
      await expectOnlyColorLcdWindows(fixture)
    } finally {
      await fixture?.close()
      if (targetPid > 0) {
        await expect.poll(() => processExistsLocally(targetPid), {
          timeout: 30_000,
          message: 'the retained offscreen PTY must exit with the App fixture'
        }).toBe(false)
      }
    }
  })
})

function terminalSurface(fixture: MatouFixture, sessionId: string): Locator {
  return fixture.page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
}

async function scrollToCarouselEdge(
  carousel: Locator,
  edge: 'start' | 'end'
): Promise<void> {
  await carousel.evaluate((element, requestedEdge) => {
    const viewport = element as HTMLElement
    viewport.scrollLeft = requestedEdge === 'start'
      ? 0
      : Math.max(0, viewport.scrollWidth - viewport.clientWidth)
    viewport.dispatchEvent(new Event('scroll', { bubbles: true }))
  }, edge)
  await expect.poll(() => carousel.evaluate((element) => {
    const viewport = element as HTMLElement
    return {
      atStart: viewport.scrollLeft < 1,
      atEnd: Math.abs(viewport.scrollWidth - viewport.clientWidth - viewport.scrollLeft) < 2
    }
  })).toMatchObject(edge === 'start' ? { atStart: true } : { atEnd: true })
}

function outputScript(releasePath: string, donePath: string): string {
  return [
    '#!/bin/sh',
    "printf 'VIEWPORT_OUTPUT_READY\\n'",
    `while [ ! -f ${shellQuote(releasePath)} ]; do sleep 0.05; done`,
    'i=1',
    'while [ "$i" -le 12 ]; do',
    "  printf 'VIEWPORT_OUTPUT_%02d\\n' \"$i\"",
    '  sleep 0.05',
    '  i=$((i + 1))',
    'done',
    "printf 'VIEWPORT_OUTPUT_DONE\\n'",
    `printf done > ${shellQuote(donePath)}`,
    ''
  ].join('\n')
}

function delayedMarkerScript(marker: string): string {
  return [
    '#!/bin/sh',
    'sleep 0.2',
    `printf '${marker}\\n'`,
    ''
  ].join('\n')
}

async function journalText(dataRoot: string, sessionId: string): Promise<string> {
  const frames = await readSessionFrames(dataRoot, sessionId)
  return frames.filter((frame) => frame.kind === 'output')
    .map((frame) => new TextDecoder().decode(frame.data)).join('')
}

function occurrences(value: string, search: string): number {
  return value.split(search).length - 1
}

function positiveInteger(value: string | null, label: string): number {
  const parsed = Number(value)
  if (!Number.isInteger(parsed) || parsed <= 0) throw new Error(`${label} is invalid`)
  return parsed
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`
}

async function processExists(fixture: MatouFixture, pid: number): Promise<boolean> {
  return fixture.app.evaluate((_electron, candidatePid) => {
    try {
      process.kill(candidatePid, 0)
      return true
    } catch (error) {
      return !(error instanceof Error && 'code' in error && error.code === 'ESRCH')
    }
  }, pid)
}

function processExistsLocally(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    return !(error instanceof Error && 'code' in error && error.code === 'ESRCH')
  }
}

function assertAcceptanceDisplaysBeforeLaunch(): void {
  const displays = execFileSync('system_profiler', ['SPDisplaysDataType'], { encoding: 'utf8' })
  expect(displays).toContain('XV272U:')
  expect(displays).toContain('Color LCD:')
}

async function expectOnlyColorLcdWindows(fixture: MatouFixture): Promise<void> {
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    const visible = BrowserWindow.getAllWindows().filter((window) => window.isVisible()).map((window) => {
      const bounds = window.getBounds()
      const display = screen.getDisplayMatching(bounds)
      return {
        internal: display.internal,
        primary: display.id === primary.id,
        fullyContained: bounds.x >= display.workArea.x && bounds.y >= display.workArea.y &&
          bounds.x + bounds.width <= display.workArea.x + display.workArea.width &&
          bounds.y + bounds.height <= display.workArea.y + display.workArea.height
      }
    })
    return { primaryLabel: primary.label, visible }
  })).toEqual({
    primaryLabel: 'XV272U',
    // Electron localizes the built-in display label on Chinese macOS. The
    // pre-launch system_profiler assertion identifies it as Color LCD; here
    // the native `internal` flag and primary identity bind the same display.
    visible: [{ internal: true, primary: false, fullyContained: true }]
  })
}
