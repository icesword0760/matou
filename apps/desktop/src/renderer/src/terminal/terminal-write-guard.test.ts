import { afterEach, describe, expect, it, vi } from 'vitest'

import { TerminalWriteGuard } from './terminal-write-guard'

afterEach(() => vi.useRealTimers())

describe('TerminalWriteGuard', () => {
  it('detects a stalled asynchronous parser even while new output keeps arriving', () => {
    vi.useFakeTimers()
    const failed = vi.fn()
    const write = vi.fn()
    const guard = new TerminalWriteGuard({ write }, failed, 100)
    guard.write('first')
    vi.advanceTimersByTime(90)
    guard.write('second')
    vi.advanceTimersByTime(10)
    expect(failed).toHaveBeenCalledTimes(1)
    expect(guard.healthy).toBe(false)
    guard.write('after failure')
    expect(write).toHaveBeenCalledTimes(2)
    guard.dispose()
  })

  it('allows long streams that keep making progress and ignores late callbacks after failure', () => {
    vi.useFakeTimers()
    const callbacks: Array<() => void> = []
    const failed = vi.fn()
    const applied = vi.fn()
    const guard = new TerminalWriteGuard({ write: (_data, done) => callbacks.push(done!) }, failed, 100)
    guard.write('a', applied)
    guard.write('b', applied)
    vi.advanceTimersByTime(90)
    callbacks[0]!()
    vi.advanceTimersByTime(90)
    expect(failed).not.toHaveBeenCalled()
    callbacks[1]!()
    vi.advanceTimersByTime(500)
    expect(failed).not.toHaveBeenCalled()
    guard.write('stalled', applied)
    vi.advanceTimersByTime(100)
    callbacks[2]!()
    expect(applied).toHaveBeenCalledTimes(2)
    expect(failed).toHaveBeenCalledTimes(1)
    guard.dispose()
  })

  it.each(['write', 'callback'])('recovers a synchronous exception from the %s', (at) => {
    const failed = vi.fn()
    const error = () => { throw new RangeError('Array buffer allocation failed') }
    const guard = new TerminalWriteGuard({ write: at === 'write' ? error : (_data, done) => done?.() }, failed)
    expect(() => guard.write('data', at === 'callback' ? error : undefined)).not.toThrow()
    expect(failed).toHaveBeenCalledTimes(1)
    guard.dispose()
  })

  it('does not recover a deliberately disposed terminal', () => {
    vi.useFakeTimers()
    const failed = vi.fn()
    const guard = new TerminalWriteGuard({ write: vi.fn() }, failed, 100)
    guard.write('data')
    guard.dispose()
    vi.advanceTimersByTime(100)
    expect(failed).not.toHaveBeenCalled()
  })
})
