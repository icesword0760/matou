import { describe, expect, it, vi } from 'vitest'

import { RuntimeSessionRegistry } from './runtime-session-registry'

describe('RuntimeSessionRegistry', () => {
  it('waits for every PTY journal to close during a graceful Runtime shutdown', async () => {
    const registry = new RuntimeSessionRegistry()
    const first = session('first')
    const second = session('second')
    registry.set(first.value)
    registry.set(second.value)

    let complete = false
    const shutdown = registry.shutdownAll().then(() => { complete = true })

    expect(first.dispose).toHaveBeenCalledWith({
      notifyExit: false,
      reason: 'runtime-shutdown'
    })
    expect(second.dispose).toHaveBeenCalledWith({
      notifyExit: false,
      reason: 'runtime-shutdown'
    })
    expect(complete).toBe(false)

    first.resolve()
    await Promise.resolve()
    expect(complete).toBe(false)
    second.resolve()
    await shutdown
    expect(complete).toBe(true)
    expect(registry.has('first')).toBe(false)
    expect(registry.has('second')).toBe(false)
  })
})

function session(sessionId: string) {
  let resolve: () => void = () => {}
  const closed = new Promise<void>((done) => { resolve = done })
  const dispose = vi.fn()
  return {
    value: {
      sessionId,
      dispose,
      whenClosed: () => closed
    } as never,
    dispose,
    resolve
  }
}
