import { describe, expect, it } from 'vitest'

import { recoveryWaterTimeline } from './SessionRecoveryWater'

describe('recovery water timeline', () => {
  it('restarts the full fill motion while terminal content is still pending', () => {
    const opening = recoveryWaterTimeline(0, false)
    const filled = recoveryWaterTimeline(7_000, false)
    const replayedOpening = recoveryWaterTimeline(7_200, false)

    expect(filled.alpha).toBe(1)
    expect(filled.rise).toBeGreaterThan(.85)
    expect(replayedOpening).toEqual(opening)
  })
})
