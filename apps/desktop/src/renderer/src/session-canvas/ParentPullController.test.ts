import { describe, expect, it } from 'vitest'

import { ParentPullController, parentPullThreshold } from './ParentPullController'

describe('ParentPullController', () => {
  it('uses one oversized gesture only to reach and arm the left edge', () => {
    const controller = new ParentPullController()

    expect(controller.begin({ scrollLeft: 640, hasParent: true })).toBe('scrolling')
    expect(controller.move({ deltaTowardParent: 900, viewportWidth: 800 })).toMatchObject({ consume: false, pullDistance: 0 })
    expect(controller.end({ scrollLeft: 0, viewportWidth: 800 })).toMatchObject({ commit: false, edgeArmed: true })
  })

  it('springs back when a fresh edge gesture stays below the threshold', () => {
    const controller = armedController()
    expect(controller.begin({ scrollLeft: 0, hasParent: true })).toBe('pulling')
    const movement = controller.move({ deltaTowardParent: 80, viewportWidth: 800 })
    expect(movement.consume).toBe(true)
    expect(movement.pullDistance).toBeGreaterThan(0)

    expect(controller.end({ scrollLeft: 0, viewportWidth: 800 })).toMatchObject({
      commit: false, edgeArmed: true, springBack: true
    })
  })

  it('commits the parent after a fresh edge gesture crosses the clamped threshold', () => {
    const controller = armedController()
    controller.begin({ scrollLeft: 0, hasParent: true })
    controller.move({ deltaTowardParent: 420, viewportWidth: 800 })

    expect(controller.snapshot().pullDistance).toBeGreaterThanOrEqual(parentPullThreshold(800))
    expect(controller.end({ scrollLeft: 0, viewportWidth: 800 })).toMatchObject({ commit: true })
  })

  it('applies resistance without a parent and ignores vertical motion', () => {
    const controller = new ParentPullController()
    expect(controller.begin({ scrollLeft: 0, hasParent: false })).toBe('scrolling')
    expect(controller.move({ deltaTowardParent: 300, viewportWidth: 800, verticalDominant: true }))
      .toEqual({ consume: false, pullDistance: 0, progress: 0 })
    expect(controller.end({ scrollLeft: 0, viewportWidth: 800 }).commit).toBe(false)
  })

  it('uses a 22 percent threshold clamped between 96 and 180 pixels', () => {
    expect(parentPullThreshold(300)).toBe(96)
    expect(parentPullThreshold(600)).toBe(132)
    expect(parentPullThreshold(1200)).toBe(180)
  })
})

function armedController(): ParentPullController {
  const controller = new ParentPullController()
  controller.begin({ scrollLeft: 100, hasParent: true })
  controller.end({ scrollLeft: 0, viewportWidth: 800 })
  return controller
}
