import { afterEach, describe, expect, it, vi } from 'vitest'

import { ProviderReadyRegistry } from './provider-ready-registry'

describe('ProviderReadyRegistry', () => {
  afterEach(() => vi.useRealTimers())

  it('resolves only a waiter for the matching Session with the recorded run identity', async () => {
    const ready = new ProviderReadyRegistry()
    let settled = false
    const pending = ready.wait('session-target', 1_000).then((identity) => {
      settled = true
      return identity
    })

    ready.record('session-other', 'run-other')
    await Promise.resolve()
    expect(settled).toBe(false)

    ready.record('session-target', 'run-target')
    await expect(pending).resolves.toEqual({ sessionId: 'session-target', runId: 'run-target' })
  })

  it('ignores an identity recorded before a fresh waiter is registered', async () => {
    const ready = new ProviderReadyRegistry()
    ready.record('session-1', 'run-old')

    let settled = false
    const pending = ready.wait('session-1', 1_000).then((identity) => {
      settled = true
      return identity
    })
    await Promise.resolve()
    expect(settled).toBe(false)

    ready.record('session-1', 'run-new')
    await expect(pending).resolves.toEqual({ sessionId: 'session-1', runId: 'run-new' })
  })

  it('keeps waiting when a stale Run reports after registration', async () => {
    const ready = new ProviderReadyRegistry()
    let settled = false
    const pending = ready.wait('session-1', 1_000).then((identity) => {
      settled = true
      return identity
    })

    ready.record('session-1', 'run-old', 'run-new')
    await Promise.resolve()
    expect(settled).toBe(false)

    ready.record('session-1', 'run-new', 'run-new')
    await expect(pending).resolves.toEqual({ sessionId: 'session-1', runId: 'run-new' })
  })

  it('removes a timed-out waiter without consuming a later identity', async () => {
    vi.useFakeTimers()
    const ready = new ProviderReadyRegistry()
    const timedOut = expect(ready.wait('session-1', 25)).rejects.toThrow(
      '等待会话 session-1 的 Provider 就绪超时'
    )

    await vi.advanceTimersByTimeAsync(25)
    await timedOut

    const next = ready.wait('session-1', 25)
    ready.record('session-1', 'run-next')
    await expect(next).resolves.toEqual({ sessionId: 'session-1', runId: 'run-next' })
  })

  it('actively cancels one waiter and all remaining waiters', async () => {
    const ready = new ProviderReadyRegistry()
    const first = ready.wait('session-1', 1_000)
    const second = ready.wait('session-2', 1_000)

    ready.cancel('session-1', new Error('startup failed'))
    await expect(first).rejects.toThrow('startup failed')
    expect(ready.pendingWaiterCount).toBe(1)

    ready.cancelAll(new Error('runtime shutdown'))
    await expect(second).rejects.toThrow('runtime shutdown')
    expect(ready.pendingWaiterCount).toBe(0)
  })
})
