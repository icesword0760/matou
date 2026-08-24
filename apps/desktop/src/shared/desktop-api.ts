export interface MatouDesktopApi {
  selectWorkspaceDirectory(): Promise<string | null>
  hideWindow(windowId: string): Promise<void>
  showWindow(windowId: string): Promise<void>
}

export const DESKTOP_CHANNELS = {
  selectWorkspaceDirectory: 'matou:select-workspace-directory',
  hideWindow: 'matou:hide-window',
  showWindow: 'matou:show-window'
} as const
