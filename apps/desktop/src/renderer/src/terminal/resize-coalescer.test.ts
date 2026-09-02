import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ResizeCoalescer } from './resize-coalescer'

describe('ResizeCoalescer', () => {
  let callbacks: Map<number, FrameRequestCallback>
  let nextFrameId: number

  beforeEach(() => {
    callbacks = new Map()
    nextFrameId = 1
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
      const frameId = nextFrameId++
      callbacks.set(frameId, callback)
      return frameId
    })
    vi.stubGlobal('cancelAnimationFrame', (frameId: number) => {
      callbacks.delete(frameId)
    })
  })

  afterEach(() => vi.unstubAllGlobals())

  const runFrame = (timestamp: number) => {
    const frame = [...callbacks.entries()][0]
    if (!frame) throw new Error('No animation frame was scheduled')
    callbacks.delete(frame[0])
    frame[1](timestamp)
  }

  it('sends only the final dimensions offered during one animation frame', () => {
    const sent = vi.fn()
    const coalescer = new ResizeCoalescer(sent)

    for (let index = 0; index < 100; index += 1) {
      coalescer.offer(80 + index, 24 + index)
    }
    expect(sent).not.toHaveBeenCalled()

    runFrame(1_000)

    expect(sent).toHaveBeenCalledTimes(1)
    expect(sent).toHaveBeenCalledWith(179, 123)
  })

  it('does not send the same dimensions again on a later frame', () => {
    const sent = vi.fn()
    const coalescer = new ResizeCoalescer(sent)

    coalescer.offer(120, 40)
    runFrame(1_000)
    coalescer.offer(120, 40)
    runFrame(1_017)

    expect(sent).toHaveBeenCalledTimes(1)
  })

  it('waits for a 60 Hz interval before sending the next changed dimensions', () => {
    const sent = vi.fn()
    const coalescer = new ResizeCoalescer(sent)

    coalescer.offer(100, 30)
    runFrame(1_000)
    coalescer.offer(101, 31)
    runFrame(1_008)
    expect(sent).toHaveBeenCalledTimes(1)

    runFrame(1_017)
    expect(sent).toHaveBeenCalledTimes(2)
    expect(sent).toHaveBeenLastCalledWith(101, 31)
  })

  it('flushes the final pending dimensions synchronously and cancels its frame', () => {
    const sent = vi.fn()
    const coalescer = new ResizeCoalescer(sent)

    coalescer.offer(132, 51)
    coalescer.flush()

    expect(sent).toHaveBeenCalledWith(132, 51)
    expect(callbacks.size).toBe(0)
  })

  it('drops pending and future dimensions after disposal', () => {
    const sent = vi.fn()
    const coalescer = new ResizeCoalescer(sent)

    coalescer.offer(90, 28)
    coalescer.dispose()
    coalescer.offer(100, 32)

    expect(callbacks.size).toBe(0)
    expect(sent).not.toHaveBeenCalled()
  })
})
