import { describe, expect, it, vi } from 'vitest'

import { RuntimeSessionRegistry } from './runtime-session-registry'

describe('RuntimeSessionRegistry', () => {
  it('tracks a launch digest only for its current run', () => {
    const registry = new RuntimeSessionRegistry()
    registry.set(session('digest').value)
    registry.recordProviderLaunchDigest('digest', 'stale', 'ignored')
    expect(registry.providerLaunchDigest('digest')).toBeUndefined()
    registry.recordProviderLaunchDigest('digest', 'run-digest', 'current')
    expect(registry.providerLaunchDigest('digest')).toBe('current')
    registry.delete('digest')
    expect(registry.providerLaunchDigest('digest')).toBeUndefined()
  })

  it('retries pending provider activation with bounded concurrency and keeps newer work', async () => {
    const registry = new RuntimeSessionRegistry()
    let release!: () => void
    const gate = new Promise<void>(resolve => { release = resolve })
    let active = 0
    let maximum = 0
    const attempts: string[] = []
    for (let index = 0; index < 6; index++) {
      registry.queueProviderActivation(String(index), async () => {
        active++
        maximum = Math.max(maximum, active)
        await gate
        active--
        attempts.push(String(index))
        return true
      })
    }
    const draining = registry.drainProviderActivations()
    await Promise.resolve()
    expect(maximum).toBe(4)
    await registry.drainProviderActivations()
    expect(maximum).toBe(4)
    release()
    await draining
    expect(attempts).toHaveLength(6)
    let retry = 0
    registry.queueProviderActivation('pending', async () => ++retry > 1)
    await registry.drainProviderActivations()
    await registry.drainProviderActivations()
    expect(retry).toBe(2)
    await registry.drainProviderActivations()
    expect(retry).toBe(2)
    registry.clearProviderActivations()
  })

  it('does not retry cancelled provider transitions or lose replacements queued while a retry runs', async () => {
    const registry = new RuntimeSessionRegistry()
    const removed = vi.fn(async () => true)
    registry.queueProviderActivation('gone', removed)
    registry.cancelProviderActivation('gone')
    await registry.drainProviderActivations()
    expect(removed).not.toHaveBeenCalled()
    const newer = vi.fn(async () => true)
    registry.queueProviderActivation('same', async () => {
      registry.queueProviderActivation('same', newer)
      return true
    })
    await registry.drainProviderActivations()
    await registry.drainProviderActivations()
    expect(newer).toHaveBeenCalledOnce()
    registry.clearProviderActivations()
  })

  it('invalidates an in-flight activation when its Session or Runtime is disposed', async () => {
    const registry = new RuntimeSessionRegistry()
    const version = registry.providerActivationVersion('disposed')
    registry.queueProviderActivation('disposed', async () => false)
    registry.cancelProviderActivation('disposed')
    expect(registry.providerActivationVersion('disposed')).not.toBe(version)
    const afterCancel = registry.providerActivationVersion('disposed')
    registry.clearProviderActivations()
    expect(registry.providerActivationVersion('disposed')).not.toBe(afterCancel)
  })

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

  it('records identity confirmation for the current fresh run without a pending output gate', () => {
    const registry = new RuntimeSessionRegistry()
    const fresh = session('fresh-provider')
    registry.set(fresh.value)

    expect(registry.providerIdentityPending('fresh-provider')).toBe(false)
    expect(registry.confirmProviderIdentity('fresh-provider', 'stale-run')).toBe(false)
    expect(registry.confirmProviderIdentity('fresh-provider', 'run-fresh-provider')).toBe(true)
    expect(registry.providerIdentityConfirmed('run-fresh-provider')).toBe(true)
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
      runId: `run-${sessionId}`,
      pid: 101,
      shutdownForRuntime,
      whenClosed: () => closed
    } as never,
    shutdownForRuntime,
    resolve
  }
}
