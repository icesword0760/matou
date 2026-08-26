export interface WorkspaceView {
  id: string; name: string; rootDirectory: string
  isDefault?: boolean; isPinned?: boolean; pinSortKey?: string; lastOpenedAt?: number
}
export interface TaskView {
  id: string; workspaceId: string; title: string; sortKey?: string
  isPinned?: boolean; pinSortKey?: string; lastOpenedAt?: number; createdAt?: number
}
export interface SceneView {
  id: string
  taskId: string
  name: string
  sortKey?: string
  titlePinned?: boolean
  rootNodeId?: string
  layoutRevision?: number
}
export interface SessionView {
  id: string
  taskId: string
  title: string
  kind?: 'shell' | 'claude-code' | 'codex' | string
  status?: string
  executionContextId?: string
}
export type HudPermissionMode = 'default' | 'acceptEdits' | 'plan' | 'bypassPermissions'
export type HudModelStrategy = 'opusplan' | 'claude-opus-4-6' | 'claude-sonnet-4-6'
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
  contextPercent?: number
  taskStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  teamRole?: string
  teamStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  subagentCount?: number
  runningTools?: Array<{ name: string; target?: string }>
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
  sceneSnapshots?: SceneSnapshotView[]
  sessions: SessionView[]
  pathStates: WorkspacePathView[]
  navigation: NavigationView
  taskPlacements: Array<{ windowId: string; taskId: string; ordinal: number }>
  unreadByTask?: Record<string, number>
  sessionHuds?: SessionHudView[]
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
  reorderTask(workspaceId: string, taskId: string, beforeTaskId?: string): unknown
  deleteTask(taskId: string): unknown
  setTaskPinned(taskId: string, pinned: boolean): unknown
  reorderPinnedTask(workspaceId: string, taskId: string, beforeTaskId?: string): unknown
  activateScene(sceneId: string): unknown
  createScene(taskId: string): unknown
  renameScene(sceneId: string, name: string): unknown
  reorderScene(sceneId: string, beforeSceneId?: string): unknown
  closeScene(sceneId: string, confirmed?: boolean): unknown
  splitSession(sceneId: string, sessionId: string, direction: 'horizontal' | 'vertical'): unknown
  forkSession(sceneId: string, sessionId: string): unknown
  putGeometry(sceneId: string, ownerKey: string, layoutRevision: number, geometry: unknown): unknown
  activateSession(sessionId: string): unknown
  deleteSession(sessionId: string, confirmed?: boolean): unknown
  detachSession(sceneId: string, mountId: string, sessionId: string, sceneWindowId: string): unknown
  returnSession(sceneWindowId: string): unknown
  setPermissionMode(sessionId: string, permissionMode: HudPermissionMode, respawn: boolean): unknown
  setModel(sessionId: string, modelStrategy: HudModelStrategy): unknown
}
