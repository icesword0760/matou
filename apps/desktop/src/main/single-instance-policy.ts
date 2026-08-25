import type { WindowManager } from './window-manager'

export interface SingleInstanceApp {
  requestSingleInstanceLock(): boolean
  quit(): void
  onSecondInstance(listener: () => void): void
}

/**
 * Keeps the packaged desktop application as the sole owner of its Runtime and SQLite store.
 * Development instances stay independent so local comparison builds can run side by side.
 */
export function claimSingleInstance(
  app: SingleInstanceApp,
  windows: WindowManager,
  enabled: boolean
): boolean {
  if (!enabled) return true
  if (!app.requestSingleInstanceLock()) {
    app.quit()
    return false
  }
  app.onSecondInstance(() => {
    const windowId = windows.firstLiveWindowId()
    if (windowId) windows.showWindow(windowId)
  })
  return true
}
