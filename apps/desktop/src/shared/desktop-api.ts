export interface MatouDesktopApi {
  getPathForFile(file: File): string
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
  updateDagNotifications(mainWindowId: string, sessionIds: string[]): Promise<void>
  onDagContext(listener: (context: DagWindowContext) => void): () => void
  onDagNotifications(listener: (sessionIds: string[]) => void): () => void
  onDagNodeSelected(listener: (selection: DagNodeSelection) => void): () => void
  onDagShortcut(listener: (kind: 'short' | 'long') => void): () => void
  onScrollGesture(listener: (phase: 'begin' | 'end') => void): () => void
  onRuntimeConnectionState(listener: (state: RuntimeConnectionState) => void): () => void
  getAppUpdateState(): Promise<AppUpdateState>
  checkForAppUpdates(): Promise<void>
  downloadAppUpdate(): Promise<void>
  installAppUpdate(): Promise<void>
  onAppUpdateState(listener: (state: AppUpdateState) => void): () => void
}

export type RuntimeConnectionState = 'reconnecting' | 'ready'

export interface AppUpdateProgress {
  percent: number
  transferredBytes: number
  totalBytes: number
  bytesPerSecond: number
  remainingSeconds?: number
}

export interface AppUpdateReleaseState {
  currentVersion: string
  version: string
  releaseDate?: string
  releaseNotes: string[]
  sizeBytes?: number
}

export type AppUpdateState =
  | { status: 'idle' | 'checking' | 'not-available'; currentVersion: string }
  | ({ status: 'available' | 'downloaded' } & AppUpdateReleaseState)
  | ({ status: 'downloading'; progress: AppUpdateProgress } & AppUpdateReleaseState)
  | { status: 'error'; currentVersion: string; errorMessage: string }

export interface DetachedTerminalWindowInput {
  windowId: string
  mainWindowId: string
  sceneId: string
  mountId: string
  sessionId: string
  executionContextId: string
  profile: 'shell' | 'claude-code' | 'codex' | 'agent-team-member'
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
  notificationSessionIds?: string[]
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
  updateDagNotifications: 'matou:update-dag-notifications',
  dagContext: 'matou:dag-context',
  dagNotifications: 'matou:dag-notifications',
  dagNodeSelected: 'matou:dag-node-selected',
  dagShortcut: 'matou:dag-shortcut',
  scrollGesture: 'matou:scroll-gesture',
  runtimeConnectionState: 'matou:runtime-connection-state',
  getAppUpdateState: 'matou:app-update:get-state',
  checkForAppUpdates: 'matou:app-update:check',
  downloadAppUpdate: 'matou:app-update:download',
  installAppUpdate: 'matou:app-update:install',
  appUpdateState: 'matou:app-update:state'
} as const
