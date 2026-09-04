import { describe, expect, it, vi } from 'vitest'

import { ForegroundTerminalModelCache } from './terminal-model-cache'

describe('ForegroundTerminalModelCache', () => {
  it('reuses the same xterm model after a foreground card leaves and re-enters virtualized DOM', () => {
    const cache = new ForegroundTerminalModelCache<{ dispose(): void }>()
    const dispose = vi.fn()
    const create = vi.fn(() => ({ dispose }))
    cache.setForegroundSessions(['session-1'])

    const first = cache.acquire('session-1', create)
    cache.release('session-1')
    const restored = cache.acquire('session-1', create)

    expect(restored).toBe(first)
    expect(create).toHaveBeenCalledTimes(1)
    expect(dispose).not.toHaveBeenCalled()
  })

  it('waits for a mounted surface to release its model when projection changes foreground level', () => {
    const cache = new ForegroundTerminalModelCache<{ dispose(): void }>()
    const firstDispose = vi.fn()
    const secondDispose = vi.fn()
    cache.setForegroundSessions(['session-1', 'session-2'])
    cache.acquire('session-1', () => ({ dispose: firstDispose }))
    cache.acquire('session-2', () => ({ dispose: secondDispose }))

    cache.setForegroundSessions(['session-2'])

    expect(firstDispose).not.toHaveBeenCalled()
    expect(secondDispose).not.toHaveBeenCalled()
    expect(cache.size).toBe(2)

    expect(cache.release('session-1')).toBe(false)
    expect(firstDispose).toHaveBeenCalledTimes(1)
    expect(cache.size).toBe(1)
  })

  it('disposes an unmounted retained model when its level leaves the foreground', () => {
    const cache = new ForegroundTerminalModelCache<{ dispose(): void }>()
    const dispose = vi.fn()
    cache.setForegroundSessions(['session-1'])
    cache.acquire('session-1', () => ({ dispose }))

    expect(cache.release('session-1')).toBe(true)
    cache.setForegroundSessions([])

    expect(dispose).toHaveBeenCalledTimes(1)
    expect(cache.size).toBe(0)
  })
})
