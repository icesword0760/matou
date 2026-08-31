import { describe, expect, it, vi } from 'vitest'

import { RuntimeSessionRegistry } from './runtime-session-registry'

describe('RuntimeSessionRegistry', () => {
  it('serializes lifecycle operations for the same Session across Runtime connections', async () => {
    const registry = new RuntimeSessionRegistry()
    const events: string[] = []
    let releaseFirst: () => void = () => {}
    const firstGate = new Promise<void>((resolve) => { releaseFirst = resolve })
    let markFirstStarted: () => void = () => {}
    const firstStarted = new Promise<void>((resolve) => { markFirstStarted = resolve })

    const first = registry.runExclusive('shared-session', async () => {
      events.push('first:start')
      markFirstStarted()
      await firstGate
      events.push('first:end')
    })
    const second = registry.runExclusive('shared-session', async () => {
      events.push('second:start')
      events.push('second:end')
    })

    await firstStarted
    expect(events).toEqual(['first:start'])
    releaseFirst()
    await Promise.all([first, second])
    expect(events).toEqual(['first:start', 'first:end', 'second:start', 'second:end'])
  })

  it('does not serialize lifecycle operations for different Sessions', async () => {
    const registry = new RuntimeSessionRegistry()
    let releaseFirst: () => void = () => {}
    const firstGate = new Promise<void>((resolve) => { releaseFirst = resolve })
    let secondStarted = false

    const first = registry.runExclusive('first-session', () => firstGate)
    const second = registry.runExclusive('second-session', async () => {
      secondStarted = true
    })

    await second
    expect(secondStarted).toBe(true)
    releaseFirst()
    await first
  })

  it('waits for every PTY journal to close during a graceful Runtime shutdown', async () => {
    const registry = new RuntimeSessionRegistry()
    const first = session('first')
    const second = session('second')
    registry.set(first.value)
    registry.set(second.value)

    let complete = false
    const shutdown = registry.shutdownAll().then(() => { complete = true })

    expect(first.shutdownForRuntime).toHaveBeenCalledOnce()
    expect(second.shutdownForRuntime).toHaveBeenCalledOnce()
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
  const shutdownForRuntime = vi.fn(() => closed)
  return {
    value: {
      sessionId,
      shutdownForRuntime,
      whenClosed: () => closed
    } as never,
    shutdownForRuntime,
    resolve
  }
}
