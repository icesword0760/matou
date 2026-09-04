export interface WorkspaceView {
  id: string; name: string; rootDirectory: string
  isDefault?: boolean; isPinned?: boolean; pinSortKey?: string; lastOpenedAt?: number
}
export interface TaskView {
  id: string; workspaceId: string; title: string; sortKey?: string
  status?: 'planned' | 'active' | 'blocked' | 'completed' | 'archived'
  isPinned?: boolean; pinSortKey?: string; lastOpenedAt?: number; createdAt?: number
  updatedAt?: number
}
export interface SceneView {
  id: string
  taskId: string
  name: string
  sortKey?: string
  titlePinned?: boolean
  rootNodeId?: string
  layoutRevision?: number
  archivedAt?: number
}
export interface SessionView {
  id: string
  taskId: string
  title: string
  titleSource?: 'default' | 'auto' | 'manual'
  providerTitle?: string
  kind?: 'shell' | 'claude-code' | 'codex' | string
  status?: string
  executionContextId?: string
}
export interface SessionGraphNodeView {
  sessionId: string
  sceneId: string
  parentSessionId?: string
  relationKind?: 'derived-from' | 'forked-from'
  currentMode: 'shell' | 'claude-code' | 'codex' | 'agent-team-member'
  workStatus: 'starting' | 'idle' | 'running' | 'needs-input' | 'error' | 'interrupted' | 'exited'
  providerRestoreState: 'none' | 'restoring' | 'failed'
  providerRestoreError?: string
  forkState?: 'pending' | 'starting' | 'succeeded' | 'failed'
  forkError?: string
  forkAttempt?: number
  forkProgress?: import('@matou/domain').ForkProgress
  providerSpawnRevision?: number
  canFork: boolean
  title: string
  cwd: string
  git?: SessionGitState
  sharedWorkingDirectory?: boolean
  worktree?: { branch: string; path: string; shared: boolean }
  environment?: SessionEnvironment
  hasOwnedWorktree?: boolean
  activeChildCount: number
  stoppedChildCount: number
  childModeCounts: { shell: number; claudeCode: number }
  latestLines: string[]
  siblingCreatedSeq?: number
  lastUserInteractionSeq: number
  lastActivityAt?: number
  archivedAt?: number
  detachedWindowId?: string
}
export interface SessionGraphView {
  sceneId: string
  runtimeGeneration?: string
  eventSequence?: number
  layoutRevision?: number
  focusedSessionId?: string
  nodes: SessionGraphNodeView[]
  edges: Array<{
    parentSessionId: string
    childSessionId: string
    relationKind: 'derived-from' | 'forked-from'
    createdAt: number
  }>
}
export type HudPermissionMode = 'default' | 'auto' | 'acceptEdits' | 'plan' | 'bypassPermissions'
export type HudModelStrategy = 'opusplan' | 'claude-opus-4-6' | 'claude-sonnet-4-6'
export interface HudUsageWindow { label: string; percent: number; resetsAt?: number }
export interface HudToolCount { name: string; count: number }
export interface HudToolActivity { name: string; target?: string; status: 'running' | 'completed' | 'error' }
export interface HudConfigCounts {
  instructionFiles: number
  projectInstructionFileExists?: boolean
  mcpServers: number
  hooks: number
  mcpServerNames?: string[]
  hookNames?: string[]
}
export interface SessionHudView {
  sessionId: string
  mode: 'shell' | 'agent'
  shell?: string
  cwd?: string
  gitBranch?: string
  gitDirty?: boolean
  startedAt: number
  permissionMode?: HudPermissionMode
  modelStrategy?: HudModelStrategy
  model?: string
  sessionName?: string
  contextPercent?: number
  contextWindowSize?: number
  taskStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  teamRole?: string
  teamStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  subagentCount?: number
  subagents?: string[]
  runningTools?: Array<{ name: string; target?: string }>
  toolCounts?: HudToolCount[]
  lastTool?: HudToolActivity
  usageWindows?: HudUsageWindow[]
  configCounts?: HudConfigCounts
  mcpErrors?: string[]
  todos?: Array<{ content: string; status: 'pending' | 'in_progress' | 'completed' }>
  resumable?: boolean
}
export interface SceneNodeView {
  id: string
  sceneId: string
  parentNodeId?: string
  kind: 'root' | 'split' | 'mount' | 'group'
  direction?: 'horizontal' | 'vertical'
  ordinal: number
}
export interface SessionMountView {
  id: string
  sceneId: string
  sceneNodeId?: string
  sceneWindowId?: string
  sessionId: string
}
export interface SceneSnapshotView {
  scene: SceneView
  nodes: SceneNodeView[]
  mounts: SessionMountView[]
  windows: Array<{ id: string; sceneId: string; state: 'attached' | 'detached' | 'closed' }>
  geometry?: Array<{
    sceneId: string
    ownerKey: string
    layoutRevision: number
    geometry: { ratio?: number } | Record<string, unknown>
    now: number
  }>
}
export interface WorkspacePathView {
  workspaceId: string
  status: 'valid' | 'invalid'
  reason: '' | 'missing' | 'not-directory' | 'no-access' | 'unknown'
}
export interface NavigationView {
  windowId: string
  activeWorkspaceId?: string
  taskByWorkspace: Record<string, string>
  sceneByTask: Record<string, string>
  sessionByScene: Record<string, string>
}
export interface HierarchyProjection {
  windowId: string
  workspaces: WorkspaceView[]
  tasks: TaskView[]
  scenes: SceneView[]
  closedScenes?: SceneView[]
  sceneSnapshots?: SceneSnapshotView[]
  sessions: SessionView[]
  pathStates: WorkspacePathView[]
  navigation: NavigationView
  taskPlacements: Array<{ windowId: string; taskId: string; ordinal: number }>
  unreadByTask?: Record<string, number>
  sessionHuds?: SessionHudView[]
  sessionGraphs?: Record<string, SessionGraphView>
}

export interface HierarchyCommands {
  activateWorkspace(workspaceId: string): unknown
  createWorkspace(path: string): unknown
  renameWorkspace(workspaceId: string, name: string): unknown
  relinkWorkspace(workspaceId: string, rootDirectory: string): unknown
  removeWorkspace(workspaceId: string): unknown
  setWorkspacePinned(workspaceId: string, pinned: boolean): unknown
  reorderPinnedWorkspace(workspaceId: string, beforeWorkspaceId?: string): unknown
  activateTask(taskId: string): unknown
  createTask(workspaceId: string): unknown
  renameTask(taskId: string, title: string): unknown
  renameSession?(sessionId: string, title: string): unknown
  restoreSessionAutoTitle?(sessionId: string): unknown
  reorderTask(workspaceId: string, taskId: string, beforeTaskId?: string): unknown
  moveTaskOnBoard?(workspaceId: string, taskId: string, status: 'planned' | 'active' | 'blocked' | 'completed', beforeTaskId?: string): unknown
  deleteTask(taskId: string): unknown
  setTaskPinned(taskId: string, pinned: boolean): unknown
  reorderPinnedTask(workspaceId: string, taskId: string, beforeTaskId?: string): unknown
  activateScene(sceneId: string): unknown
  createScene(taskId: string): unknown
  renameScene(sceneId: string, name: string): unknown
  reorderScene(sceneId: string, beforeSceneId?: string): unknown
  closeScene(sceneId: string, confirmed?: boolean): unknown
  reopenScene?(sceneId: string): unknown
  splitSession(sceneId: string, sessionId: string, direction: 'horizontal' | 'vertical'): unknown
  forkSession(sceneId: string, sessionId: string): unknown
  createCanvas(taskId: string): unknown
  createShellSibling(sceneId: string, sessionId: string, parentSessionId?: string): unknown
  createForkChild(
    sceneId: string, sessionId: string, name: string,
    worktreeMode: 'current' | 'new', submissionKey: string
  ): unknown
  createForkSibling(
    sceneId: string, sessionId: string, name: string,
    worktreeMode: 'current' | 'new', submissionKey: string
  ): unknown
  createForkPeer(
    sceneId: string, sessionId: string, name: string,
    worktreeMode: 'current' | 'new', submissionKey: string
  ): unknown
  retryFork(sceneId: string, sessionId: string): unknown
  removeFailedFork(sceneId: string, sessionId: string): unknown
  retryProviderRestore(sessionId: string): unknown
  startFreshProvider(sessionId: string): unknown
  listClaudeSessions(sessionId: string, query: string, searchScope?: 'metadata' | 'all'): Promise<ClaudeSessionListResult>
  getClaudeSessionDetail(sessionId: string, providerSessionId: string, query: string): Promise<ClaudeSessionDetail>
  loadClaudeSession(sessionId: string, providerSessionId: string): Promise<ClaudeSessionLoadResult>
  restartStoppedSession?(sessionId: string): unknown
  removeSessionBranch?(sceneId: string, sessionId: string, scope: RemoveNodeScope): unknown
  getSceneSessionGraph(sceneId: string): unknown
  recordSessionInteraction(sessionId: string, interactionKind: 'submit' | 'control' | 'provider-action'): unknown
  setFocusedSession(sceneId: string, sessionId: string): unknown
  putGeometry(sceneId: string, ownerKey: string, layoutRevision: number, geometry: unknown): unknown
  activateSession(sessionId: string): unknown
  deleteSession?(sessionId: string, confirmed?: boolean, preserveSceneOnLastSession?: boolean): unknown
  openSessionEnvironment(sessionId: string): Promise<SessionEnvironmentOpenResult>
  restoreSessionEnvironment(sessionId: string): Promise<SessionEnvironmentActionResult>
  locateSessionEnvironment(sessionId: string, path: string): Promise<SessionEnvironmentActionResult>
  handoffSessionEnvironment(
    sessionId: string,
    target: SessionEnvironmentTarget
  ): Promise<SessionEnvironmentActionResult>
  detachSession(sceneId: string, mountId: string, sessionId: string, sceneWindowId: string): unknown
  returnSession(sceneWindowId: string): unknown
  setPermissionMode(sessionId: string, permissionMode: HudPermissionMode, respawn: boolean): unknown
  setModel(sessionId: string, modelStrategy: HudModelStrategy): unknown
}
import type { SessionEnvironment, SessionGitState } from '@matou/domain'
import type {
  ClaudeSessionDetail, ClaudeSessionListResult, ClaudeSessionLoadResult,
  RemoveNodeScope,
  SessionEnvironmentActionResult, SessionEnvironmentOpenResult,
  SessionEnvironmentTarget
} from '@matou/contracts'
export type { RemoveNodeScope } from '@matou/contracts'
