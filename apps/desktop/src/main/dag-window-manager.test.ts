import { describe, expect, it, vi } from 'vitest'

import { DagWindowManager, type DagWindowAdapter } from './dag-window-manager'
import type { DagWindowContext } from '../shared/desktop-api'

describe('DagWindowManager', () => {
  it('keeps one centered DAG window per main window and reconnects Runtime on ready', () => {
    const adapters: FakeDagWindow[] = []
    const connectRuntime = vi.fn()
    const manager = new DagWindowManager({
      createWindow: (input) => {
        const window = new FakeDagWindow(`dag-${adapters.length + 1}`, input.bounds)
        adapters.push(window)
        return window
      },
      displayBounds: () => ({ x: 100, y: 40, width: 1400, height: 900 }),
      connectRuntime,
      routeSelection: vi.fn(),
      activateTargetWindow: vi.fn(() => false)
    })

    manager.open(context('session-a'))
    expect(adapters).toHaveLength(1)
    expect(adapters[0]!.bounds).toEqual({ x: 320, y: 170, width: 960, height: 640 })
    adapters[0]!.ready()
    expect(connectRuntime).toHaveBeenCalledWith(adapters[0])
    expect(adapters[0]!.sent.at(-1)).toMatchObject({ value: { sessionId: 'session-a' } })

    manager.open(context('session-b'))
    expect(adapters).toHaveLength(1)
    expect(adapters[0]!.shown).toBe(2)
    expect(adapters[0]!.focused).toBe(2)
    expect(adapters[0]!.sent.at(-1)).toMatchObject({ value: { sessionId: 'session-b' } })

    manager.updateNotifications('main-1', ['session-b'])
    expect(adapters[0]!.sent.at(-1)).toMatchObject({ value: ['session-b'] })
  })

  it('routes node selection, activates detached targets and closes without touching a PTY', () => {
    const routeSelection = vi.fn()
    const activateTargetWindow = vi.fn((id: string) => id === 'detached-1')
    const adapter = new FakeDagWindow('dag-1', { x: 0, y: 0, width: 960, height: 640 })
    const manager = new DagWindowManager({
      createWindow: () => adapter,
      displayBounds: () => ({ x: 0, y: 0, width: 1200, height: 800 }),
      connectRuntime: vi.fn(), routeSelection, activateTargetWindow
    })
    manager.open(context('session-a'))

    manager.selectNode({ ...context('session-b'), targetWindowId: 'detached-1' })
    expect(activateTargetWindow).toHaveBeenCalledWith('detached-1')
    expect(routeSelection).not.toHaveBeenCalled()
    expect(adapter.closed).toBe(1)

    const second = new FakeDagWindow('dag-2', { x: 0, y: 0, width: 960, height: 640 })
    const manager2 = new DagWindowManager({
      createWindow: () => second,
      displayBounds: () => ({ x: 0, y: 0, width: 1200, height: 800 }),
      connectRuntime: vi.fn(), routeSelection, activateTargetWindow: vi.fn(() => false)
    })
    manager2.open(context('session-a'))
    manager2.selectNode(context('session-c'))
    expect(routeSelection).toHaveBeenCalledWith('main-1', expect.objectContaining({ sessionId: 'session-c' }))
    expect(second.closed).toBe(1)
  })
})

function context(sessionId: string): DagWindowContext {
  return { mainWindowId: 'main-1', sceneId: 'scene-1', sessionId, theme: 'light' }
}

class FakeDagWindow implements DagWindowAdapter {
  readonly sent: Array<{ channel: string; value: unknown }> = []
  shown = 0
  focused = 0
  closed = 0
  #ready: (() => void) | undefined
  #closed: (() => void) | undefined

  constructor(readonly id: string, readonly bounds: { x: number; y: number; width: number; height: number }) {}
  isDestroyed() { return false }
  show() { this.shown += 1 }
  focus() { this.focused += 1 }
  close() { this.closed += 1; this.#closed?.() }
  send(channel: string, value: unknown) { this.sent.push({ channel, value }) }
  onReady(listener: () => void) { this.#ready = listener }
  onClosed(listener: () => void) { this.#closed = listener }
  ready() { this.#ready?.() }
}
