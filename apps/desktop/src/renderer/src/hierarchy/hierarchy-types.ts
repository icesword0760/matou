export interface WorkspaceView { id: string; name: string; rootDirectory: string }
export interface TaskView { id: string; workspaceId: string; title: string }
export interface SceneView { id: string; taskId: string; name: string; sortKey?: string; titlePinned?: boolean }
export interface SessionView { id: string; taskId: string; title: string; kind?: string }
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
}
