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

  it('forwards a workspace path from a second launch to the existing packaged instance', () => {
    const app = new FakeApp(true)
    const windows = new WindowManager()
    const window = new FakeWindow()
    const openWorkspace = vi.fn()
    windows.register('main-window-1', window)

    expect(claimSingleInstance(app, windows, true, openWorkspace)).toBe(true)
    app.emitSecondInstance([
      '/Applications/码头.app/Contents/MacOS/码头',
      '--open-workspace',
      '/Users/demo/projects/matou'
    ])

    expect(openWorkspace).toHaveBeenCalledWith('/Users/demo/projects/matou')
    expect(window.focused).toBe(true)
  })

  it('ignores a second launch without an explicit workspace request', () => {
    const app = new FakeApp(true)
    const openWorkspace = vi.fn()

    expect(claimSingleInstance(app, new WindowManager(), true, openWorkspace)).toBe(true)
    app.emitSecondInstance(['/Applications/码头.app/Contents/MacOS/码头'])

    expect(openWorkspace).not.toHaveBeenCalled()
  })
})

class FakeApp implements SingleInstanceApp {
  readonly requestSingleInstanceLock = vi.fn(() => this.lockGranted)
  readonly quit = vi.fn()
  #secondInstance: ((argv: string[]) => void) | undefined

  constructor(private readonly lockGranted: boolean) {}

  onSecondInstance(listener: (argv: string[]) => void): void {
    this.#secondInstance = listener
  }

  emitSecondInstance(argv: string[] = []): void {
    this.#secondInstance?.(argv)
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
