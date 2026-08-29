// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from 'vitest'

import { DagShortcutController, clampDagHoldDuration } from './useDagShortcut'

afterEach(() => vi.useRealTimers())

describe('DagShortcutController', () => {
  it('forwards one terminal Tab when released at 449ms', () => {
    vi.useFakeTimers()
    const shortPress = vi.fn()
    const longPress = vi.fn()
    const controller = new DagShortcutController({ shortPress, longPress, holdDuration: 450 })

    controller.keyDown(key('Tab', true))
    vi.advanceTimersByTime(449)
    controller.keyUp(key('Tab', true))

    expect(shortPress).toHaveBeenCalledTimes(1)
    expect(longPress).not.toHaveBeenCalled()
  })

  it('opens once at 450ms, consumes Tab and ignores repeats', () => {
    vi.useFakeTimers()
    const shortPress = vi.fn()
    const longPress = vi.fn()
    const controller = new DagShortcutController({ shortPress, longPress, holdDuration: 450 })

    expect(controller.keyDown(key('Tab', true))).toBe(true)
    controller.keyDown({ ...key('Tab', true), repeat: true })
    vi.advanceTimersByTime(450)
    controller.keyUp(key('Tab', true))

    expect(longPress).toHaveBeenCalledTimes(1)
    expect(shortPress).not.toHaveBeenCalled()
  })

  it('clamps settings and clears pending state on blur or cancel', () => {
    expect(clampDagHoldDuration(100)).toBe(350)
    expect(clampDagHoldDuration(900)).toBe(800)
    vi.useFakeTimers()
    const shortPress = vi.fn()
    const longPress = vi.fn()
    const controller = new DagShortcutController({ shortPress, longPress, holdDuration: 500 })

    controller.keyDown(key('Tab', true))
    controller.cancel()
    vi.runAllTimers()
    controller.keyUp(key('Tab', true))

    expect(shortPress).not.toHaveBeenCalled()
    expect(longPress).not.toHaveBeenCalled()
  })
})

function key(value: string, altKey: boolean) {
  return { key: value, altKey, repeat: false, preventDefault: vi.fn(), stopPropagation: vi.fn() }
}
