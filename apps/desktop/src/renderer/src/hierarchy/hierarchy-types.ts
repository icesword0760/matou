export interface WorkspaceView { id: string; name: string; rootDirectory: string }
export interface TaskView { id: string; workspaceId: string; title: string }
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
}

export interface HierarchyCommands {
  activateWorkspace(workspaceId: string): unknown
  createWorkspace(path: string): unknown
  renameWorkspace(workspaceId: string, name: string): unknown
  removeWorkspace(workspaceId: string): unknown
  activateTask(taskId: string): unknown
  createTask(workspaceId: string): unknown
  renameTask(taskId: string, title: string): unknown
  reorderTask(taskId: string, beforeTaskId?: string): unknown
  deleteTask(taskId: string): unknown
  activateScene(sceneId: string): unknown
  createScene(taskId: string): unknown
  renameScene(sceneId: string, name: string): unknown
  reorderScene(sceneId: string, beforeSceneId?: string): unknown
  closeScene(sceneId: string, confirmed?: boolean): unknown
  splitSession(sceneId: string, sessionId: string, direction: 'horizontal' | 'vertical'): unknown
  activateSession(sessionId: string): unknown
  deleteSession(sessionId: string, confirmed?: boolean): unknown
  detachSession(sceneId: string, mountId: string, sessionId: string, sceneWindowId: string): unknown
  returnSession(sceneWindowId: string): unknown
}
