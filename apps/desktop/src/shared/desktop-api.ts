import type {
  RuntimeRecoveryCommandAction,
  RuntimeRecoverySnapshot
} from '@matou/contracts'

export interface MatouDesktopApi {
  getPathForFile(file: File): string
  selectWorkspaceDirectory(): Promise<string | null>
  selectSessionEnvironmentDirectory(): Promise<string | null>
  revealDirectory(path: string): Promise<void>
  openDirectoryInTerminal(path: string): Promise<void>
  hideWindow(windowId: string): Promise<void>
  showWindow(windowId: string): Promise<void>
  createDetachedTerminalWindow(input: DetachedTerminalWindowInput): Promise<void>
  closeDetachedTerminalWindow(windowId: string): Promise<void>
  detachedTerminalWindowExists(windowId: string): Promise<boolean>
  onDetachedWindowClosed(listener: (event: DetachedWindowClosedEvent) => void): () => void
  openDagWindow(input: DagWindowContext): Promise<void>
  selectDagNode(input: DagNodeSelection): Promise<void>
  closeDagWindow(mainWindowId: string): Promise<void>
  updateDagNotifications(mainWindowId: string, sessionIds: string[]): Promise<void>
  onDagContext(listener: (context: DagWindowContext) => void): () => void
  onDagNotifications(listener: (sessionIds: string[]) => void): () => void
  onDagNodeSelected(listener: (selection: DagNodeSelection) => void): () => void
  onDagShortcut(listener: (kind: 'short' | 'long') => void): () => void
  onRuntimeConnectionState(listener: (state: RuntimeConnectionState) => void): () => void
  getRuntimeLifecycle(): Promise<RuntimeLifecyclePresentation>
  onRuntimeLifecycle(listener: (state: RuntimeLifecyclePresentation) => void): () => void
  restoreDatabaseBackup(
    backupId: string,
    expectedRecoveryId: string
  ): Promise<RuntimeRecoveryCommandResult>
  exportDatabaseRecoveryBundle(): Promise<RuntimeRecoveryCommandResult>
  retryDatabaseOpen(expectedRecoveryId: string): Promise<RuntimeRecoveryCommandResult>
  startWithEmptyDatabase(expectedRecoveryId: string): Promise<RuntimeRecoveryCommandResult>
}

export type RuntimeConnectionState = 'reconnecting' | 'ready'

export interface RuntimeRecoveryBackup {
  id: string
  createdAt: number
  reason: 'pre-migration' | 'clean-exit'
  schemaVersion: number
  size: number
  sha256: string
}

export interface RuntimeRecoveryDetails {
  recoveryId: string
  reason: 'physical-corruption' | 'wal-recovery-required' | 'ownership-recovery-required'
  durableDatabasePath: string
  quarantinedPath: string
  ownershipIssue?: 'owner-record-malformed' | 'takeover-sidecar-unusable'
  backups: RuntimeRecoveryBackup[]
  error?: string
}

export interface RuntimeRecoveryOperation {
  requestId: string
  action: RuntimeRecoveryCommandAction
  pending: boolean
  error?: string
}

export interface RuntimeLifecyclePresentation {
  snapshot: RuntimeRecoverySnapshot
  recovery?: RuntimeRecoveryDetails
  operation?: RuntimeRecoveryOperation
}

export interface RuntimeRecoveryCommandResult {
  exportedPath?: string
}

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
  selectSessionEnvironmentDirectory: 'matou:select-session-environment-directory',
  revealDirectory: 'matou:reveal-directory',
  openDirectoryInTerminal: 'matou:open-directory-in-terminal',
  hideWindow: 'matou:hide-window',
  showWindow: 'matou:show-window',
  createDetachedTerminalWindow: 'matou:create-detached-terminal-window',
  closeDetachedTerminalWindow: 'matou:close-detached-terminal-window',
  detachedTerminalWindowExists: 'matou:detached-terminal-window-exists',
  detachedWindowClosed: 'matou:detached-window-closed',
  openDagWindow: 'matou:open-dag-window',
  selectDagNode: 'matou:select-dag-node',
  closeDagWindow: 'matou:close-dag-window',
  updateDagNotifications: 'matou:update-dag-notifications',
  dagContext: 'matou:dag-context',
  dagNotifications: 'matou:dag-notifications',
  dagNodeSelected: 'matou:dag-node-selected',
  dagShortcut: 'matou:dag-shortcut',
  runtimeConnectionState: 'matou:runtime-connection-state',
  runtimeLifecycle: 'matou:runtime-lifecycle',
  getRuntimeLifecycle: 'matou:get-runtime-lifecycle',
  restoreDatabaseBackup: 'matou:restore-database-backup',
  exportDatabaseRecoveryBundle: 'matou:export-database-recovery-bundle',
  retryDatabaseOpen: 'matou:retry-database-open',
  startWithEmptyDatabase: 'matou:start-with-empty-database'
} as const
