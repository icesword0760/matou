import { describe, expect, it, vi } from 'vitest'

import { LatestValueWriter } from './latest-value-writer'

describe('LatestValueWriter', () => {
  it('keeps only the newest snapshot behind an in-flight write', async () => {
    let release: (() => void) | undefined
    const first = new Promise<void>((resolve) => { release = resolve })
    const values: number[] = []
    const write = vi.fn(async (value: number) => {
      values.push(value)
      if (value === 1) await first
    })
    const writer = new LatestValueWriter(write)

    writer.schedule(1)
    await Promise.resolve()
    writer.schedule(2)
    writer.schedule(3)
    release?.()
    await writer.whenIdle()

    expect(values).toEqual([1, 3])
  })

  it('continues with the latest snapshot after an earlier sidecar write fails', async () => {
    let attempts = 0
    const values: number[] = []
    const writer = new LatestValueWriter<number>(async (value) => {
      values.push(value)
      attempts += 1
      if (attempts === 1) throw new Error('temporary sidecar fault')
    })

    writer.schedule(1)
    writer.schedule(2)
    await writer.whenIdle()

    expect(values).toEqual([1, 2])
  })
})
