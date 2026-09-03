import type {
  RuntimeRecoveryCommandAction,
  RuntimeRecoverySnapshot,
  RuntimeStartupFailure
} from '@matou/contracts'

export interface MatouDesktopApi {
  getPathForFile(file: File): string
  selectWorkspaceDirectory(): Promise<string | null>
  selectSessionEnvironmentDirectory(): Promise<string | null>
  consumeWorkspaceOpenRequests(): Promise<string[]>
  onWorkspaceOpenRequested(listener: () => void): () => void
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
  onScrollGesture(listener: (phase: 'begin' | 'end') => void): () => void
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
  retryRuntimeStart(): Promise<void>
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
  installMode: AppUpdateInstallMode
  manualDownloadUrl?: string
}

export type AppUpdateInstallMode = 'automatic' | 'manual'
export type AppUpdateErrorStage = 'check' | 'download' | 'verify' | 'install'

export type AppUpdateState =
  | { status: 'idle' | 'not-available'; currentVersion: string }
  | {
      status: 'checking'
      currentVersion: string
      retryAttempt?: number
      maxRetryAttempts?: number
    }
  | ({ status: 'available' | 'downloaded' } & AppUpdateReleaseState)
  | ({ status: 'downloading'; progress: AppUpdateProgress } & AppUpdateReleaseState)
  | {
      status: 'error'
      currentVersion: string
      errorMessage: string
      errorStage: AppUpdateErrorStage
      version?: string
      manualDownloadUrl?: string
    }

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
  startupFailure?: RuntimeStartupFailure
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
  requestedAt?: number
  initialGraph?: unknown
}

export interface DagNodeSelection extends Omit<DagWindowContext, 'initialGraph' | 'requestedAt'> {
  targetWindowId?: string
}

export const DESKTOP_CHANNELS = {
  selectWorkspaceDirectory: 'matou:select-workspace-directory',
  selectSessionEnvironmentDirectory: 'matou:select-session-environment-directory',
  consumeWorkspaceOpenRequests: 'matou:consume-workspace-open-requests',
  workspaceOpenRequested: 'matou:workspace-open-requested',
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
  scrollGesture: 'matou:scroll-gesture',
  runtimeConnectionState: 'matou:runtime-connection-state',
  runtimeLifecycle: 'matou:runtime-lifecycle',
  getRuntimeLifecycle: 'matou:get-runtime-lifecycle',
  restoreDatabaseBackup: 'matou:restore-database-backup',
  exportDatabaseRecoveryBundle: 'matou:export-database-recovery-bundle',
  retryDatabaseOpen: 'matou:retry-database-open',
  startWithEmptyDatabase: 'matou:start-with-empty-database',
  retryRuntimeStart: 'matou:retry-runtime-start',
  getAppUpdateState: 'matou:app-update:get-state',
  checkForAppUpdates: 'matou:app-update:check',
  downloadAppUpdate: 'matou:app-update:download',
  installAppUpdate: 'matou:app-update:install',
  appUpdateState: 'matou:app-update:state'
} as const
