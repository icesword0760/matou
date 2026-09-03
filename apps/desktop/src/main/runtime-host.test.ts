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
    const child = electron.children[0] as MockUtilityProcess
    child.emit('spawn')
    await starting

    const pending = host.getScaleMetrics({ resetStatementCount: true })
    expect(child.postMessage).toHaveBeenCalledWith({
      type: 'runtime.scale-metrics-request',
      requestId: expect.any(String),
      resetStatementCount: true
    })
    const request = child.postMessage.mock.calls[0]![0] as { requestId: string }
    child.emit('message', {
      type: 'runtime.scale-metrics-result',
      requestId: request.requestId,
      runtimePid: 321,
      ptyCount: 17,
      ptyPids: [401, 402],
      ptySessions: [{ sessionId: 'session-1', pid: 401 }],
      recoveryObservation: {
        maxRestoring: 1,
        transitions: [{
          sequence: 1, sessionId: 'session-1', sceneId: 'scene-1',
          priority: 'active-session', state: 'restoring', restoringCount: 1
        }]
      },
      statementCount: 81,
      statementProfile: [{ statement: 'SELECT 1', count: 81 }],
      eventLoopDelayP99Ms: 12.5,
      eventLoopDelayMaxMs: 21.75,
      maxUnackedBytes: 524_288,
      retainedDurabilityBytes: 262_144,
      heapUsedBytes: 10_000_000,
      externalBytes: 2_000_000,
      arrayBufferBytes: 1_000_000
    })

    await expect(pending).resolves.toEqual({
      runtimePid: 321,
      ptyCount: 17,
      ptyPids: [401, 402],
      ptySessions: [{ sessionId: 'session-1', pid: 401 }],
      recoveryObservation: {
        maxRestoring: 1,
        transitions: [{
          sequence: 1, sessionId: 'session-1', sceneId: 'scene-1',
          priority: 'active-session', state: 'restoring', restoringCount: 1
        }]
      },
      statementCount: 81,
      statementProfile: [{ statement: 'SELECT 1', count: 81 }],
      eventLoopDelayP99Ms: 12.5,
      eventLoopDelayMaxMs: 21.75,
      maxUnackedBytes: 524_288,
      retainedDurabilityBytes: 262_144,
      heapUsedBytes: 10_000_000,
      externalBytes: 2_000_000,
      arrayBufferBytes: 1_000_000
    })
  })

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
        recoveryId: 'durable-recovery-host',
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

  it('transfers a fresh port when the same Renderer finishes a reload', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    const child = electron.children[0] as MockUtilityProcess
    child.emit('spawn')
    await starting
    child.emit('message', ready)
    const renderer = new MockWebContents()

    host.connect(renderer as never)
    host.connect(renderer as never)

    expect(renderer.messages).toHaveLength(2)
    expect(renderer.messages[1]!.ports[0]).not.toBe(renderer.messages[0]!.ports[0])
    expect(child.postMessage).toHaveBeenCalledTimes(2)
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

  it('stops automatic retries for a deterministic startup failure and permits an explicit retry', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    const first = electron.children[0] as MockUtilityProcess
    first.emit('spawn')
    await starting
    const renderer = new MockWebContents()
    host.connect(renderer as never)

    first.emit('message', {
      type: 'runtime.startup-failure',
      code: 'MIGRATION_HISTORY_MISMATCH',
      message: '升级记录与当前版本不一致',
      retryable: false
    })
    first.emit('exit', 1)
    await vi.advanceTimersByTimeAsync(30_000)

    expect(electron.children).toHaveLength(1)
    expect(host.getLifecycle()).toMatchObject({
      startupFailure: {
        code: 'MIGRATION_HISTORY_MISMATCH',
        message: '升级记录与当前版本不一致'
      }
    })
    expect(renderer.signals).toContainEqual([
      'matou:runtime-lifecycle',
      expect.objectContaining({ startupFailure: expect.any(Object) })
    ])

    const retrying = host.retryStartup()
    const second = electron.children[1] as MockUtilityProcess
    second.emit('spawn')
    await retrying
    second.emit('message', ready)

    expect(host.getLifecycle().startupFailure).toBeUndefined()
    expect(host.getLifecycle().snapshot.stage).toBe('ready')
  })

  it('correlates recovery commands and keeps an error available to the recovery page', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    const child = electron.children[0] as MockUtilityProcess
    child.emit('spawn')
    await starting
    child.emit('message', {
      type: 'runtime.recovery-details',
      recovery: {
        recoveryId: 'durable-recovery-host',
        reason: 'physical-corruption', durableDatabasePath: '/data/matou.sqlite',
        quarantinedPath: '/data/matou.sqlite.corrupt-1', backups: []
      }
    })

    const pending = host.recover({
      type: 'runtime.recovery-command', requestId: 'restore-1',
      action: 'restore-backup', backupId: 'backup-1',
      expectedRecoveryId: 'durable-recovery-host'
    })
    expect(child.postMessage).toHaveBeenCalledWith(expect.objectContaining({ requestId: 'restore-1' }))
    await expect(host.recover({
      type: 'runtime.recovery-command', requestId: 'retry-while-restoring', action: 'retry-open',
      expectedRecoveryId: 'durable-recovery-host'
    })).rejects.toThrow('already running')
    expect(child.postMessage).toHaveBeenCalledTimes(1)
    child.emit('message', {
      type: 'runtime.recovery-result', requestId: 'restore-1', ok: false,
      error: '校验失败'
    })
    await expect(pending).rejects.toThrow('校验失败')
    expect(host.getLifecycle().operation).toMatchObject({ error: '校验失败' })
  })

  it('rejects a stale recovery generation before publishing operation state or messaging Runtime', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    const child = electron.children[0] as MockUtilityProcess
    child.emit('spawn')
    await starting
    child.emit('message', {
      type: 'runtime.recovery-details',
      recovery: {
        recoveryId: 'durable-recovery-current',
        reason: 'physical-corruption', durableDatabasePath: '/data/matou.sqlite',
        quarantinedPath: '/data/matou.sqlite.corrupt-1', backups: []
      }
    })
    child.emit('message', recoveryRequired)
    const presentationBefore = structuredClone(host.getLifecycle())

    const attempt = host.recover({
      type: 'runtime.recovery-command', requestId: 'stale-generation',
      action: 'retry-open', expectedRecoveryId: 'durable-recovery-previous'
    })
    child.emit('message', {
      type: 'runtime.recovery-result', requestId: 'stale-generation', ok: false,
      error: '数据库恢复周期已更新'
    })
    await expect(attempt).rejects.toThrow('数据库恢复周期已更新')

    expect(child.postMessage).not.toHaveBeenCalled()
    expect(host.getLifecycle()).toEqual(presentationBefore)
  })

  it('keeps recovery details and the interrupted operation error while the Runtime reconnects', async () => {
    const host = new RuntimeHost('/runtime/index.cjs')
    const starting = host.start()
    const first = electron.children[0] as MockUtilityProcess
    first.emit('spawn')
    await starting
    first.emit('message', {
      type: 'runtime.recovery-details',
      recovery: {
        recoveryId: 'durable-recovery-crash-1',
        reason: 'physical-corruption', durableDatabasePath: '/data/matou.sqlite',
        quarantinedPath: '/data/matou.sqlite.corrupt-1', backups: []
      }
    })
    first.emit('message', recoveryRequired)

    const pending = host.recover({
      type: 'runtime.recovery-command', requestId: 'restore-crash',
      action: 'restore-backup', backupId: 'backup-1',
      expectedRecoveryId: 'durable-recovery-crash-1'
    })
    first.emit('exit', 9)

    await expect(pending).rejects.toThrow('恢复操作期间退出')
    expect(host.getLifecycle()).toMatchObject({
      snapshot: { stage: 'opening-database' },
      recovery: { quarantinedPath: '/data/matou.sqlite.corrupt-1' },
      operation: {
        requestId: 'restore-crash', pending: false,
        error: '数据库恢复操作未完成：Runtime 在恢复操作期间退出'
      }
    })

    await vi.advanceTimersByTimeAsync(100)
    const second = electron.children[1] as MockUtilityProcess
    second.emit('spawn')
    await Promise.resolve()
    second.emit('message', {
      type: 'runtime.recovery-details',
      recovery: {
        recoveryId: 'durable-recovery-crash-2',
        reason: 'physical-corruption', durableDatabasePath: '/data/matou.sqlite',
        quarantinedPath: '/data/matou.sqlite.corrupt-2', backups: []
      }
    })
    second.emit('message', recoveryRequired)
    expect(host.getLifecycle()).toMatchObject({
      recovery: { quarantinedPath: '/data/matou.sqlite.corrupt-2' },
      operation: { pending: false, error: expect.stringContaining('Runtime') }
    })
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
