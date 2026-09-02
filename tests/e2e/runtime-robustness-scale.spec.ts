import { expect, test } from '@playwright/test'

import { createRuntimeStressFixture } from './fixtures/runtime-stress-fixture'

test.describe('real terminal robustness scale gate', () => {
  test.describe.configure({ mode: 'serial', timeout: 4 * 60_000 })

  test('keeps twenty foreground terminals responsive during sustained real PTY output', async () => {
    const stress = await createRuntimeStressFixture(20)
    try {
      await stress.startOutput(1024 * 1024, 30)
      const samples = []
      for (let index = 1; index <= 3; index += 1) {
        const sample = await stress.sample(`runtime-stress-${index}`, 600)
        samples.push(sample)
        expect(sample.p95).toBeLessThan(32)
        expect(sample.longTaskMax).toBeLessThan(100)
        expect(sample.eventLoopDelayP99Ms).toBeLessThan(50)
        expect(sample.eventLoopDelayMaxMs).toBeLessThan(200)
        expect(sample.maxUnackedBytes).toBeLessThanOrEqual(2 * 1024 * 1024)
        expect(sample.ptyCount).toBe(20)
      }

      const first = samples[0]!
      const last = samples.at(-1)!
      expect(last.runtimeRssMb - first.runtimeRssMb).toBeLessThan(256)
      expect(last.rendererRssMb - first.rendererRssMb).toBeLessThan(512)
      await expect(stress.surface(stress.sessionIds[0]!).locator('.xterm-rows'))
        .toContainText(`MATOU_STRESS_DONE_${stress.sessionIds[0]}`, { timeout: 20_000 })
      await expect(stress.fixture.page.locator('.session-carousel'))
        .toHaveAttribute('data-foreground-terminals', '20')
    } finally {
      await stress.close()
    }
  })
})
