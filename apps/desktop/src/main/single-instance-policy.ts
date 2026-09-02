import type { WindowManager } from './window-manager'

export interface SingleInstanceApp {
  requestSingleInstanceLock(): boolean
  quit(): void
  onSecondInstance(listener: (argv: string[]) => void): void
}

/**
 * Keeps the packaged desktop application as the sole owner of its Runtime and SQLite store.
 * Development instances stay independent so local comparison builds can run side by side.
 */
export function claimSingleInstance(
  app: SingleInstanceApp,
  windows: WindowManager,
  enabled: boolean,
  openWorkspace?: (path: string) => void
): boolean {
  if (!enabled) return true
  if (!app.requestSingleInstanceLock()) {
    app.quit()
    return false
  }
  app.onSecondInstance((argv) => {
    const path = workspacePathFromArguments(argv)
    if (path) openWorkspace?.(path)
    const windowId = windows.firstLiveWindowId()
    if (windowId) windows.showWindow(windowId)
  })
  return true
}

export function workspacePathFromArguments(argv: string[]): string | undefined {
  const marker = argv.lastIndexOf('--open-workspace')
  const path = marker >= 0 ? argv[marker + 1]?.trim() : undefined
  return path || undefined
}
