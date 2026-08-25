import { describe, expect, it, vi } from 'vitest'

import { claimSingleInstance, type SingleInstanceApp } from './single-instance-policy'
import { WindowManager, type ManagedWindow } from './window-manager'

describe('claimSingleInstance', () => {
  it('keeps development instances independent without taking a production lock', () => {
    const app = new FakeApp(false)
    expect(claimSingleInstance(app, new WindowManager(), false)).toBe(true)
    expect(app.requestSingleInstanceLock).not.toHaveBeenCalled()
    expect(app.quit).not.toHaveBeenCalled()
  })

  it('ends a second packaged instance before it can open the Runtime database', () => {
    const app = new FakeApp(false)
    expect(claimSingleInstance(app, new WindowManager(), true)).toBe(false)
    expect(app.requestSingleInstanceLock).toHaveBeenCalledOnce()
    expect(app.quit).toHaveBeenCalledOnce()
  })

  it('shows and focuses the existing packaged window when another launch is attempted', () => {
    const app = new FakeApp(true)
    const windows = new WindowManager()
    const window = new FakeWindow()
    windows.register('main-window-1', window)
    windows.hideWindow('main-window-1')

    expect(claimSingleInstance(app, windows, true)).toBe(true)
    app.emitSecondInstance()

    expect(window.isVisible()).toBe(true)
    expect(window.focused).toBe(true)
  })
})

class FakeApp implements SingleInstanceApp {
  readonly requestSingleInstanceLock = vi.fn(() => this.lockGranted)
  readonly quit = vi.fn()
  #secondInstance: (() => void) | undefined

  constructor(private readonly lockGranted: boolean) {}

  onSecondInstance(listener: () => void): void {
    this.#secondInstance = listener
  }

  emitSecondInstance(): void {
    this.#secondInstance?.()
  }
}

class FakeWindow implements ManagedWindow {
  #visible = true
  focused = false
  hide(): void { this.#visible = false }
  show(): void { this.#visible = true }
  focus(): void { this.focused = true }
  isVisible(): boolean { return this.#visible }
  isDestroyed(): boolean { return false }
}
