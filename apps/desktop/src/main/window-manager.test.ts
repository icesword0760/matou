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
})

class FakeWindow implements ManagedWindow {
  #visible = true
  hide(): void { this.#visible = false }
  show(): void { this.#visible = true }
  focus(): void {}
  isVisible(): boolean { return this.#visible }
  isDestroyed(): boolean { return false }
}
