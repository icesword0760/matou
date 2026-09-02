import { describe, expect, it, vi } from 'vitest'

import {
  DurabilityBufferOverflowError,
  SessionDurabilityGate,
  type PendingDurableFrame
} from './session-durability-gate'

function storageError(code: 'ENOSPC' | 'EACCES'): NodeJS.ErrnoException {
  return Object.assign(new Error(code), { code })
}

function frame(
  sequence: number,
  bytes: number,
  persist: () => Promise<void>,
  afterPersist?: () => void
): PendingDurableFrame {
  return {
    sequence,
    kind: 'output',
    bytes: new Uint8Array(bytes),
    persist,
    ...(afterPersist === undefined ? {} : { afterPersist })
  }
}

describe('SessionDurabilityGate', () => {
  it('retains the failed frame and later frames, then retries without a sequence gap or duplicate', async () => {
    const persisted: number[] = []
    const sideEffects: number[] = []
    const states: string[] = []
    let failSequenceTwo = true
    const pauser = {
      pause: vi.fn(() => { states.push('paused') }),
      resume: vi.fn(() => { states.push('resumed') })
    }
    const faults: number[] = []
    const recovered: number[] = []
    const gate = new SessionDurabilityGate({
      sessionId: 'session-a',
      pauser,
      onFault: ({ failedSequence }) => { faults.push(failedSequence) },
      onRecovered: ({ throughSequence }) => { recovered.push(throughSequence) }
    })
    const makeFrame = (sequence: number) => frame(
      sequence,
      8,
      async () => {
        if (sequence === 2 && failSequenceTwo) {
          failSequenceTwo = false
          throw storageError('ENOSPC')
        }
        persisted.push(sequence)
      },
      () => { sideEffects.push(sequence) }
    )

    await gate.append(makeFrame(1))
    await gate.append(makeFrame(2))
    await gate.append(makeFrame(3))

    expect(gate.state).toBe('paused')
    expect(gate.retainedBytes).toBe(16)
    expect(faults).toEqual([2])
    expect(persisted).toEqual([1])
    expect(sideEffects).toEqual([1])
    expect(pauser.pause).toHaveBeenCalledTimes(1)

    await gate.retry()

    expect(gate.state).toBe('healthy')
    expect(gate.retainedBytes).toBe(0)
    expect(persisted).toEqual([1, 2, 3])
    expect(sideEffects).toEqual([1, 2, 3])
    expect(recovered).toEqual([3])
    expect(states).toEqual(['paused', 'resumed'])
  })

  it('pauses before admitting retained output and never exceeds its byte limit', async () => {
    const order: string[] = []
    const gate = new SessionDurabilityGate({
      sessionId: 'bounded',
      maxRetainedBytes: 4 * 1024 * 1024,
      pauser: {
        pause: () => { order.push('pause') },
        resume: () => { order.push('resume') }
      }
    })

    await gate.append(frame(1, 2 * 1024 * 1024, async () => {
      order.push('persist')
      throw storageError('ENOSPC')
    }))
    await gate.append(frame(2, 2 * 1024 * 1024, async () => {}))

    expect(order).toEqual(['pause', 'persist'])
    expect(gate.retainedBytes).toBe(4 * 1024 * 1024)
    expect(() => gate.append(frame(3, 1, async () => {}))).toThrow(DurabilityBufferOverflowError)
    expect(gate.retainedBytes).toBe(4 * 1024 * 1024)
    expect(gate.lastAcceptedSequence).toBe(2)
  })

  it('backpressures a healthy producer before the retained FIFO fills and resumes after durable drain', async () => {
    let releaseFirstWrite: () => void = () => {}
    const firstWriteReleased = new Promise<void>((resolve) => { releaseFirstWrite = resolve })
    const persisted: number[] = []
    const pause = vi.fn()
    const resume = vi.fn()
    const gate = new SessionDurabilityGate({
      sessionId: 'healthy-backpressure',
      pauser: { pause, resume }
    })
    const persist = (sequence: number) => async () => {
      if (sequence === 1) await firstWriteReleased
      persisted.push(sequence)
    }

    const writes = [
      gate.append(frame(1, 1024 * 1024, persist(1))),
      gate.append(frame(2, 1024 * 1024, persist(2))),
      gate.append(frame(3, 1024 * 1024, persist(3)))
    ]

    expect(gate.state).toBe('healthy')
    expect(gate.retainedBytes).toBe(3 * 1024 * 1024)
    expect(pause).toHaveBeenCalledTimes(1)
    expect(resume).not.toHaveBeenCalled()

    releaseFirstWrite()
    await Promise.all(writes)

    expect(persisted).toEqual([1, 2, 3])
    expect(gate.retainedBytes).toBe(0)
    expect(resume).toHaveBeenCalledTimes(1)
  })

  it('re-arms healthy backpressure after a storage fault is retried', async () => {
    let failFirstWrite = true
    let releaseSecondWrite: () => void = () => {}
    const secondWriteReleased = new Promise<void>((resolve) => { releaseSecondWrite = resolve })
    const pause = vi.fn()
    const gate = new SessionDurabilityGate({
      sessionId: 'backpressure-after-retry',
      pauser: { pause, resume: vi.fn() }
    })

    await gate.append(frame(1, 1024 * 1024, async () => {
      if (failFirstWrite) {
        failFirstWrite = false
        throw storageError('ENOSPC')
      }
    }))
    await gate.retry()

    const secondWrite = gate.append(frame(2, 1024 * 1024, async () => {
      await secondWriteReleased
    }))
    expect(pause).toHaveBeenCalledTimes(2)

    releaseSecondWrite()
    await secondWrite
  })

  it('isolates an EACCES fault to one session while another keeps persisting', async () => {
    const pausedA = vi.fn()
    const pausedB = vi.fn()
    const persistedB: number[] = []
    const gateA = new SessionDurabilityGate({
      sessionId: 'a',
      pauser: { pause: pausedA, resume: vi.fn() }
    })
    const gateB = new SessionDurabilityGate({
      sessionId: 'b',
      pauser: { pause: pausedB, resume: vi.fn() }
    })

    await gateA.append(frame(1, 1, async () => { throw storageError('EACCES') }))
    await gateB.append(frame(1, 1, async () => { persistedB.push(1) }))
    await gateB.append(frame(2, 1, async () => { persistedB.push(2) }))

    expect(gateA.state).toBe('paused')
    expect(gateB.state).toBe('healthy')
    expect(pausedA).toHaveBeenCalledTimes(1)
    expect(pausedB).not.toHaveBeenCalled()
    expect(persistedB).toEqual([1, 2])
  })

  it('stays paused when retry fails again and emits one fault event for the episode', async () => {
    const faults = vi.fn()
    const resume = vi.fn()
    const gate = new SessionDurabilityGate({
      sessionId: 'retry-fails',
      pauser: { pause: vi.fn(), resume },
      onFault: faults
    })
    let attempts = 0
    await gate.append(frame(1, 1, async () => {
      attempts += 1
      throw storageError('ENOSPC')
    }))

    await expect(gate.retry()).rejects.toMatchObject({ code: 'ENOSPC' })

    expect(attempts).toBe(2)
    expect(gate.state).toBe('paused')
    expect(faults).toHaveBeenCalledTimes(1)
    expect(resume).not.toHaveBeenCalled()
  })

  it('queues a retry requested while the first pause is still in progress', async () => {
    let releasePause: () => void = () => {}
    const pauseReleased = new Promise<void>((resolve) => { releasePause = resolve })
    let failOnce = true
    const persisted: number[] = []
    const gate = new SessionDurabilityGate({
      sessionId: 'retry-during-pause',
      pauser: {
        pause: () => pauseReleased,
        resume: vi.fn()
      }
    })
    const append = gate.append(frame(1, 1, async () => {
      if (failOnce) {
        failOnce = false
        throw storageError('ENOSPC')
      }
      persisted.push(1)
    }))
    await vi.waitFor(() => { expect(gate.state).toBe('pausing') })

    const retry = gate.retry()
    releasePause()
    await append
    await retry

    expect(gate.state).toBe('healthy')
    expect(persisted).toEqual([1])
  })

  it('ends a faulted gate without waiting for storage and releases the stopped process', async () => {
    const resume = vi.fn()
    const gate = new SessionDurabilityGate({
      sessionId: 'ended',
      pauser: { pause: vi.fn(), resume }
    })
    await gate.append(frame(1, 32, async () => { throw storageError('ENOSPC') }))

    await gate.end()

    expect(gate.state).toBe('ended')
    expect(gate.retainedBytes).toBe(0)
    expect(resume).toHaveBeenCalledTimes(1)
  })
})
