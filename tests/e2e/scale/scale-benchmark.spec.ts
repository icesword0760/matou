import { expect, test } from '@playwright/test'

import {
  launchMatou,
  restartMatou,
  stopMatouPreservingData,
  type MatouFixture
} from '../matou-fixture'
import {
  readScaleDatabaseCounts,
  seedScaleDatabase,
  type ScaleDataset
} from './scale-database'
import { collectScaleSample } from './scale-metrics'
import { readSessionFrames } from '../../../apps/runtime/src/journal/segment-journal'

test.describe('real Electron scale benchmark', () => {
  test.describe.configure({ mode: 'serial', timeout: 10 * 60_000 })

  for (const siblingSessions of [50, 200, 1000] as const) {
    test(`harness starts the ${siblingSessions} Session dataset and records a complete warm sample`, async () => {
      const dataset: ScaleDataset = { siblingSessions }
      let fixture: MatouFixture | undefined
      let measuredProcessIds: number[] = []
      try {
        fixture = await launchMatou({ env: { MATOU_E2E_SCALE: '1' } })
        await expect(fixture.page.locator('.hierarchy-shell')).toBeVisible()
        await stopMatouPreservingData(fixture)

        await seedScaleDatabase(fixture.dataDirectory, dataset)
        const firstCounts = await readScaleDatabaseCounts(fixture.dataDirectory)
        await seedScaleDatabase(fixture.dataDirectory, dataset)
        expect(await readScaleDatabaseCounts(fixture.dataDirectory)).toEqual(firstCounts)
        expect(firstCounts.siblingSessions).toBe(siblingSessions)

        fixture = await restartMatou(fixture, { env: { MATOU_E2E_SCALE: '1' } })
        await expect(fixture.page.locator('.hierarchy-shell')).toBeVisible()
        await expect(fixture.page.locator('[data-session-card]')).toHaveCount(siblingSessions)

        const sample = await collectScaleSample(fixture, {
          name: `siblings-${siblingSessions}`,
          minimumFrameCount: 120,
          warmupRuns: 2,
          measuredRuns: 5
        })

        expect(sample).toMatchObject({
          name: `siblings-${siblingSessions}`,
          warmupRuns: 2,
          measuredRuns: 5,
          electronPid: expect.any(Number),
          rendererPid: expect.any(Number),
          runtimePid: expect.any(Number),
          rendererRssMb: expect.any(Number),
          runtimeRssMb: expect.any(Number),
          ptyCount: expect.any(Number),
          domNodes: expect.any(Number),
          statementCount: expect.any(Number),
          p50: expect.any(Number),
          p95: expect.any(Number),
          max: expect.any(Number)
        })
        expect(sample.count).toBeGreaterThanOrEqual(120 * 5)
        expect(sample.rendererPid).toBeGreaterThan(0)
        expect(sample.runtimePid).toBeGreaterThan(0)
        expect(sample.ptyCount).toBeGreaterThan(0)
        expect(sample.domNodes).toBeGreaterThan(0)
        expect(sample.statementCount).toBeGreaterThanOrEqual(0)
        measuredProcessIds = [
          sample.electronPid,
          sample.rendererPid,
          sample.runtimePid,
          ...sample.ptyPids
        ]
      } finally {
        await fixture?.close()
        await expect.poll(() => measuredProcessIds.filter(isProcessAlive), {
          timeout: 30_000,
          message: 'Electron, Runtime, Renderer, and PTY children must exit after the scale fixture closes'
        }).toEqual([])
      }
    })
  }

  test('seed contract creates the 5000-deep chain and 10000-node DAG deterministically', async () => {
    let fixture: MatouFixture | undefined
    try {
      fixture = await launchMatou({ env: { MATOU_E2E_SCALE: '1' } })
      await expect(fixture.page.locator('.hierarchy-shell')).toBeVisible()
      await stopMatouPreservingData(fixture)
      const dataset: ScaleDataset = {
        siblingSessions: 50,
        relationshipDepth: 5000,
        dagNodes: 10000,
        scenes: 3
      }

      await seedScaleDatabase(fixture.dataDirectory, dataset)
      const first = await readScaleDatabaseCounts(fixture.dataDirectory)
      await seedScaleDatabase(fixture.dataDirectory, dataset)
      const second = await readScaleDatabaseCounts(fixture.dataDirectory)

      expect(second).toEqual(first)
      expect(second).toMatchObject({
        siblingSessions: 50,
        relationshipDepth: 5000,
        depthRelations: 4999,
        dagNodes: 10000,
        dagRelations: 9999,
        scenes: 3
      })
    } finally {
      await fixture?.close()
    }
  })

  test('seed contract writes deterministic real Journal payloads when requested', async () => {
    let fixture: MatouFixture | undefined
    try {
      fixture = await launchMatou({ env: { MATOU_E2E_SCALE: '1' } })
      await expect(fixture.page.locator('.hierarchy-shell')).toBeVisible()
      await stopMatouPreservingData(fixture)
      const dataset: ScaleDataset = {
        siblingSessions: 50,
        journalBytesPerSession: 256
      }

      await seedScaleDatabase(fixture.dataDirectory, dataset)
      expect(await journalPayloadBytes(fixture.dataDirectory, 'scale-sibling-00001')).toBe(256)
      expect(await journalPayloadBytes(fixture.dataDirectory, 'scale-sibling-00050')).toBe(256)
      await seedScaleDatabase(fixture.dataDirectory, dataset)
      expect(await journalPayloadBytes(fixture.dataDirectory, 'scale-sibling-00001')).toBe(256)
    } finally {
      await fixture?.close()
    }
  })
})

function isProcessAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    return !(error instanceof Error && 'code' in error && error.code === 'ESRCH')
  }
}

async function journalPayloadBytes(dataDirectory: string, sessionId: string): Promise<number> {
  return (await readSessionFrames(dataDirectory, sessionId)).reduce(
    (total, frame) => total + (frame.kind === 'output' ? frame.data.byteLength : 0),
    0
  )
}
