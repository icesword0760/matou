import { describe, expect, it, vi } from 'vitest'

import { CreditWindow } from './credit-window'

describe('CreditWindow', () => {
  it('pauses once unacknowledged bytes exceed the high watermark', () => {
    const pause = vi.fn()
    const window = new CreditWindow({ highWatermarkBytes: 100, lowWatermarkBytes: 40, onPause: pause })

    window.recordSent(1, 60)
    window.recordSent(2, 41)

    expect(window.unackedBytes).toBe(101)
    expect(window.isPaused).toBe(true)
    expect(pause).toHaveBeenCalledTimes(1)
  })

  it('resumes only after cumulative ACK reaches the low watermark', () => {
    const resume = vi.fn()
    const window = new CreditWindow({ highWatermarkBytes: 100, lowWatermarkBytes: 40, onResume: resume })
    window.recordSent(1, 70)
    window.recordSent(2, 50)

    window.acknowledge(1)
    expect(window.unackedBytes).toBe(50)
    expect(window.isPaused).toBe(true)

    window.acknowledge(2)
    expect(window.unackedBytes).toBe(0)
    expect(window.isPaused).toBe(false)
    expect(resume).toHaveBeenCalledTimes(1)
  })

  it('treats duplicate cumulative ACKs as idempotent', () => {
    const window = new CreditWindow({ highWatermarkBytes: 100, lowWatermarkBytes: 40 })
    window.recordSent(1, 20)

    window.acknowledge(1)
    window.acknowledge(1)

    expect(window.unackedBytes).toBe(0)
  })

  it('keeps accounting independent across session windows', () => {
    const first = new CreditWindow({ highWatermarkBytes: 100, lowWatermarkBytes: 40 })
    const second = new CreditWindow({ highWatermarkBytes: 100, lowWatermarkBytes: 40 })

    first.recordSent(1, 101)
    second.recordSent(1, 20)

    expect(first.isPaused).toBe(true)
    expect(second.isPaused).toBe(false)
    expect(second.unackedBytes).toBe(20)
  })

  it('rejects ACKs beyond the latest sent sequence', () => {
    const window = new CreditWindow({ highWatermarkBytes: 100, lowWatermarkBytes: 40 })
    window.recordSent(1, 20)

    expect(() => window.acknowledge(2)).toThrow(/latest sent sequence/)
  })
})
