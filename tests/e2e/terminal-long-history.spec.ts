import { Buffer } from 'node:buffer'
import { execFile } from 'node:child_process'
import { readdir, stat, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { expect, test, type Locator, type Page } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'
import { collectScaleSample, type ScaleSample } from './scale/scale-metrics'
import {
  terminalCommand, visibleSurfaces, waitForShell
} from './fixtures/session-canvas-fixture'

const MIB = 1024 * 1024
const execFileAsync = promisify(execFile)
const E2E_ENV = {
  MATOU_E2E_SCALE: '1',
  MATOU_E2E_TERMINAL_DIAGNOSTICS: '0',
  // Production remains 16 MiB segments with a 256 MiB raw hot window. The
  // E2E-only scale factor makes the identical rotate/compress path repeatable.
  MATOU_E2E_JOURNAL_MAX_SEGMENT_BYTES: String(512 * 1024),
  MATOU_E2E_JOURNAL_RAW_HOT_BYTES: String(1024 * 1024)
}

test.describe('real PTY long terminal history gate', () => {
  test.setTimeout(180_000)

  test('keeps compressed history searchable, responsive, and session-isolated after restart', async () => {
    await assertAcceptanceDisplays()
    let fixture: MatouFixture = await launchMatou({ env: E2E_ENV })
    const metrics: Record<string, number> = {}
    try {
      await expectOnlyColorLcdWindows(fixture)
      await expect(fixture.page.getByTestId('active-task')).toHaveText('默认')
      const first = visibleSurfaces(fixture.page).first()
      await waitForShell(first)
      const sessionA = await requiredAttribute(first, 'data-session-id')

      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(visibleSurfaces(fixture.page)).toHaveCount(2)
      const second = visibleSurfaces(fixture.page).last()
      await waitForShell(second)
      const sessionB = await requiredAttribute(second, 'data-session-id')

      await fixture.page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
      await expect(fixture.page.getByTestId('active-task')).toHaveText('新事项')
      const otherTaskSurface = visibleSurfaces(fixture.page).first()
      await waitForShell(otherTaskSurface)
      const otherTaskSession = await requiredAttribute(otherTaskSurface, 'data-session-id')
      await activateTask(fixture.page, '默认')

      const stableA = surface(fixture.page, sessionA)
      const stableB = surface(fixture.page, sessionB)
      await stableA.click({ position: { x: 12, y: 12 } })
      const before = await collectScaleSample(fixture, {
        name: 'long-history-before-output', minimumFrameCount: 90,
        warmupRuns: 0, measuredRuns: 1
      })

      const outputStartedAt = performance.now()
      await terminalCommand(stableA, sustainedOutputCommand({
        marker: 'MATOU_COLD_ARCHIVE_MARKER',
        done: 'MATOU_LONG_HISTORY_DONE',
        bytes: 8 * MIB,
        durationMs: 20_000
      }))

      const inputStartedAt = performance.now()
      await terminalCommand(stableB, encodedPrintCommand('B_RESPONSIVE_DURING_LONG_OUTPUT'))
      await expect(stableB.locator('.xterm-rows')).toContainText('B_RESPONSIVE_DURING_LONG_OUTPUT')
      metrics.inputEchoDuringOutputMs = round(performance.now() - inputStartedAt)

      await activateTask(fixture.page, '新事项')
      const stableOtherTask = surface(fixture.page, otherTaskSession)
      await expect(stableOtherTask).toHaveAttribute('data-pid', /[1-9][0-9]*/)
      await activateTask(fixture.page, '默认')
      await stableA.click({ position: { x: 12, y: 12 } })

      const underLoad = await collectScaleSample(fixture, {
        name: 'long-history-under-output', minimumFrameCount: 180,
        warmupRuns: 0, measuredRuns: 1
      })
      assertRuntimeBudgets(before, underLoad)
      await expect(stableA.locator('.xterm-rows')).toContainText('MATOU_LONG_HISTORY_DONE', {
        timeout: 30_000
      })
      metrics.outputDurationMs = round(performance.now() - outputStartedAt)

      const journalDirectory = join(fixture.dataDirectory, 'journal', sessionA)
      const compressed = await waitForCompressedSegments(journalDirectory)
      metrics.compressedSegments = compressed.length
      metrics.journalBytes = await journalBytes(journalDirectory)

      metrics.compressedSearchMs = await searchArchivedHistory(
        fixture.page, 'MATOU_COLD_ARCHIVE_MARKER', 0
      )
      await closeHistorySearch(fixture.page)

      // Prove that a clean compressed archive survives a full Runtime restart
      // and is replayed/searchable before introducing the isolated fault.
      await activateSession(stableA)
      const cleanRestartStartedAt = performance.now()
      fixture = await restartMatou(fixture, { env: E2E_ENV })
      await expectOnlyColorLcdWindows(fixture)
      const cleanA = surface(fixture.page, sessionA)
      await waitForShell(cleanA)
      await expect(cleanA.locator('.xterm-rows')).toContainText('MATOU_LONG_HISTORY_DONE')
      metrics.firstScreenRecoveryMs = round(performance.now() - cleanRestartStartedAt)

      const restoredInputStartedAt = performance.now()
      await terminalCommand(cleanA, encodedPrintCommand('MATOU_POST_RESTART_INPUT_ECHO'))
      await expect(cleanA.locator('.xterm-rows')).toContainText('MATOU_POST_RESTART_INPUT_ECHO')
      metrics.postRestartInputEchoMs = round(performance.now() - restoredInputStartedAt)
      metrics.postRestartCompressedSearchMs = await searchArchivedHistory(
        fixture.page, 'MATOU_COLD_ARCHIVE_MARKER', 0
      )
      await closeHistorySearch(fixture.page)

      const damagedArchive = join(journalDirectory, compressed[0]!)
      await writeFile(damagedArchive, 'isolated corrupt compressed journal')
      await terminalCommand(cleanA, sustainedOutputCommand({
        marker: 'MATOU_HEALTHY_AFTER_BAD_ARCHIVE',
        done: 'MATOU_POST_CORRUPTION_DONE',
        bytes: 8 * MIB,
        durationMs: 20_000
      }))
      await expect(cleanA.locator('.xterm-rows')).toContainText('MATOU_POST_CORRUPTION_DONE', {
        timeout: 30_000
      })
      metrics.isolatedSearchMs = await searchArchivedHistory(
        fixture.page, 'MATOU_HEALTHY_AFTER_BAD_ARCHIVE', 1
      )
      await closeHistorySearch(fixture.page)

      const cleanB = surface(fixture.page, sessionB)
      await activateSession(cleanB)
      await terminalCommand(cleanB, encodedPrintCommand('B_RESPONSIVE_AFTER_BAD_ARCHIVE'))
      await expect(cleanB.locator('.xterm-rows')).toContainText('B_RESPONSIVE_AFTER_BAD_ARCHIVE')
      await searchVisibleHistory(fixture.page, 'B_RESPONSIVE_AFTER_BAD_ARCHIVE')
      await closeHistorySearch(fixture.page)

      // Keep the healthy sibling focused for the corrupt restart. The damaged
      // session is expected to be the sole recovery unit held back.
      await activateSession(cleanB)
      const isolatedRestartStartedAt = performance.now()
      fixture = await restartMatou(fixture, { env: E2E_ENV })
      await expectOnlyColorLcdWindows(fixture)
      const isolatedB = surface(fixture.page, sessionB)
      await waitForShell(isolatedB)
      metrics.healthySessionRecoveryMs = round(performance.now() - isolatedRestartStartedAt)
      await expect(isolatedB.locator('.xterm-rows')).toContainText('B_RESPONSIVE_AFTER_BAD_ARCHIVE')

      const isolatedInputStartedAt = performance.now()
      await terminalCommand(isolatedB, encodedPrintCommand('B_INPUT_AFTER_ISOLATED_RESTART'))
      await expect(isolatedB.locator('.xterm-rows')).toContainText('B_INPUT_AFTER_ISOLATED_RESTART')
      metrics.healthySessionInputEchoMs = round(performance.now() - isolatedInputStartedAt)
      await searchVisibleHistory(fixture.page, 'B_RESPONSIVE_AFTER_BAD_ARCHIVE')
      await closeHistorySearch(fixture.page)

      const isolatedA = sessionCard(fixture.page, sessionA)
      await expect(isolatedA.locator('.terminal-surface'))
        .not.toHaveAttribute('data-pid', /[1-9][0-9]*/)
      await expect(isolatedA.locator('.session-start-failure-card')).toContainText('会话启动失败')

      await activateTask(fixture.page, '新事项')
      await waitForShell(surface(fixture.page, otherTaskSession))
      await activateTask(fixture.page, '默认')
      await waitForShell(surface(fixture.page, sessionB))

      metrics.runtimeRssGrowthMb = round(underLoad.runtimeRssMb - before.runtimeRssMb)
      metrics.rendererRssGrowthMb = round(underLoad.rendererRssMb - before.rendererRssMb)
      metrics.frameP50Ms = underLoad.p50
      metrics.frameP95Ms = underLoad.p95
      metrics.frameMaxMs = underLoad.max
      metrics.longTaskP95Ms = underLoad.longTaskP95
      metrics.longTaskMaxMs = underLoad.longTaskMax
      metrics.eventLoopDelayP99Ms = underLoad.eventLoopDelayP99Ms
      metrics.eventLoopDelayMaxMs = underLoad.eventLoopDelayMaxMs
      console.log(`[long-history-baseline] ${JSON.stringify(metrics)}`)
    } finally {
      await fixture.close()
    }
  })
})

function assertRuntimeBudgets(before: ScaleSample, underLoad: ScaleSample): void {
  expect(underLoad.runtimeRssMb - before.runtimeRssMb).toBeLessThan(256)
  expect(underLoad.rendererRssMb - before.rendererRssMb).toBeLessThan(512)
  expect(underLoad.p95).toBeLessThan(32)
  expect(underLoad.longTaskP95).toBeLessThan(100)
  expect(underLoad.longTaskMax).toBeLessThan(100)
  expect(underLoad.eventLoopDelayP99Ms).toBeLessThan(50)
  expect(underLoad.eventLoopDelayMaxMs).toBeLessThan(200)
  expect(underLoad.maxUnackedBytes).toBeLessThanOrEqual(2 * MIB)
  expect(underLoad.retainedDurabilityBytes).toBeLessThanOrEqual(4 * MIB)
}

async function searchArchivedHistory(page: Page, query: string, expectedGaps: number): Promise<number> {
  const startedAt = performance.now()
  await page.keyboard.press(`${process.platform === 'darwin' ? 'Meta' : 'Control'}+f`)
  const input = page.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' })
  await input.fill(query)
  const history = page.getByRole('region', { name: '终端历史记录' })
  await expect(history).toBeVisible({ timeout: 15_000 })
  await expect(history.locator('[data-current-match="true"]')).toContainText(query)
  if (expectedGaps > 0) await expect(history).toContainText(`${expectedGaps} 处历史缺口`)
  else await expect(history).not.toContainText('历史缺口')
  return round(performance.now() - startedAt)
}

async function searchVisibleHistory(page: Page, query: string): Promise<void> {
  await page.keyboard.press(`${process.platform === 'darwin' ? 'Meta' : 'Control'}+f`)
  await page.getByRole('textbox', { name: '搜索当前 Tab 的终端内容' }).fill(query)
  await expect(page.locator('.terminal-search-bar__count')).toHaveText('1/1')
}

async function closeHistorySearch(page: Page): Promise<void> {
  const returnToTerminal = page.getByRole('button', { name: '返回实时终端' })
  if (await returnToTerminal.isVisible().catch(() => false)) await returnToTerminal.click()
  await page.getByRole('button', { name: '关闭搜索' }).click()
}

async function activateTask(page: Page, title: string): Promise<void> {
  const item = page.locator('.workbench-item').filter({
    has: page.locator('.workbench-item__name', { hasText: title })
  }).first()
  await item.click()
  await expect(page.getByTestId('active-task')).toHaveText(title)
}

function surface(page: Page, sessionId: string): Locator {
  return page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
}

function sessionCard(page: Page, sessionId: string): Locator {
  return page.locator(`.session-card-slot[data-session-id="${sessionId}"]`)
}

async function activateSession(target: Locator): Promise<void> {
  await target.click({ position: { x: 12, y: 12 } })
  await expect(target.locator(
    'xpath=ancestor::div[contains(concat(" ", normalize-space(@class), " "), " session-card-slot ")][1]'
  )).toHaveClass(/is-focused/)
}

async function waitForCompressedSegments(directory: string): Promise<string[]> {
  let compressed: string[] = []
  await expect.poll(async () => {
    compressed = (await readdir(directory)).filter((name) => name.endsWith('.mtj.gz')).sort()
    return compressed.length
  }, { timeout: 30_000 }).toBeGreaterThan(0)
  return compressed
}

async function journalBytes(directory: string): Promise<number> {
  const entries = (await readdir(directory)).filter((name) => /\.mtj(?:\.gz)?$/.test(name))
  const sizes = await Promise.all(entries.map((name) => stat(join(directory, name))))
  return sizes.reduce((total, item) => total + item.size, 0)
}

function sustainedOutputCommand(input: {
  marker: string
  done: string
  bytes: number
  durationMs: number
}): string {
  const script = [
    'import sys,time',
    `marker=${JSON.stringify(input.marker)}`,
    `done=${JSON.stringify(input.done)}`,
    `target=${input.bytes}`,
    `duration=${input.durationMs / 1000}`,
    // Keep the writer continuously busy without creating a thousand IPC
    // deliveries per second; each chunk contains ordinary terminal lines.
    'line=(("H"*127)+"\\n").encode()',
    'payload=line*64',
    'chunk=payload',
    'written=0',
    'started=time.time()',
    'print(marker,flush=True)',
    'while written<target:',
    ' sys.stdout.buffer.write(chunk)',
    ' sys.stdout.buffer.flush()',
    ' written+=len(chunk)',
    ' expected=started+(written/target)*duration',
    ' time.sleep(max(0,expected-time.time()))',
    'print(done,flush=True)'
  ].join('\n')
  return encodedPythonCommand(script)
}

function encodedPrintCommand(value: string): string {
  return encodedPythonCommand(`print(${JSON.stringify(value)},flush=True)`)
}

function encodedPythonCommand(script: string): string {
  return `python3 -u -c "import base64;exec(base64.b64decode('${Buffer.from(script).toString('base64')}'))"`
}

async function expectOnlyColorLcdWindows(fixture: MatouFixture): Promise<void> {
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    const visible = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
    const placements = visible.map((window) => screen.getDisplayMatching(window.getBounds()))
    return {
      visible: visible.length,
      // Electron exposes the built-in panel through the stable `internal`
      // identity while system_profiler (used by launchMatou's preflight) names
      // the same acceptance display "Color LCD".
      colorLcd: placements.filter(({ internal }) => internal).length,
      primary: placements.filter(({ id }) => id === primary.id).length,
      xv272u: placements.filter(({ label }) => /xv272u/i.test(label)).length
    }
  })).toEqual({ visible: 1, colorLcd: 1, primary: 0, xv272u: 0 })
}

async function assertAcceptanceDisplays(): Promise<void> {
  if (process.platform !== 'darwin') {
    throw new Error('Visible Electron gate requires macOS display inventory before launch')
  }
  const { stdout } = await execFileAsync(
    '/usr/sbin/system_profiler', ['SPDisplaysDataType', '-json'],
    { maxBuffer: 4 * MIB }
  )
  const report = JSON.parse(stdout) as {
    SPDisplaysDataType?: Array<{ spdisplays_ndrvs?: Array<Record<string, unknown>> }>
  }
  const displays = report.SPDisplaysDataType?.flatMap(
    ({ spdisplays_ndrvs }) => spdisplays_ndrvs ?? []
  ) ?? []
  const colorLcd = displays.find((display) =>
    display._name === 'Color LCD' &&
    display.spdisplays_online === 'spdisplays_yes' &&
    display.spdisplays_main !== 'spdisplays_yes' &&
    typeof display.spdisplays_display_type === 'string' &&
    display.spdisplays_display_type.includes('built-in')
  )
  const primary = displays.find((display) =>
    display._name === 'XV272U' && display.spdisplays_main === 'spdisplays_yes'
  )
  if (!colorLcd || !primary) {
    throw new Error('Visible Electron gate requires secondary Color LCD and primary XV272U before launch')
  }
}

async function requiredAttribute(locator: Locator, name: string): Promise<string> {
  const value = await locator.getAttribute(name)
  if (!value) throw new Error(`${name} is missing`)
  return value
}

function round(value: number): number {
  return Math.round(value * 100) / 100
}
