import { describe, expect, it } from 'vitest'

import { recoveryWaterTimeline } from './SessionRecoveryWater'

describe('recovery water timeline', () => {
  it('keeps the water visible and moving instead of ending before terminal content is ready', () => {
    const firstPass = recoveryWaterTimeline(7_000, false)
    const stillWaiting = recoveryWaterTimeline(12_000, false)

    expect(firstPass.alpha).toBe(1)
    expect(stillWaiting.alpha).toBe(1)
    expect(stillWaiting.rise).toBeGreaterThan(firstPass.rise)
    expect(stillWaiting.rise).toBeLessThan(1)
  })
})
