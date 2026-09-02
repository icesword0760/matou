import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

import { expect, test, type Locator } from '@playwright/test'

import {
  expectVisibleWindowsOnPrimaryDisplay,
  launchMatou,
  primaryAcceptanceDisplayRequested,
  type MatouFixture
} from './matou-fixture'
import { terminalCommand, visibleSurfaces } from './fixtures/session-canvas-fixture'

const execFileAsync = promisify(execFile)

interface RuntimeMetrics {
  runtimePid: number
  ptyCount: number
  ptyPids: number[]
}

test('recovers two foreground terminals after only Runtime is killed with healthy storage', async () => {
  test.skip(process.platform !== 'darwin', 'the visible acceptance window must use the internal display')
  test.setTimeout(90_000)
  const fixture = await launchMatou({ env: {
    MATOU_E2E_SCALE: '1', MATOU_E2E_TERMINAL_DIAGNOSTICS: '0'
  } })
  try {
    const appPid = fixture.app.process().pid
    await expectMainWindowOnInternalDisplay(fixture)
    await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
    await expect(visibleSurfaces(fixture.page)).toHaveCount(2)

    const sessionIds = await visibleSurfaces(fixture.page).evaluateAll((surfaces) =>
      surfaces.map((surface) => surface.getAttribute('data-session-id')).filter(Boolean) as string[]
    )
    expect(new Set(sessionIds).size).toBe(2)
    const beforePids = new Map<string, number>()
    const beforeMarkers = new Map<string, string>()
    for (const [index, sessionId] of sessionIds.entries()) {
      const surface = surfaceFor(fixture, sessionId)
      const marker = `MATOU_${index === 0 ? 'A' : 'B'}_BEFORE`
      beforeMarkers.set(sessionId, marker)
      await terminalCommand(surface, `/bin/echo ${marker}`)
      await expect(surface.locator('.xterm-rows')).toContainText(marker)
      beforePids.set(sessionId, Number(await surface.getAttribute('data-pid')))
    }

    const beforeRuntime = await readRuntimeMetrics(fixture)
    expect(beforeRuntime.ptyCount).toBe(2)
    expect(new Set(beforeRuntime.ptyPids).size).toBe(2)
    await installRecoveryOverlayProbe(fixture, sessionIds)

    process.kill(beforeRuntime.runtimePid, 'SIGKILL')

    await expect.poll(() => readRecoveryOverlayProbe(fixture), {
      message: 'each foreground card must visibly enter recovery after the Runtime dies'
    }).toMatchObject({ seenSessionIds: expect.arrayContaining(sessionIds), maxVisible: 2 })

    let recoveredRuntime: RuntimeMetrics | undefined
    await expect.poll(async () => {
      const candidate = await readRuntimeMetrics(fixture).catch(() => undefined)
      if (!candidate || candidate.runtimePid === beforeRuntime.runtimePid || candidate.ptyCount !== 2) return false
      recoveredRuntime = candidate
      return true
    }, { message: 'a fresh Runtime must restore exactly the two foreground PTYs' }).toBe(true)

    expect(recoveredRuntime).toBeDefined()
    expect(fixture.app.process().pid).toBe(appPid)
    expect(fixture.page.isClosed()).toBe(false)
    expect(new Set(recoveredRuntime!.ptyPids).size).toBe(2)
    await expect.poll(() => runtimeUtilityPids(fixture.app.process().pid))
      .toEqual([recoveredRuntime!.runtimePid])

    const recoveredPids = new Set<number>()
    for (const [index, sessionId] of sessionIds.entries()) {
      const surface = surfaceFor(fixture, sessionId)
      await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
      const recoveredPid = Number(await surface.getAttribute('data-pid'))
      recoveredPids.add(recoveredPid)
      expect(recoveredPid).not.toBe(beforePids.get(sessionId))
      await expect(surface.locator('.xterm-rows')).toContainText(beforeMarkers.get(sessionId)!)

      const marker = `MATOU_${index === 0 ? 'A' : 'B'}_AFTER`
      await terminalCommand(surface, `/bin/echo ${marker}`)
      await expect(surface.locator('.xterm-rows')).toContainText(marker)
    }
    expect(recoveredPids).toEqual(new Set(recoveredRuntime!.ptyPids))
    await expect.poll(() => [...beforePids.values()].every((pid) => !processExists(pid))).toBe(true)
  } finally {
    await fixture.close()
  }
})

function surfaceFor(fixture: MatouFixture, sessionId: string): Locator {
  return fixture.page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
}

async function readRuntimeMetrics(fixture: MatouFixture): Promise<RuntimeMetrics> {
  return fixture.app.evaluate(async () => {
    const read = (globalThis as typeof globalThis & {
      __matouE2eScaleMetrics?: () => Promise<RuntimeMetrics>
    }).__matouE2eScaleMetrics
    if (!read) throw new Error('Runtime metrics bridge is unavailable')
    return read()
  })
}

async function installRecoveryOverlayProbe(fixture: MatouFixture, sessionIds: string[]): Promise<void> {
  await fixture.page.evaluate((ids) => {
    const targets = ids.map((sessionId) => ({
      sessionId,
      pane: document.querySelector(`.terminal-surface[data-session-id="${sessionId}"]`)
        ?.closest<HTMLElement>('[data-testid="terminal-pane"]')
    }))
    if (targets.some(({ pane }) => !pane)) throw new Error('A foreground terminal pane is missing')
    const result = { seenSessionIds: [] as string[], maxVisible: 0 }
    const scan = () => {
      const visible = targets.filter(({ pane }) => pane?.querySelector('.session-recovery-overlay'))
      result.maxVisible = Math.max(result.maxVisible, visible.length)
      for (const { sessionId } of visible) {
        if (!result.seenSessionIds.includes(sessionId)) result.seenSessionIds.push(sessionId)
      }
    }
    new MutationObserver(scan).observe(document.body, {
      subtree: true, childList: true, attributes: true, attributeFilter: ['aria-busy']
    })
    ;(window as typeof window & { __matouRuntimeCrashProbe?: typeof result })
      .__matouRuntimeCrashProbe = result
    scan()
  }, sessionIds)
}

async function readRecoveryOverlayProbe(fixture: MatouFixture): Promise<{
  seenSessionIds: string[]
  maxVisible: number
}> {
  return fixture.page.evaluate(() => (
    window as typeof window & {
      __matouRuntimeCrashProbe?: { seenSessionIds: string[]; maxVisible: number }
    }
  ).__matouRuntimeCrashProbe ?? { seenSessionIds: [], maxVisible: 0 })
}

async function runtimeUtilityPids(electronPid: number): Promise<number[]> {
  const { stdout } = await execFileAsync('ps', ['-axo', 'pid=,ppid=,command='])
  return stdout.split('\n').flatMap((line) => {
    const match = line.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/)
    return match && Number(match[2]) === electronPid &&
      match[3]!.includes('--utility-sub-type=node.mojom.NodeService')
      ? [Number(match[1])]
      : []
  }).sort((left, right) => left - right)
}

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

async function expectMainWindowOnInternalDisplay(fixture: MatouFixture): Promise<void> {
  if (primaryAcceptanceDisplayRequested()) {
    await expectVisibleWindowsOnPrimaryDisplay(fixture)
    return
  }
  let result: { target: Electron.Rectangle; bounds: Electron.Rectangle } | undefined
  await expect.poll(async () => {
    result = await fixture.app.evaluate(({ BrowserWindow, screen }) => {
      const primaryId = screen.getPrimaryDisplay().id
      const candidates = screen.getAllDisplays().filter(({ id }) => id !== primaryId)
      const target = candidates.find(({ internal }) => internal) ??
        candidates.find(({ label }) => /color\s*lcd|built[- ]?in|内建/i.test(label)) ??
        candidates[0]
      const window = BrowserWindow.getAllWindows().find((candidate) => candidate.isVisible())
      return target && window ? { target: target.workArea, bounds: window.getBounds() } : undefined
    })
    return result !== undefined
  }, { message: 'the internal Color LCD and visible main window must both exist' }).toBe(true)
  const { target, bounds } = result!
  expect(bounds.x).toBeGreaterThanOrEqual(target.x)
  expect(bounds.y).toBeGreaterThanOrEqual(target.y)
  expect(bounds.x + bounds.width).toBeLessThanOrEqual(target.x + target.width)
  expect(bounds.y + bounds.height).toBeLessThanOrEqual(target.y + target.height)
}
