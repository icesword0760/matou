import { describe, expect, it, vi } from 'vitest'

import { ForegroundTerminalModelCache } from './terminal-model-cache'

describe('ForegroundTerminalModelCache', () => {
  it('reports whether a Session can reuse an existing xterm model', () => {
    const cache = new ForegroundTerminalModelCache<{ dispose(): void }>()

    expect(cache.has('session-1')).toBe(false)
    cache.acquire('session-1', () => ({ dispose: vi.fn() }))
    expect(cache.has('session-1')).toBe(true)
  })

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

  it('keeps the previous level warm until the bounded cache needs its slot', () => {
    const cache = new ForegroundTerminalModelCache<{ dispose(): void }>(2)
    const firstDispose = vi.fn()
    const secondDispose = vi.fn()
    const thirdDispose = vi.fn()
    cache.setForegroundSessions(['session-1'])
    cache.acquire('session-1', () => ({ dispose: firstDispose }))
    expect(cache.release('session-1')).toBe(true)

    cache.setForegroundSessions(['session-2'])
    cache.acquire('session-2', () => ({ dispose: secondDispose }))
    expect(cache.release('session-2')).toBe(true)

    expect(firstDispose).not.toHaveBeenCalled()
    expect(secondDispose).not.toHaveBeenCalled()
    expect(cache.size).toBe(2)

    cache.setForegroundSessions(['session-3'])
    cache.acquire('session-3', () => ({ dispose: thirdDispose }))

    expect(firstDispose).toHaveBeenCalledTimes(1)
    expect(secondDispose).not.toHaveBeenCalled()
    expect(cache.size).toBe(2)
  })

  it('uses recent access rather than insertion order when evicting a warm model', () => {
    const cache = new ForegroundTerminalModelCache<{ dispose(): void }>(2)
    const firstDispose = vi.fn()
    const secondDispose = vi.fn()
    cache.acquire('session-1', () => ({ dispose: firstDispose }))
    cache.release('session-1')
    cache.acquire('session-2', () => ({ dispose: secondDispose }))
    cache.release('session-2')

    cache.acquire('session-1', () => ({ dispose: firstDispose }))
    cache.release('session-1')
    cache.acquire('session-3', () => ({ dispose: vi.fn() }))

    expect(firstDispose).not.toHaveBeenCalled()
    expect(secondDispose).toHaveBeenCalledTimes(1)
  })

  it('never evicts a mounted visible model to make room for a warm model', () => {
    const cache = new ForegroundTerminalModelCache<{ dispose(): void }>(1)
    const visibleDispose = vi.fn()
    const warmDispose = vi.fn()
    cache.acquire('visible', () => ({ dispose: visibleDispose }))
    cache.acquire('warm', () => ({ dispose: warmDispose }))
    cache.release('warm')

    expect(visibleDispose).not.toHaveBeenCalled()
    expect(warmDispose).toHaveBeenCalledTimes(1)
    expect(cache.size).toBe(1)
  })
})
