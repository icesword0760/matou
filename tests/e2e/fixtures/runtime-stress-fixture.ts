import { Buffer } from 'node:buffer'
import { join } from 'node:path'

import { expect, type Locator } from '@playwright/test'

import {
  launchMatou, restartMatou, stopMatouPreservingData, type MatouFixture
} from '../matou-fixture'
import { seedScaleDatabase } from '../scale/scale-database'
import { collectScaleSample, type ScaleSample } from '../scale/scale-metrics'
import { RuntimeDatabase } from '../../../apps/runtime/src/storage/database'

export interface RuntimeStressFixture {
  fixture: MatouFixture
  sessionIds: string[]
  surface(sessionId: string): Locator
  startOutput(bytesPerSecond: number, durationSeconds: number): Promise<void>
  sample(name: string, frameCount: number): Promise<ScaleSample>
  close(): Promise<void>
}

export async function createRuntimeStressFixture(sessionCount = 20): Promise<RuntimeStressFixture> {
  const env = {
    MATOU_E2E_SCALE: '1',
    // The channel smoke terminal is useful for protocol E2E tests but would
    // add a twenty-first PTY/xterm to this product workload benchmark.
    MATOU_E2E_TERMINAL_DIAGNOSTICS: '0'
  }
  let fixture = await launchMatou({ env })
  await expect(fixture.page.locator('.hierarchy-shell')).toBeVisible()
  await stopMatouPreservingData(fixture)
  await seedScaleDatabase(fixture.dataDirectory, { siblingSessions: sessionCount })
  const database = RuntimeDatabase.open(join(fixture.dataDirectory, 'matou.sqlite'))
  try {
    database.run(
      "UPDATE sessions SET work_status = 'running' WHERE id GLOB 'scale-sibling-*'"
    )
  } finally {
    database.close()
  }
  fixture = await restartMatou(fixture, { env })
  await expect(fixture.page.locator('.hierarchy-shell')).toBeVisible()
  await expect(fixture.page.locator('.session-carousel'))
    .toHaveAttribute('data-total-sessions', String(sessionCount))
  await expect(fixture.page.locator('.session-carousel'))
    .toHaveAttribute('data-foreground-terminals', String(sessionCount))

  const sessionIds = Array.from(
    { length: sessionCount },
    (_, index) => `scale-sibling-${String(index + 1).padStart(5, '0')}`
  )
  const surface = (sessionId: string) => fixture.page.locator(
    `.terminal-surface[data-session-id="${sessionId}"]`
  )

  return {
    get fixture() { return fixture },
    sessionIds,
    surface,
    async startOutput(bytesPerSecond, durationSeconds) {
      if (!Number.isSafeInteger(bytesPerSecond) || bytesPerSecond < 1) {
        throw new Error('bytesPerSecond must be a positive integer')
      }
      if (!Number.isFinite(durationSeconds) || durationSeconds <= 0) {
        throw new Error('durationSeconds must be positive')
      }
      const startAtSeconds = Date.now() / 1000 + Math.max(5, sessionCount / 2)
      for (const sessionId of sessionIds) {
        const textarea = surface(sessionId).locator('.xterm-helper-textarea')
        await expect(textarea).toBeAttached()
        await textarea.click()
        await fixture.page.keyboard.insertText(stressCommand({
          sessionId, bytesPerSecond, durationSeconds, startAtSeconds
        }))
        await textarea.press('Enter')
      }
      await fixture.page.waitForTimeout(Math.max(0, startAtSeconds * 1000 - Date.now()) + 250)
    },
    sample: (name, frameCount) => collectScaleSample(fixture, {
      name, minimumFrameCount: frameCount, warmupRuns: 0, measuredRuns: 1
    }),
    close: () => fixture.close()
  }
}

function stressCommand(input: {
  sessionId: string
  bytesPerSecond: number
  durationSeconds: number
  startAtSeconds: number
}): string {
  const script = [
    'import sys,time',
    `start=${input.startAtSeconds}`,
    `duration=${input.durationSeconds}`,
    `rate=${input.bytesPerSecond}`,
    `prefix=${JSON.stringify(`${input.sessionId}:`)}`,
    'time.sleep(max(0,start-time.time()))',
    'payload=(prefix+("x"*max(1,1023-len(prefix)))+"\\n").encode()',
    'interval=len(payload)/rate',
    'deadline=start+duration',
    'while time.time()<deadline:',
    ' sys.stdout.buffer.write(payload)',
    ' sys.stdout.buffer.flush()',
    ' time.sleep(interval)',
    `print(${JSON.stringify(`MATOU_STRESS_DONE_${input.sessionId}`)},flush=True)`
  ].join('\n')
  const encoded = Buffer.from(script).toString('base64')
  return `python3 -u -c "import base64;exec(base64.b64decode('${encoded}'))"`
}
