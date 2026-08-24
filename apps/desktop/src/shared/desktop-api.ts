export interface MatouDesktopApi {
  selectWorkspaceDirectory(): Promise<string | null>
  hideWindow(windowId: string): Promise<void>
  showWindow(windowId: string): Promise<void>
  createDetachedTerminalWindow(input: DetachedTerminalWindowInput): Promise<void>
  closeDetachedTerminalWindow(windowId: string): Promise<void>
  onDetachedWindowClosed(listener: (event: DetachedWindowClosedEvent) => void): () => void
}

export interface DetachedTerminalWindowInput {
  windowId: string
  mainWindowId: string
  sceneId: string
  mountId: string
  sessionId: string
  executionContextId: string
  profile: 'shell' | 'claude-code' | 'codex'
  title: string
}

export interface DetachedWindowClosedEvent {
  windowId: string
  mainWindowId: string
  sceneId: string
  mountId: string
  sessionId: string
}

export const DESKTOP_CHANNELS = {
  selectWorkspaceDirectory: 'matou:select-workspace-directory',
  hideWindow: 'matou:hide-window',
  showWindow: 'matou:show-window',
  createDetachedTerminalWindow: 'matou:create-detached-terminal-window',
  closeDetachedTerminalWindow: 'matou:close-detached-terminal-window',
  detachedWindowClosed: 'matou:detached-window-closed'
} as const
