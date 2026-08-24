import { describe, expect, it } from 'vitest'

import { WindowManager, type ManagedWindow } from './window-manager'

describe('WindowManager', () => {
  it('hides only the protected main window and restores it independently', () => {
    const manager = new WindowManager()
    const first = new FakeWindow()
    const second = new FakeWindow()
    manager.register('window-1', first)
    manager.register('window-2', second)

    manager.hideWindow('window-1')
    expect(first.isVisible()).toBe(false)
    expect(second.isVisible()).toBe(true)
    manager.showWindow('window-1')
    expect(first.isVisible()).toBe(true)
  })

  it('tracks a detached window independently from its owning main window', () => {
    const manager = new WindowManager()
    const main = new FakeWindow()
    const detached = new FakeWindow()
    manager.register('main-1', main)
    manager.register('detached-1', detached)

    expect(manager.getWindow('detached-1')).toBe(detached)
    manager.unregister('detached-1')
    expect(manager.getWindow('detached-1')).toBeUndefined()
    expect(manager.getWindow('main-1')).toBe(main)
  })
})

class FakeWindow implements ManagedWindow {
  #visible = true
  hide(): void { this.#visible = false }
  show(): void { this.#visible = true }
  focus(): void {}
  isVisible(): boolean { return this.#visible }
  isDestroyed(): boolean { return false }
}
