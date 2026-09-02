import { describe, expect, it, vi } from 'vitest'

import { RuntimeSessionRegistry } from './runtime-session-registry'

describe('RuntimeSessionRegistry', () => {
  it('reports the Runtime-authoritative number of live PTYs', () => {
    const registry = new RuntimeSessionRegistry()
    registry.set(session('first').value)
    registry.set(session('second').value)

    expect(registry.size).toBe(2)
    expect(registry.pids()).toEqual([101, 101])
    expect(registry.sessionPids()).toEqual([
      { sessionId: 'first', pid: 101 },
      { sessionId: 'second', pid: 101 }
    ])
    registry.delete('first')
    expect(registry.size).toBe(1)
  })

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

  it('keeps provider identity confirmation and output ownership shared across connections', () => {
    const registry = new RuntimeSessionRegistry()
    const confirm = vi.fn()
    const reject = vi.fn()

    registry.markProviderIdentityPending('shared-provider', 'run-1', { confirm, reject })
    expect(registry.providerIdentityPending('shared-provider')).toBe(true)

    expect(registry.confirmProviderIdentity('shared-provider', 'stale-run')).toBe(false)
    expect(confirm).not.toHaveBeenCalled()
    expect(registry.confirmProviderIdentity('shared-provider', 'run-1')).toBe(true)
    expect(confirm).toHaveBeenCalledOnce()
    expect(reject).not.toHaveBeenCalled()
    expect(registry.providerIdentityPending('shared-provider')).toBe(false)
    expect(registry.providerIdentityConfirmed('run-1')).toBe(true)

    registry.forgetProviderIdentity('shared-provider', 'run-1')
    expect(registry.providerIdentityConfirmed('run-1')).toBe(false)
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
      pid: 101,
      shutdownForRuntime,
      whenClosed: () => closed
    } as never,
    shutdownForRuntime,
    resolve
  }
}
