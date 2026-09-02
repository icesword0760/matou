import { expect, test, type Page } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { performance } from 'node:perf_hooks'

import {
  expectVisibleWindowsOnPrimaryDisplay,
  launchMatou,
  primaryAcceptanceDisplayRequested,
  restartMatou,
  stopMatouPreservingData,
  type MatouFixture
} from '../matou-fixture'
import { readScaleDatabaseCounts, seedScaleDatabase } from './scale-database'

const DAG_NODE_COUNT = 10_000
const FIRST_OPERABLE_BUDGET_MS = 300
const SEARCH_BUDGET_MS = 100
const AGGREGATE_FOCUS_BUDGET_MS = 300
const FRAME_P95_BUDGET_MS = 16.7
const INPUT_RESPONSE_P95_BUDGET_MS = 34
const LONG_TASK_P95_BUDGET_MS = 100
const MAX_RENDERED_ITEMS = 400
const MAX_RENDERED_EDGES = 800
const MAX_TOTAL_DOM_NODES = 2_500
const INTERACTION_FRAMES = 180

interface InteractionSample {
  frameDurations: number[]
  responseDurations: number[]
  longTasks: number[]
  longTaskSupported: boolean
  nodeCount: number
  aggregateCount: number
  edgeCount: number
  domCount: number
  finalPan: string
  finalScale: string
}

test.describe('real Electron 10,000-node DAG acceptance', () => {
  test.describe.configure({ mode: 'serial', timeout: 5 * 60_000 })

  test('meets navigation, interaction, DOM, long-task, and display budgets from real SQLite authority', async () => {
    test.skip(process.platform !== 'darwin', 'the accepted dual-display names are macOS-specific')
    if (!primaryAcceptanceDisplayRequested()) {
      const systemDisplays = execFileSync('system_profiler', ['SPDisplaysDataType'], { encoding: 'utf8' })
      expect(systemDisplays).toContain('XV272U:')
      expect(systemDisplays).toContain('Color LCD:')
    }
    let fixture: MatouFixture | undefined
    try {
      fixture = await launchMatou({ env: { MATOU_E2E_SCALE: '1' } })
      await expect(fixture.page.locator('.hierarchy-shell')).toBeVisible()
      await expectVisibleWindowsOnColorLcd(fixture, 1)
      await stopMatouPreservingData(fixture)

      await seedScaleDatabase(fixture.dataDirectory, {
        siblingSessions: 20,
        dagNodes: DAG_NODE_COUNT,
        scenes: 2
      })
      expect(await readScaleDatabaseCounts(fixture.dataDirectory)).toMatchObject({
        dagNodes: DAG_NODE_COUNT,
        dagRelations: DAG_NODE_COUNT - 1,
        scenes: 2
      })
      fixture = await restartMatou(fixture, { env: { MATOU_E2E_SCALE: '1' } })
      await expect(fixture.page.locator('.hierarchy-shell')).toBeVisible()
      await expectVisibleWindowsOnColorLcd(fixture, 1)
      await fixture.page.getByRole('tab', { name: 'Scale DAG 10000' }).click()
      await expect(fixture.page.getByRole('tab', { name: 'Scale DAG 10000' }))
        .toHaveAttribute('aria-selected', 'true')

      const firstOpenedAt = performance.now()
      const dagWindowPromise = fixture.app.waitForEvent('window')
      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      const dag = await dagWindowPromise
      await expect(dag.getByRole('application', { name: '会话 DAG 画布' })).toBeVisible()
      const observedOpenMs = performance.now() - firstOpenedAt
      const firstOperableMs = Number(await dag.locator('.dag-window')
        .getAttribute('data-first-operable-ms'))
      console.log(`[dag-10000-cold-open] ${JSON.stringify({
        firstOperableMs: round(firstOperableMs), observedOpenMs: round(observedOpenMs)
      })}`)
      expect(firstOperableMs).toBeLessThan(FIRST_OPERABLE_BUDGET_MS)
      await expectVisibleWindowsOnColorLcd(fixture, 2)

      const canvas = dag.getByRole('application', { name: '会话 DAG 画布' })
      const initialAggregate = dag.locator('.dag-aggregate-card').first()
      await expect(initialAggregate).toBeVisible()
      const previousFocusedSessionId = await dag.locator('.dag-node-card.is-focused')
        .getAttribute('data-session-id')
      const previousPan = await canvas.getAttribute('data-pan')
      const aggregateStartedAt = performance.now()
      await initialAggregate.click()
      await expect.poll(() => canvas.getAttribute('data-pan')).not.toBe(previousPan)
      await expect.poll(() => dag.locator('.dag-node-card.is-focused').getAttribute('data-session-id'))
        .not.toBe(previousFocusedSessionId)
      const aggregateFocusMs = performance.now() - aggregateStartedAt
      expect(aggregateFocusMs).toBeLessThan(AGGREGATE_FOCUS_BUDGET_MS)
      await expectVisibleWindowsOnColorLcd(fixture, 2)

      const interaction = await collectDagInteractionSample(dag)
      const frameP95 = percentile(interaction.frameDurations, .95)
      const responseP95 = percentile(interaction.responseDurations, .95)
      const longTaskP95 = percentile(interaction.longTasks, .95)
      console.log(`[dag-10000-interaction] ${JSON.stringify({
        firstOperableMs: round(firstOperableMs),
        aggregateFocusMs: round(aggregateFocusMs),
        frameP95,
        responseP95,
        longTaskP95,
        longTaskMax: maximum(interaction.longTasks),
        frameCount: interaction.frameDurations.length,
        responseCount: interaction.responseDurations.length,
        longTaskCount: interaction.longTasks.length,
        longTaskSupported: interaction.longTaskSupported,
        nodeCount: interaction.nodeCount,
        aggregateCount: interaction.aggregateCount,
        edgeCount: interaction.edgeCount,
        domCount: interaction.domCount,
        finalPan: interaction.finalPan,
        finalScale: interaction.finalScale
      })}`)
      expect(frameP95).toBeLessThanOrEqual(FRAME_P95_BUDGET_MS)
      expect(responseP95).toBeLessThan(INPUT_RESPONSE_P95_BUDGET_MS)
      expect(interaction.longTaskSupported).toBe(true)
      expect(longTaskP95).toBeLessThan(LONG_TASK_P95_BUDGET_MS)
      expect(interaction.nodeCount + interaction.aggregateCount).toBeLessThanOrEqual(MAX_RENDERED_ITEMS)
      expect(interaction.edgeCount).toBeLessThanOrEqual(MAX_RENDERED_EDGES)
      expect(interaction.domCount).toBeLessThanOrEqual(MAX_TOTAL_DOM_NODES)
      expect(interaction.finalPan).not.toBe(previousPan)
      expect(Number(interaction.finalScale)).not.toBe(1)
      await expectVisibleWindowsOnColorLcd(fixture, 2)

      const searchStartedAt = performance.now()
      await dag.getByRole('searchbox', { name: '搜索会话' }).fill('dag 10000')
      const remoteResult = dag.getByRole('option', { name: /^dag 10000/ })
      await expect(remoteResult).toBeVisible()
      const searchMs = performance.now() - searchStartedAt
      expect(searchMs).toBeLessThan(SEARCH_BUDGET_MS)
      await remoteResult.click()

      await expect.poll(async () => (await fixture!.app.windows()).length).toBe(1)
      await expect(fixture.page.getByRole('tab', { name: 'Scale DAG 10000' }))
        .toHaveAttribute('aria-selected', 'true')
      await expect(fixture.page.locator('[aria-current="true"][aria-label="会话：dag 10000"]'))
        .toBeInViewport()
      await expectVisibleWindowsOnColorLcd(fixture, 1)
      console.log(`[dag-10000-navigation] ${JSON.stringify({
        firstOperableMs: round(firstOperableMs),
        aggregateFocusMs: round(aggregateFocusMs),
        searchMs: round(searchMs),
        selectedSessionId: 'scale-dag-10000'
      })}`)
    } finally {
      await fixture?.close()
    }
  })
})

async function collectDagInteractionSample(dag: Page): Promise<InteractionSample> {
  return dag.evaluate(async ({ frameCount }) => {
    const canvas = document.querySelector<HTMLElement>('.dag-canvas')
    if (!canvas) throw new Error('DAG canvas is unavailable')
    const longTasks: number[] = []
    const longTaskSupported = typeof PerformanceObserver !== 'undefined' &&
      PerformanceObserver.supportedEntryTypes.includes('longtask')
    const observer = typeof PerformanceObserver === 'undefined'
      ? undefined
      : new PerformanceObserver((entries) => {
          for (const entry of entries.getEntries()) longTasks.push(entry.duration)
        })
    try {
      observer?.observe({ type: 'longtask', buffered: true })
    } catch {
      observer?.disconnect()
    }

    const frameDurations: number[] = []
    const responseDurations: number[] = []
    let previousTimestamp: number | undefined
    let previousSignature = `${canvas.dataset.pan}|${canvas.dataset.scale}`
    let pendingAt: number | undefined
    for (let index = 0; index < frameCount; index += 1) {
      const timestamp = await new Promise<number>((resolve) => requestAnimationFrame(resolve))
      if (previousTimestamp !== undefined) frameDurations.push(timestamp - previousTimestamp)
      previousTimestamp = timestamp
      const signature = `${canvas.dataset.pan}|${canvas.dataset.scale}`
      if (pendingAt !== undefined && signature !== previousSignature) {
        responseDurations.push(performance.now() - pendingAt)
        pendingAt = undefined
      }
      previousSignature = signature

      const zoom = index % 6 === 5
      canvas.dispatchEvent(new WheelEvent('wheel', {
        bubbles: true,
        cancelable: true,
        clientX: canvas.clientWidth / 2,
        clientY: canvas.clientHeight / 2,
        deltaX: zoom ? 0 : (index % 12 < 6 ? 13 : -11),
        deltaY: zoom ? (index % 12 < 6 ? -9 : 7) : (index % 16 < 8 ? 5 : -4),
        ctrlKey: zoom
      }))
      if (pendingAt === undefined) pendingAt = performance.now()
    }
    for (let index = 0; pendingAt !== undefined && index < 4; index += 1) {
      await new Promise<number>((resolve) => requestAnimationFrame(resolve))
      const signature = `${canvas.dataset.pan}|${canvas.dataset.scale}`
      if (signature !== previousSignature) {
        responseDurations.push(performance.now() - pendingAt)
        pendingAt = undefined
      }
      previousSignature = signature
    }
    observer?.takeRecords().forEach((entry) => longTasks.push(entry.duration))
    observer?.disconnect()
    return {
      frameDurations,
      responseDurations,
      longTasks,
      longTaskSupported,
      nodeCount: document.querySelectorAll('.dag-node-card').length,
      aggregateCount: document.querySelectorAll('.dag-aggregate-card').length,
      edgeCount: document.querySelectorAll('.dag-edge').length,
      domCount: document.getElementsByTagName('*').length,
      finalPan: canvas.dataset.pan ?? '',
      finalScale: canvas.dataset.scale ?? ''
    }
  }, { frameCount: INTERACTION_FRAMES })
}

async function expectVisibleWindowsOnColorLcd(fixture: MatouFixture, count: number): Promise<void> {
  if (primaryAcceptanceDisplayRequested()) {
    await expectVisibleWindowsOnPrimaryDisplay(fixture, count)
    return
  }
  await expect.poll(async () => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    return {
      primaryLabel: primary.label,
      visibleWindows: BrowserWindow.getAllWindows().filter((window) => window.isVisible()).map((window) => {
        const display = screen.getDisplayMatching(window.getBounds())
        return {
          label: display.label,
          internal: display.internal,
          primary: display.id === primary.id,
          bounds: window.getBounds(),
          workArea: display.workArea
        }
      })
    }
  }), { timeout: 15_000 }).toMatchObject({
    primaryLabel: 'XV272U',
    visibleWindows: Array.from({ length: count }, () => ({ internal: true, primary: false }))
  })

  const placement = await fixture.app.evaluate(({ BrowserWindow, screen }) => ({
    primaryLabel: screen.getPrimaryDisplay().label,
    windows: BrowserWindow.getAllWindows().filter((window) => window.isVisible()).map((window) => {
      const bounds = window.getBounds()
      const display = screen.getDisplayMatching(bounds)
      return {
        label: display.label,
        internal: display.internal,
        primary: display.id === screen.getPrimaryDisplay().id,
        bounds,
        workArea: display.workArea
      }
    })
  }))
  expect(placement.primaryLabel).toBe('XV272U')
  expect(placement.windows).toHaveLength(count)
  for (const window of placement.windows) {
    expect(window.internal).toBe(true)
    expect(window.primary).toBe(false)
    expect(window.bounds.x).toBeGreaterThanOrEqual(window.workArea.x)
    expect(window.bounds.y).toBeGreaterThanOrEqual(window.workArea.y)
    expect(window.bounds.x + window.bounds.width)
      .toBeLessThanOrEqual(window.workArea.x + window.workArea.width)
    expect(window.bounds.y + window.bounds.height)
      .toBeLessThanOrEqual(window.workArea.y + window.workArea.height)
  }
}

function percentile(values: number[], quantile: number): number {
  if (values.length === 0) return 0
  const ordered = [...values].sort((left, right) => left - right)
  return round(ordered[Math.min(ordered.length - 1, Math.ceil(ordered.length * quantile) - 1)]!)
}

function maximum(values: number[]): number {
  return values.length === 0 ? 0 : round(Math.max(...values))
}

function round(value: number): number {
  return Math.round(value * 100) / 100
}
