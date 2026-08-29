export interface MatouDesktopApi {
  selectWorkspaceDirectory(): Promise<string | null>
  revealDirectory(path: string): Promise<void>
  hideWindow(windowId: string): Promise<void>
  showWindow(windowId: string): Promise<void>
  createDetachedTerminalWindow(input: DetachedTerminalWindowInput): Promise<void>
  closeDetachedTerminalWindow(windowId: string): Promise<void>
  onDetachedWindowClosed(listener: (event: DetachedWindowClosedEvent) => void): () => void
  openDagWindow(input: DagWindowContext): Promise<void>
  selectDagNode(input: DagNodeSelection): Promise<void>
  closeDagWindow(mainWindowId: string): Promise<void>
  onDagContext(listener: (context: DagWindowContext) => void): () => void
  onDagNodeSelected(listener: (selection: DagNodeSelection) => void): () => void
  onDagShortcut(listener: (kind: 'short' | 'long') => void): () => void
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

export interface DagWindowContext {
  mainWindowId: string
  sceneId: string
  sessionId: string
  theme: 'light' | 'dark'
}

export interface DagNodeSelection extends DagWindowContext {
  targetWindowId?: string
}

export const DESKTOP_CHANNELS = {
  selectWorkspaceDirectory: 'matou:select-workspace-directory',
  revealDirectory: 'matou:reveal-directory',
  hideWindow: 'matou:hide-window',
  showWindow: 'matou:show-window',
  createDetachedTerminalWindow: 'matou:create-detached-terminal-window',
  closeDetachedTerminalWindow: 'matou:close-detached-terminal-window',
  detachedWindowClosed: 'matou:detached-window-closed',
  openDagWindow: 'matou:open-dag-window',
  selectDagNode: 'matou:select-dag-node',
  closeDagWindow: 'matou:close-dag-window',
  dagContext: 'matou:dag-context',
  dagNodeSelected: 'matou:dag-node-selected',
  dagShortcut: 'matou:dag-shortcut'
} as const
