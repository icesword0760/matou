import { afterEach, describe, expect, it, vi } from 'vitest'

import { TerminalOutputCoalescer } from './terminal-output-coalescer'

afterEach(() => vi.useRealTimers())

describe('TerminalOutputCoalescer', () => {
  it('writes visible output immediately without copying it', () => {
    const write = vi.fn()
    const output = new TerminalOutputCoalescer(write)
    const bytes = new Uint8Array([1, 2, 3])

    output.offer(bytes, 7, true)

    expect(write).toHaveBeenCalledWith(bytes, 7)
  })

  it('joins hidden output at a bounded cadence and acknowledges the newest sequence', () => {
    vi.useFakeTimers()
    const write = vi.fn()
    const output = new TerminalOutputCoalescer(write, { delayMs: 100, maxBytes: 1024 })
    output.offer(new Uint8Array([1, 2]), 3, false)
    output.offer(new Uint8Array([4, 5]), 4, false)

    vi.advanceTimersByTime(99)
    expect(write).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)

    expect([...write.mock.calls[0]![0]]).toEqual([1, 2, 4, 5])
    expect(write.mock.calls[0]![1]).toBe(4)
  })

  it('flushes before immediate output so byte and sequence order stay stable', () => {
    const write = vi.fn()
    const output = new TerminalOutputCoalescer(write)
    output.offer(new Uint8Array([1]), 1, false)
    output.offer(new Uint8Array([2]), 2, true)

    expect(write.mock.calls.map(([bytes, sequence]) => [[...bytes], sequence])).toEqual([
      [[1], 1], [[2], 2]
    ])
  })

  it('flushes at the byte cap instead of growing with sustained output', () => {
    const write = vi.fn()
    const output = new TerminalOutputCoalescer(write, { maxBytes: 4 })
    output.offer(new Uint8Array([1, 2]), 1, false)
    output.offer(new Uint8Array([3, 4]), 2, false)

    expect([...write.mock.calls[0]![0]]).toEqual([1, 2, 3, 4])
    expect(write.mock.calls[0]![1]).toBe(2)
  })
})
