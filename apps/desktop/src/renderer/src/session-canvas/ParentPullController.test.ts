import { describe, expect, it } from 'vitest'

import {
  ParentPullController,
  parentPullThreshold,
  stepParentReturnSpring
} from './ParentPullController'

describe('ParentPullController', () => {
  it('uses one oversized gesture only to reach and arm the left edge', () => {
    const controller = new ParentPullController()

    expect(controller.begin({ scrollLeft: 640, hasParent: true })).toBe('scrolling')
    expect(controller.move({ deltaTowardParent: 900, viewportWidth: 800 })).toMatchObject({ consume: false, pullDistance: 0 })
    expect(controller.end({ scrollLeft: 0, viewportWidth: 800 })).toMatchObject({ commit: false, edgeArmed: true })
  })

  it('springs back when a fresh edge gesture stays below the threshold', () => {
    const controller = new ParentPullController()
    expect(controller.begin({ scrollLeft: 0, hasParent: true })).toBe('pulling')
    const movement = controller.move({ deltaTowardParent: 80, viewportWidth: 800, timeMs: 100 })
    expect(movement.consume).toBe(true)
    expect(movement.pullDistance).toBeGreaterThan(0)

    const nextMovement = controller.move({ deltaTowardParent: 30, viewportWidth: 800, timeMs: 116 })
    const result = controller.end({ scrollLeft: 0, viewportWidth: 800, timeMs: 120 })
    expect(result).toMatchObject({
      commit: false,
      edgeArmed: true,
      springBack: true,
      pullDistance: nextMovement.pullDistance
    })
    expect(result.releaseVelocity).toBeGreaterThan(0)
  })

  it('hands an edge gesture moving away from the parent back to list scrolling', () => {
    const controller = new ParentPullController()
    expect(controller.begin({ scrollLeft: 0, hasParent: true })).toBe('pulling')

    expect(controller.move({ deltaTowardParent: -48, viewportWidth: 800 })).toEqual({
      consume: false,
      pullDistance: 0,
      progress: 0,
      effectIntensity: 0
    })
    expect(controller.snapshot().phase).toBe('scrolling')
  })

  it('commits the parent after a fresh edge gesture crosses the clamped threshold', () => {
    const controller = new ParentPullController()
    controller.begin({ scrollLeft: 0, hasParent: true })
    controller.move({ deltaTowardParent: 420, viewportWidth: 800 })

    expect(controller.snapshot().progress).toBe(1)
    expect(controller.end({ scrollLeft: 0, viewportWidth: 800 })).toMatchObject({ commit: true })
  })

  it('commits a deliberate trackpad pull using raw gesture travel rather than resisted pixels', () => {
    const controller = new ParentPullController()
    controller.begin({ scrollLeft: 0, hasParent: true })

    for (const deltaTowardParent of [78, 82, 80, 80]) {
      controller.move({ deltaTowardParent, viewportWidth: 800 })
    }

    expect(controller.snapshot().progress).toBe(1)
    expect(controller.end({ scrollLeft: 0, viewportWidth: 800 })).toMatchObject({ commit: true })
  })

  it('locks the ready projection in place until the gesture is released', () => {
    const controller = new ParentPullController()
    controller.begin({ scrollLeft: 0, hasParent: true })

    const notYetReady = controller.move({ deltaTowardParent: 300, viewportWidth: 800 })
    const ready = controller.move({ deltaTowardParent: 20, viewportWidth: 800 })
    const held = controller.move({ deltaTowardParent: 260, viewportWidth: 800 })

    expect(notYetReady.progress).toBeLessThan(1)
    expect(ready.progress).toBe(1)
    expect(held.progress).toBe(1)
    expect(ready.pullDistance).toBe(parentPullThreshold(800))
    expect(held.pullDistance).toBe(ready.pullDistance)
    expect(controller.snapshot().phase).toBe('pulling')
  })

  it('keeps a newly armed release readable before committing the parent', () => {
    const controller = new ParentPullController()
    controller.begin({ scrollLeft: 0, hasParent: true, timeMs: 0 })
    controller.move({ deltaTowardParent: 320, viewportWidth: 800, timeMs: 100 })

    expect(controller.end({ scrollLeft: 0, viewportWidth: 800, timeMs: 150 })).toMatchObject({
      commit: true,
      commitDelayMs: 400
    })
  })

  it('reports stronger visual energy for a faster pull at the same distance', () => {
    const slow = new ParentPullController()
    slow.begin({ scrollLeft: 0, hasParent: true, timeMs: 0 })
    const slowMovement = slow.move({ deltaTowardParent: 70, viewportWidth: 800, timeMs: 140 })
    const fast = new ParentPullController()
    fast.begin({ scrollLeft: 0, hasParent: true, timeMs: 0 })
    const fastMovement = fast.move({ deltaTowardParent: 70, viewportWidth: 800, timeMs: 24 })

    expect(fastMovement.effectIntensity).toBeGreaterThan(slowMovement.effectIntensity)
    expect(fastMovement.effectIntensity).toBeLessThanOrEqual(1)
  })

  it('applies resistance without a parent and ignores vertical motion', () => {
    const controller = new ParentPullController()
    expect(controller.begin({ scrollLeft: 0, hasParent: false })).toBe('scrolling')
    expect(controller.move({ deltaTowardParent: 300, viewportWidth: 800, verticalDominant: true }))
      .toEqual({ consume: false, pullDistance: 0, progress: 0, effectIntensity: 0 })
    expect(controller.end({ scrollLeft: 0, viewportWidth: 800 }).commit).toBe(false)
  })

  it('requires enough direct travel to expose the complete parent preview', () => {
    expect(parentPullThreshold(300)).toBe(284)
    expect(parentPullThreshold(600)).toBe(284)
    expect(parentPullThreshold(800)).toBe(320)
    expect(parentPullThreshold(1200)).toBe(340)
  })

  it('returns with a velocity-preserving underdamped physical spring', () => {
    let state = { position: 110, velocity: 540 }
    const withoutReleaseVelocity = stepParentReturnSpring({ position: 110, velocity: 0 }, 1 / 60)
    const positions: number[] = []
    for (let frame = 0; frame < 90; frame += 1) {
      state = stepParentReturnSpring(state, 1 / 60)
      positions.push(state.position)
    }

    expect(positions[0]).toBeGreaterThan(withoutReleaseVelocity.position)
    expect(Math.min(...positions)).toBeLessThan(0)
    expect(Math.abs(state.position)).toBeLessThan(0.5)
    expect(Math.abs(state.velocity)).toBeLessThan(1)
  })
})
