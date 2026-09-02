import { afterEach, describe, expect, it, vi } from 'vitest'

import { VisibleHudRefreshLoop } from './visible-hud-refresh-loop'

afterEach(() => vi.useRealTimers())

describe('VisibleHudRefreshLoop', () => {
  it('refreshes immediately, every two seconds while visible, and again on focus', () => {
    vi.useFakeTimers()
    let visible = true
    const refresh = vi.fn()
    const loop = new VisibleHudRefreshLoop(refresh, () => visible)

    loop.start()
    expect(refresh).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(2_000)
    expect(refresh).toHaveBeenCalledTimes(2)

    visible = false
    vi.advanceTimersByTime(4_000)
    loop.focus()
    expect(refresh).toHaveBeenCalledTimes(2)

    visible = true
    loop.visibilityChanged()
    loop.focus()
    expect(refresh).toHaveBeenCalledTimes(4)
    loop.stop()
    vi.advanceTimersByTime(2_000)
    expect(refresh).toHaveBeenCalledTimes(4)
  })
})
