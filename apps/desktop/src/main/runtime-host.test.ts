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

const opening = lifecycle('normal', 'opening-database', 0)
const ready = lifecycle('normal', 'ready', 1)
const recoveryRequired = lifecycle('recovery-required', 'opening-database', 0)

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
  it('publishes only child lifecycle states and withholds a terminal port until ready', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    const child = electron.children[0] as MockUtilityProcess
    child.emit('spawn')
    await starting
    const renderer = new MockWebContents()
    host.connect(renderer as never)
    expect(renderer.messages).toHaveLength(0)
    expect(host.getLifecycle().snapshot.stage).toBe('opening-database')

    child.emit('message', opening)
    expect(renderer.signals).toContainEqual(['matou:runtime-lifecycle', expect.objectContaining({
      snapshot: opening.snapshot
    })])
    expect(renderer.messages).toHaveLength(0)

    child.emit('message', ready)
    expect(host.getLifecycle().snapshot).toEqual(ready.snapshot)
    expect(renderer.messages).toHaveLength(1)
  })

  it('publishes recovery details without connecting the ordinary workspace', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    const child = electron.children[0] as MockUtilityProcess
    child.emit('spawn')
    await starting
    const renderer = new MockWebContents()
    host.connect(renderer as never)

    child.emit('message', {
      type: 'runtime.recovery-details',
      recovery: {
        reason: 'physical-corruption', durableDatabasePath: '/data/matou.sqlite',
        quarantinedPath: '/data/matou.sqlite.corrupt-1', backups: []
      }
    })
    child.emit('message', recoveryRequired)

    expect(host.getLifecycle()).toMatchObject({
      snapshot: recoveryRequired.snapshot,
      recovery: { reason: 'physical-corruption', backups: [] }
    })
    expect(renderer.messages).toHaveLength(0)
  })

  it('restarts a crashed Runtime and transfers a fresh port only after its ready event', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    const first = electron.children[0] as MockUtilityProcess
    first.emit('spawn')
    await starting
    first.emit('message', ready)
    const renderer = new MockWebContents()
    host.connect(renderer as never)
    expect(renderer.messages).toHaveLength(1)

    first.emit('exit', 9)
    expect(renderer.signals).toContainEqual(['matou:runtime-connection-state', 'reconnecting'])
    await vi.advanceTimersByTimeAsync(100)
    expect(electron.children).toHaveLength(2)
    const second = electron.children[1] as MockUtilityProcess
    second.emit('spawn')
    await Promise.resolve()
    expect(renderer.messages).toHaveLength(1)
    second.emit('message', ready)

    expect(renderer.messages).toHaveLength(2)
    expect(renderer.messages[1]!.ports[0]).not.toBe(renderer.messages[0]!.ports[0])
    expect(renderer.signals).toContainEqual(['matou:runtime-connection-state', 'ready'])
    host.stop()
    ;(electron.children[1] as MockUtilityProcess).emit('exit', 0)
    await vi.runAllTimersAsync()
    expect(electron.children).toHaveLength(2)
  })

  it('backs off repeated pre-ready failures and resets the delay after ready', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    ;(electron.children[0] as MockUtilityProcess).emit('spawn')
    await starting
    const delays = [100, 500, 1_000, 2_000, 5_000, 5_000]
    for (let index = 0; index < delays.length; index += 1) {
      ;(electron.children[index] as MockUtilityProcess).emit('exit', 9)
      await vi.advanceTimersByTimeAsync(delays[index]! - 1)
      expect(electron.children).toHaveLength(index + 1)
      await vi.advanceTimersByTimeAsync(1)
      expect(electron.children).toHaveLength(index + 2)
      ;(electron.children[index + 1] as MockUtilityProcess).emit('spawn')
      await Promise.resolve()
    }

    const recovered = electron.children.at(-1) as MockUtilityProcess
    recovered.emit('message', ready)
    recovered.emit('exit', 9)
    await vi.advanceTimersByTimeAsync(99)
    expect(electron.children).toHaveLength(delays.length + 1)
    await vi.advanceTimersByTimeAsync(1)
    expect(electron.children).toHaveLength(delays.length + 2)
  })

  it('correlates recovery commands and keeps an error available to the recovery page', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    const child = electron.children[0] as MockUtilityProcess
    child.emit('spawn')
    await starting

    const pending = host.recover({
      type: 'runtime.recovery-command', requestId: 'restore-1',
      action: 'restore-backup', backupId: 'backup-1'
    })
    expect(child.postMessage).toHaveBeenCalledWith(expect.objectContaining({ requestId: 'restore-1' }))
    await expect(host.recover({
      type: 'runtime.recovery-command', requestId: 'retry-while-restoring', action: 'retry-open'
    })).rejects.toThrow('already running')
    expect(child.postMessage).toHaveBeenCalledTimes(1)
    child.emit('message', {
      type: 'runtime.recovery-result', requestId: 'restore-1', ok: false,
      error: '校验失败'
    })
    await expect(pending).rejects.toThrow('校验失败')
    expect(host.getLifecycle().operation).toMatchObject({ error: '校验失败' })
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

function lifecycle(mode: 'normal' | 'recovery-required', stage: 'opening-database' | 'ready', completed: number) {
  return {
    type: 'runtime.lifecycle' as const,
    snapshot: {
      recoveryId: 'recovery-test', revision: stage === 'ready' ? 2 : 1,
      mode, stage, completed, total: 1, failures: []
    }
  }
}
