import { EventEmitter } from 'node:events'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const electron = vi.hoisted(() => {
  const children: unknown[] = []
  const fork = vi.fn()
  return { children, fork }
})

vi.mock('electron', () => ({
  utilityProcess: { fork: electron.fork },
  MessageChannelMain: class {
    port1 = { side: 1 }
    port2 = { side: 2 }
  }
}))

import { RuntimeHost } from './runtime-host'

beforeEach(() => {
  electron.children.splice(0)
  electron.fork.mockReset()
  electron.fork.mockImplementation(() => {
    const child = new MockUtilityProcess()
    electron.children.push(child)
    return child
  })
  vi.useFakeTimers()
})
afterEach(() => vi.useRealTimers())

describe('RuntimeHost', () => {
  it('passes the private control asset root and bundled executable to the Runtime', async () => {
    const host = new RuntimeHost('/runtime/index.cjs', '/runtime/control-assets')
    const starting = host.start()
    ;(electron.children[0] as MockUtilityProcess).emit('spawn')
    await starting

    expect(electron.fork).toHaveBeenCalledWith('/runtime/index.cjs', [], expect.objectContaining({
      env: expect.objectContaining({
        MATOU_CONTROL_ASSET_ROOT: '/runtime/control-assets',
        MATOU_CONTROL_NODE_EXECUTABLE: process.execPath
      })
    }))
  })

  it('restarts a crashed Runtime and transfers a fresh port to every live Renderer', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    ;(electron.children[0] as MockUtilityProcess).emit('spawn')
    await starting
    const renderer = new MockWebContents()
    host.connect(renderer as never)
    expect(renderer.messages).toHaveLength(1)

    ;(electron.children[0] as MockUtilityProcess).emit('exit', 9)
    expect(renderer.signals).toContainEqual(['matou:runtime-connection-state', 'reconnecting'])
    await vi.advanceTimersByTimeAsync(100)
    expect(electron.children).toHaveLength(2)
    ;(electron.children[1] as MockUtilityProcess).emit('spawn')
    await vi.runAllTimersAsync()

    expect(renderer.messages).toHaveLength(2)
    expect(renderer.messages[1]!.ports[0]).not.toBe(renderer.messages[0]!.ports[0])
    expect(renderer.signals).toContainEqual(['matou:runtime-connection-state', 'ready'])
    host.stop()
    ;(electron.children[1] as MockUtilityProcess).emit('exit', 0)
    await vi.runAllTimersAsync()
    expect(electron.children).toHaveLength(2)
  })

  it('keeps a Renderer opened during recovery registered and tells it that information is stale', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    ;(electron.children[0] as MockUtilityProcess).emit('spawn')
    await starting
    ;(electron.children[0] as MockUtilityProcess).emit('exit', 9)

    const renderer = new MockWebContents()
    expect(() => host.connect(renderer as never)).not.toThrow()
    expect(renderer.messages).toHaveLength(0)
    expect(renderer.signals).toContainEqual(['matou:runtime-connection-state', 'reconnecting'])

    await vi.advanceTimersByTimeAsync(100)
    ;(electron.children[1] as MockUtilityProcess).emit('spawn')
    await vi.runAllTimersAsync()
    expect(renderer.messages).toHaveLength(1)
    expect(renderer.signals).toContainEqual(['matou:runtime-connection-state', 'ready'])
  })

  it('does not report shutdown complete until the Runtime has flushed and exited', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    const child = electron.children[0] as MockUtilityProcess
    child.emit('spawn')
    await starting

    let stopped = false
    const stopping = host.stop().then(() => { stopped = true })
    expect(child.kill).toHaveBeenCalledOnce()
    await Promise.resolve()
    expect(stopped).toBe(false)

    child.emit('exit', 0)
    await stopping
    expect(stopped).toBe(true)
  })
})

class MockUtilityProcess extends EventEmitter {
  stdout = { pipe: vi.fn() }
  stderr = { pipe: vi.fn() }
  postMessage = vi.fn()
  kill = vi.fn()
}

class MockWebContents {
  readonly messages: Array<{ channel: string; ports: unknown[] }> = []
  readonly signals: Array<[string, unknown]> = []
  postMessage(channel: string, _message: unknown, ports: unknown[]): void {
    this.messages.push({ channel, ports })
  }
  send(channel: string, value: unknown): void { this.signals.push([channel, value]) }
  isDestroyed(): boolean { return false }
}
