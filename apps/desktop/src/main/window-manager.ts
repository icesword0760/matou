export interface ManagedWindow {
  hide(): void
  show(): void
  focus(): void
  isVisible(): boolean
  isDestroyed(): boolean
}

export class WindowManager {
  readonly #windows = new Map<string, ManagedWindow>()

  register(windowId: string, window: ManagedWindow): void {
    this.#windows.set(windowId, window)
  }

  unregister(windowId: string): void {
    this.#windows.delete(windowId)
  }

  hideWindow(windowId: string): void {
    this.#require(windowId).hide()
  }

  showWindow(windowId: string): void {
    const window = this.#require(windowId)
    window.show()
    window.focus()
  }

  firstLiveWindowId(): string | undefined {
    return [...this.#windows].find(([, window]) => !window.isDestroyed())?.[0]
  }

  #require(windowId: string): ManagedWindow {
    const window = this.#windows.get(windowId)
    if (!window || window.isDestroyed()) throw new Error(`Window ${windowId} does not exist`)
    return window
  }
}
