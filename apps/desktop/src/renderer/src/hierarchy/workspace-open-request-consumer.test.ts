import { describe, expect, it, vi } from 'vitest'

import { WorkspaceOpenRequestConsumer } from './workspace-open-request-consumer'

describe('WorkspaceOpenRequestConsumer', () => {
  it('creates or focuses requested workspaces in Finder arrival order', async () => {
    const batches = [
      ['/Users/demo/first', '/Users/demo/second'],
      []
    ]
    const read = vi.fn(async () => batches.shift() ?? [])
    const open = vi.fn(async () => {})
    const consumer = new WorkspaceOpenRequestConsumer(read, open)

    await consumer.drain()

    expect(open.mock.calls).toEqual([
      ['/Users/demo/first'],
      ['/Users/demo/second']
    ])
    expect(read).toHaveBeenCalledTimes(2)
  })

  it('coalesces simultaneous wakeups while a request is being handled', async () => {
    let release: (() => void) | undefined
    const gate = new Promise<void>((resolve) => { release = resolve })
    const read = vi.fn()
      .mockResolvedValueOnce(['/Users/demo/project'])
      .mockResolvedValue([])
    const open = vi.fn(async () => { await gate })
    const consumer = new WorkspaceOpenRequestConsumer(read, open)

    const first = consumer.drain()
    const second = consumer.drain()
    release?.()
    await Promise.all([first, second])

    expect(open).toHaveBeenCalledOnce()
    expect(read).toHaveBeenCalledTimes(2)
  })
})
