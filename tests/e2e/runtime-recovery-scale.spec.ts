import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

import { expect, test, type Locator, type Page } from '@playwright/test'

import { readSessionFrames } from '../../apps/runtime/src/journal/segment-journal'
import {
  launchMatou,
  restartMatou,
  stopMatouPreservingData,
  type MatouFixture
} from './matou-fixture'
import { terminalCommand } from './fixtures/session-canvas-fixture'
import {
  historyMarker,
  readRecoveryScaleCounts,
  RECOVERY_SCALE_ACTIVE_SESSION_ID,
  RECOVERY_SCALE_BACKGROUND_IDS,
  RECOVERY_SCALE_BACKGROUND_TASK_ID,
  RECOVERY_SCALE_BACKGROUND_WORKSPACE_ID,
  RECOVERY_SCALE_FOREGROUND_IDS,
  RECOVERY_SCALE_SESSION_IDS,
  seedRuntimeRecoveryScale
} from './fixtures/runtime-recovery-scale-fixture'

const execFileAsync = promisify(execFile)

interface RuntimeMetrics {
  runtimePid: number
  ptyCount: number
  ptySessions: Array<{ sessionId: string; pid: number }>
  recoveryObservation?: {
    maxRestoring: number
    transitions: Array<{
      sequence: number
      sessionId: string
      sceneId: string
      priority: 'active-session' | 'foreground-scene' | 'active-task' | 'active-workspace' | 'background'
      state: 'queued' | 'restoring' | 'ready' | 'failed'
      restoringCount: number
    }>
  }
}

test('recovers twenty real PTYs fairly across a durable 100-Session hierarchy after Runtime SIGKILL', async () => {
  test.skip(process.platform !== 'darwin', 'the display-constrained recovery gate runs on macOS')
  test.setTimeout(4 * 60_000)
  let fixture: MatouFixture | undefined
  const previousPids: number[] = []
  try {
    fixture = await launchMatou({ env: recoveryScaleEnvironment() })
    await expect(fixture.page.locator('.hierarchy-shell')).toBeVisible()
    await assertVisibleWindowsOnSecondaryColorLcd(fixture)
    await stopMatouPreservingData(fixture)

    await seedRuntimeRecoveryScale(fixture.dataDirectory)
    expect(await readRecoveryScaleCounts(fixture.dataDirectory)).toEqual({
      sessions: 100,
      recoverySessions: 20,
      recoveryWorkspaces: 2,
      recoveryTasks: 2,
      recoveryScenes: 2,
      workspaces: 3,
      tasks: 11,
      scenes: 13
    })

    fixture = await restartMatou(fixture, { env: recoveryScaleEnvironment() })
    const appPid = fixture.app.process().pid
    await expect(activeCarousel(fixture.page)).toHaveAttribute('data-total-sessions', '16')
    await assertVisibleWindowsOnSecondaryColorLcd(fixture)

    const before = await waitForRecoveredRuntime(fixture, undefined)
    expect(before.ptyCount).toBe(20)
    expect(new Set(before.ptySessions.map(({ pid }) => pid)).size).toBe(20)
    expect(new Set(before.ptySessions.map(({ sessionId }) => sessionId)))
      .toEqual(new Set(RECOVERY_SCALE_SESSION_IDS))
    previousPids.push(...before.ptySessions.map(({ pid }) => pid))

    await writeHistoryBeforeCrash(fixture)

    const visibleSessionIds = await visibleCardSessionIds(fixture.page)
    expect(visibleSessionIds.length).toBeGreaterThan(0)
    expect(visibleSessionIds.every((sessionId) => RECOVERY_SCALE_FOREGROUND_IDS.includes(sessionId)))
      .toBe(true)
    await installCardRecoveryProbe(fixture.page, visibleSessionIds)

    process.kill(before.runtimePid, 'SIGKILL')

    await expect.poll(() => readCardRecoveryProbe(fixture!.page), {
      message: 'every visible terminal card must show its own full-card staged recovery state'
    }).toMatchObject({
      seenSessionIds: expect.arrayContaining(visibleSessionIds),
      fullAreaSessionIds: expect.arrayContaining(visibleSessionIds),
      maxVisible: visibleSessionIds.length
    })
    const cardProbe = await readCardRecoveryProbe(fixture.page)
    for (const sessionId of visibleSessionIds) {
      expect(cardProbe.phaseBySession[sessionId]?.length).toBeGreaterThan(0)
      expect(cardProbe.phaseBySession[sessionId]?.every((phase) =>
        phase === 'queued' || phase === 'restoring')).toBe(true)
    }

    const after = await waitForRecoveredRuntime(fixture, before.runtimePid)
    expect(fixture.app.process().pid).toBe(appPid)
    expect(fixture.page.isClosed()).toBe(false)
    await expect.poll(() => runtimeUtilityPids(appPid)).toEqual([after.runtimePid])
    await expect.poll(() => processExists(before.runtimePid)).toBe(false)
    assertRecoveryScheduling(after)

    const previousBySession = new Map(before.ptySessions.map(({ sessionId, pid }) => [sessionId, pid]))
    expect(after.ptyCount).toBe(20)
    expect(new Set(after.ptySessions.map(({ pid }) => pid)).size).toBe(20)
    expect(new Set(after.ptySessions.map(({ sessionId }) => sessionId)))
      .toEqual(new Set(RECOVERY_SCALE_SESSION_IDS))
    for (const { sessionId, pid } of after.ptySessions) {
      expect(pid).not.toBe(previousBySession.get(sessionId))
    }
    await expect.poll(() => previousPids.filter(processExists), {
      message: 'the killed Runtime generation must leave no previous PTY process alive'
    }).toEqual([])

    await verifyDurableHistory(fixture.dataDirectory)
    await verifyHistoryAndInput(fixture, RECOVERY_SCALE_FOREGROUND_IDS, true)
    await openBackgroundRecoveryTask(fixture.page)
    await expect(activeCarousel(fixture.page)).toHaveAttribute('data-total-sessions', '4')
    await verifyHistoryAndInput(fixture, RECOVERY_SCALE_BACKGROUND_IDS, false)
    await assertVisibleWindowsOnSecondaryColorLcd(fixture)
  } finally {
    await fixture?.close()
  }
})

function recoveryScaleEnvironment(): Record<string, string> {
  return { MATOU_E2E_SCALE: '1', MATOU_E2E_TERMINAL_DIAGNOSTICS: '0' }
}

async function waitForRecoveredRuntime(
  fixture: MatouFixture,
  replacedRuntimePid: number | undefined
): Promise<RuntimeMetrics> {
  let result: RuntimeMetrics | undefined
  await expect.poll(async () => {
    const metrics = await readRuntimeMetrics(fixture).catch(() => undefined)
    const readyIds = new Set(metrics?.recoveryObservation?.transitions
      .filter(({ state }) => state === 'ready').map(({ sessionId }) => sessionId))
    if (
      !metrics ||
      metrics.runtimePid === replacedRuntimePid ||
      metrics.ptyCount !== RECOVERY_SCALE_SESSION_IDS.length ||
      !RECOVERY_SCALE_SESSION_IDS.every((sessionId) => readyIds.has(sessionId))
    ) return false
    result = metrics
    return true
  }, { timeout: 60_000, message: 'all twenty durable Sessions must own a restored real PTY' }).toBe(true)
  return result!
}

async function readRuntimeMetrics(fixture: MatouFixture): Promise<RuntimeMetrics> {
  return fixture.app.evaluate(async () => {
    const read = (globalThis as typeof globalThis & {
      __matouE2eScaleMetrics?: () => Promise<RuntimeMetrics>
    }).__matouE2eScaleMetrics
    if (!read) throw new Error('Runtime scale metrics bridge is unavailable')
    return read()
  })
}

function assertRecoveryScheduling(metrics: RuntimeMetrics): void {
  const observation = metrics.recoveryObservation
  expect(observation).toBeDefined()
  expect(observation!.maxRestoring).toBe(4)
  expect(observation!.transitions.every(({ restoringCount }) => restoringCount <= 4)).toBe(true)
  const restoring = observation!.transitions.filter(({ state }) => state === 'restoring')
  expect(restoring).toHaveLength(20)
  expect(observation!.transitions.some(({ state }) => state === 'failed')).toBe(false)
  expect(restoring.map(({ sessionId }) => sessionId)[0]).toBe(RECOVERY_SCALE_ACTIVE_SESSION_ID)
  expect(restoring.slice(0, 4).every(({ sessionId }) =>
    RECOVERY_SCALE_FOREGROUND_IDS.includes(sessionId))).toBe(true)

  const firstBackground = restoring.findIndex(({ sessionId }) =>
    RECOVERY_SCALE_BACKGROUND_IDS.includes(sessionId))
  expect(firstBackground).toBe(RECOVERY_SCALE_FOREGROUND_IDS.length)
  expect(restoring.slice(firstBackground).every(({ sessionId }) =>
    RECOVERY_SCALE_BACKGROUND_IDS.includes(sessionId))).toBe(true)
  const ready = new Set(observation!.transitions
    .filter(({ state }) => state === 'ready').map(({ sessionId }) => sessionId))
  expect(RECOVERY_SCALE_BACKGROUND_IDS.every((sessionId) => ready.has(sessionId))).toBe(true)
  expect(RECOVERY_SCALE_SESSION_IDS.every((sessionId) => ready.has(sessionId))).toBe(true)
}

async function writeHistoryBeforeCrash(fixture: MatouFixture): Promise<void> {
  await openBackgroundRecoveryTask(fixture.page)
  await writeHistory(fixture, RECOVERY_SCALE_BACKGROUND_IDS)
  await openForegroundRecoveryTask(fixture.page)
  await expect(activeCarousel(fixture.page)).toHaveAttribute('data-total-sessions', '16')
  await writeHistory(fixture, RECOVERY_SCALE_FOREGROUND_IDS)
  const activeCard = fixture.page.locator(
    `[data-session-card="${RECOVERY_SCALE_ACTIVE_SESSION_ID}"]`
  )
  await activeCard.scrollIntoViewIfNeeded()
  await activeCard.click({ position: { x: 12, y: 12 } })
  await expect(surfaceFor(fixture.page, RECOVERY_SCALE_ACTIVE_SESSION_ID)
    .locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]'))
    .toHaveAttribute('data-active', 'true')
}

async function writeHistory(fixture: MatouFixture, sessionIds: string[]): Promise<void> {
  for (const sessionId of sessionIds) {
    const card = fixture.page.locator(`[data-session-card="${sessionId}"]`)
    await card.scrollIntoViewIfNeeded()
    await card.click({ position: { x: 12, y: 12 } })
    const surface = surfaceFor(fixture.page, sessionId)
    await terminalCommand(surface, `printf "${historyMarker(sessionId)}\\n"`)
    await expect(surface.locator('.xterm-rows')).toContainText(historyMarker(sessionId))
  }
}

async function verifyDurableHistory(dataDirectory: string): Promise<void> {
  for (const sessionId of RECOVERY_SCALE_SESSION_IDS) {
    const output = (await readSessionFrames(dataDirectory, sessionId)).flatMap((frame) =>
      frame.kind === 'output' ? [new TextDecoder().decode(frame.data)] : []).join('')
    expect(output).toContain(historyMarker(sessionId))
  }
}

async function verifyHistoryAndInput(
  fixture: MatouFixture,
  sessionIds: string[],
  expectCachedHistory: boolean
): Promise<void> {
  for (const sessionId of sessionIds) {
    const card = fixture.page.locator(`[data-session-card="${sessionId}"]`)
    await card.scrollIntoViewIfNeeded()
    await card.click({ position: { x: 12, y: 12 } })
    const surface = surfaceFor(fixture.page, sessionId)
    await expect(surface).toBeVisible()
    if (expectCachedHistory) {
      await expect(surface.locator('.xterm-rows')).toContainText(historyMarker(sessionId))
    }
    const afterMarker = `MATOU_INPUT_${String(
      RECOVERY_SCALE_SESSION_IDS.indexOf(sessionId) + 1
    ).padStart(2, '0')}`
    await terminalCommand(surface, `printf "${afterMarker}\\n"`)
    await expect(surface.locator('.xterm-rows')).toContainText(afterMarker)
  }
}

async function openBackgroundRecoveryTask(page: Page): Promise<void> {
  const workspace = page.locator(`[data-workspace-id="${RECOVERY_SCALE_BACKGROUND_WORKSPACE_ID}"]`)
  const task = page.getByTestId(`task-${RECOVERY_SCALE_BACKGROUND_TASK_ID}`)
  if (!await task.isVisible()) await workspace.locator('.workspace-group__toggle').click()
  await task.click()
  await expect(page.getByTestId('active-task')).toHaveText('Scale Task 2.1')
}

async function openForegroundRecoveryTask(page: Page): Promise<void> {
  const workspace = page.locator('[data-workspace-id="scale-workspace"]')
  const task = page.getByTestId('task-scale-task')
  if (!await task.isVisible()) await workspace.locator('.workspace-group__toggle').click()
  await task.click()
  await expect(page.getByTestId('active-task')).toHaveText('Scale Task')
}

function surfaceFor(page: Page, sessionId: string): Locator {
  return page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
}

function activeCarousel(page: Page): Locator {
  return page.locator('.scene-stage:not([hidden]) .session-carousel')
}

async function visibleCardSessionIds(page: Page): Promise<string[]> {
  return page.locator('.scene-stage:not([hidden]) [data-in-viewport="true"] [data-testid="terminal-pane"]')
    .evaluateAll((panes) => panes.flatMap((pane) => {
      const sessionId = pane.querySelector<HTMLElement>('.terminal-surface')?.dataset.sessionId
      return sessionId ? [sessionId] : []
    }))
}

async function installCardRecoveryProbe(page: Page, sessionIds: string[]): Promise<void> {
  await page.evaluate((ids) => {
    const targets = ids.map((sessionId) => ({
      sessionId,
      pane: document.querySelector(`.terminal-surface[data-session-id="${sessionId}"]`)
        ?.closest<HTMLElement>('[data-testid="terminal-pane"]')
    }))
    if (targets.some(({ pane }) => !pane)) throw new Error('A visible terminal pane is missing')
    const result = {
      seenSessionIds: [] as string[],
      fullAreaSessionIds: [] as string[],
      phaseBySession: {} as Record<string, string[]>,
      maxVisible: 0
    }
    const scan = () => {
      let visible = 0
      for (const { sessionId, pane } of targets) {
        const overlay = pane?.querySelector<HTMLElement>('.session-recovery-overlay')
        if (!pane || !overlay) continue
        visible += 1
        if (!result.seenSessionIds.includes(sessionId)) result.seenSessionIds.push(sessionId)
        const phase = overlay.textContent?.includes('等待恢复终端') ? 'queued' :
          overlay.textContent?.includes('正在恢复终端') ? 'restoring' : 'unknown'
        const phases = result.phaseBySession[sessionId] ??= []
        if (!phases.includes(phase)) phases.push(phase)
        const paneRect = pane.getBoundingClientRect()
        const overlayRect = overlay.getBoundingClientRect()
        const style = getComputedStyle(overlay)
        const fullArea = style.position === 'absolute' &&
          Math.abs(paneRect.left - overlayRect.left) <= 1 &&
          Math.abs(paneRect.top - overlayRect.top) <= 1 &&
          Math.abs(paneRect.right - overlayRect.right) <= 1 &&
          Math.abs(paneRect.bottom - overlayRect.bottom) <= 1 &&
          pane.getAttribute('aria-busy') === 'true'
        if (fullArea && !result.fullAreaSessionIds.includes(sessionId)) {
          result.fullAreaSessionIds.push(sessionId)
        }
      }
      result.maxVisible = Math.max(result.maxVisible, visible)
    }
    new MutationObserver(scan).observe(document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ['aria-busy']
    })
    ;(window as typeof window & { __matouRecoveryScaleProbe?: typeof result })
      .__matouRecoveryScaleProbe = result
    scan()
  }, sessionIds)
}

async function readCardRecoveryProbe(page: Page) {
  return page.evaluate(() => (
    window as typeof window & {
      __matouRecoveryScaleProbe?: {
        seenSessionIds: string[]
        fullAreaSessionIds: string[]
        phaseBySession: Record<string, string[]>
        maxVisible: number
      }
    }
  ).__matouRecoveryScaleProbe ?? {
    seenSessionIds: [], fullAreaSessionIds: [], phaseBySession: {}, maxVisible: 0
  })
}

async function assertVisibleWindowsOnSecondaryColorLcd(fixture: MatouFixture): Promise<void> {
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    const colorLcd = screen.getAllDisplays().filter(({ id, internal, label }) =>
      id !== primary.id && (internal || /color\s*lcd|内建视网膜显示器/i.test(label)))
    const visibleWindows = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
    const displays = visibleWindows.map((window) => screen.getDisplayMatching(window.getBounds()))
    return {
      primaryLabel: primary.label,
      colorLcdCount: colorLcd.length,
      visibleWindowCount: visibleWindows.length,
      allWindowsOnColorLcd: displays.every(({ id }) => colorLcd.some((display) => display.id === id)),
      windowsOnPrimary: displays.filter(({ id }) => id === primary.id).length
    }
  })).toEqual({
    primaryLabel: 'XV272U',
    colorLcdCount: 1,
    visibleWindowCount: 1,
    allWindowsOnColorLcd: true,
    windowsOnPrimary: 0
  })
}

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
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
