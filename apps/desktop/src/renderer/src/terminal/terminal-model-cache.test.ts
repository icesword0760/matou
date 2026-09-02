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

  it('disposes cached xterm models when their sibling level leaves foreground', () => {
    const cache = new ForegroundTerminalModelCache<{ dispose(): void }>()
    const firstDispose = vi.fn()
    const secondDispose = vi.fn()
    cache.setForegroundSessions(['session-1', 'session-2'])
    cache.acquire('session-1', () => ({ dispose: firstDispose }))
    cache.acquire('session-2', () => ({ dispose: secondDispose }))

    cache.setForegroundSessions(['session-2'])

    expect(firstDispose).toHaveBeenCalledTimes(1)
    expect(secondDispose).not.toHaveBeenCalled()
    expect(cache.size).toBe(1)
  })
})
