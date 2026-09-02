import { afterEach, describe, expect, it, vi } from 'vitest'

import { PtyOutputBatcher } from './pty-output-batcher'

afterEach(() => vi.useRealTimers())

describe('PtyOutputBatcher', () => {
  it('preserves adjacent PTY byte-string order in one timed batch', () => {
    vi.useFakeTimers()
    const emit = vi.fn()
    const batcher = new PtyOutputBatcher(emit, { delayMs: 16 })
    batcher.offer('one')
    batcher.offer('二')

    vi.advanceTimersByTime(15)
    expect(emit).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(emit).toHaveBeenCalledWith('one二')
  })

  it('flushes synchronously at the bounded batch size', () => {
    const emit = vi.fn()
    const batcher = new PtyOutputBatcher(emit, { maxCodeUnits: 4 })
    batcher.offer('ab')
    batcher.offer('cd')

    expect(emit).toHaveBeenCalledWith('abcd')
  })

  it('flushes pending output before a following control frame', () => {
    const events: string[] = []
    const batcher = new PtyOutputBatcher((data) => events.push(`output:${data}`))
    batcher.offer('tail')
    batcher.flush()
    events.push('exit')

    expect(events).toEqual(['output:tail', 'exit'])
  })
})
