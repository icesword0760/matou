export interface HostNavigationPath {
  /** Main Renderer connection that owns and executes this navigation. */
  routeWindowId: string
  /** Visible native destination; differs from routeWindowId for a detached terminal. */
  targetWindowId: string
  workspaceId: string
  taskId: string
  sceneId: string
  sessionId?: string
}
