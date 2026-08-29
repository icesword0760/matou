import type { RuntimeClient } from '../runtime/RuntimeClient'
import type { HierarchyCommands } from './hierarchy-types'

export function createHierarchyCommands(
  client: RuntimeClient,
  windowId: string,
  afterMutation?: () => void | Promise<void>
): HierarchyCommands {
  let sequence = 0
  const command = async (type: string, input: Record<string, unknown>) => {
    const commandId = `${type}-${Date.now()}-${++sequence}`
    const result = await client.request(type as Parameters<RuntimeClient['request']>[0], {
      command: { commandId, commandType: type, requestHash: JSON.stringify(input) },
      input: { ...input, windowId, now: Date.now() }
    })
    await afterMutation?.()
    return result
  }
  return {
    activateWorkspace: (workspaceId) => command('hierarchy.activate-workspace', { workspaceId }),
    createWorkspace: (rootDirectory) => command('hierarchy.create-workspace', {
      rootDirectory, name: rootDirectory.split('/').filter(Boolean).at(-1) ?? '工作区'
    }),
    renameWorkspace: (workspaceId, name) => command('hierarchy.rename-workspace', { workspaceId, name }),
    relinkWorkspace: (workspaceId, rootDirectory) => command('hierarchy.relink-workspace', { workspaceId, rootDirectory }),
    removeWorkspace: (workspaceId) => command('hierarchy.remove-workspace', {
      workspaceId, confirmedIntent: `remove-workspace:${workspaceId}`
    }),
    setWorkspacePinned: (workspaceId, pinned) => command('hierarchy.set-workspace-pinned', { workspaceId, pinned }),
    reorderPinnedWorkspace: (workspaceId, beforeWorkspaceId) => command('hierarchy.reorder-pinned-workspace', {
      workspaceId, ...(beforeWorkspaceId ? { beforeWorkspaceId } : {})
    }),
    activateTask: (taskId) => command('hierarchy.activate-task', { taskId }),
    createTask: (workspaceId) => command('hierarchy.create-task', { workspaceId }),
    renameTask: (taskId, title) => command('hierarchy.rename-task', { taskId, title }),
    reorderTask: (workspaceId, taskId, beforeTaskId) => command('hierarchy.reorder-task', {
      workspaceId, taskId, ...(beforeTaskId ? { beforeTaskId } : {})
    }),
    deleteTask: (taskId) => command('hierarchy.delete-task', {
      taskId, confirmedIntent: `delete-task:${taskId}`
    }),
    setTaskPinned: (taskId, pinned) => command('hierarchy.set-task-pinned', { taskId, pinned }),
    reorderPinnedTask: (workspaceId, taskId, beforeTaskId) => command('hierarchy.reorder-pinned-task', {
      workspaceId, taskId, ...(beforeTaskId ? { beforeTaskId } : {})
    }),
    activateScene: (sceneId) => command('hierarchy.activate-scene', { sceneId }),
    createScene: (taskId) => command('hierarchy.create-scene', { taskId }),
    renameScene: (sceneId, name) => command('hierarchy.rename-scene', { sceneId, name }),
    reorderScene: (sceneId, beforeSceneId) => command('hierarchy.reorder-scene', {
      sceneId, ...(beforeSceneId ? { beforeSceneId } : {})
    }),
    closeScene: (sceneId, confirmed = false) => command('hierarchy.close-scene', {
      sceneId, ...(confirmed ? { confirmedIntent: `close-scene:${sceneId}` } : {})
    }),
    reopenScene: (sceneId) => command('hierarchy.reopen-scene', { sceneId }),
    splitSession: (sceneId, sourceSessionId, direction) => command('hierarchy.split-session', {
      sceneId, sourceSessionId, direction
    }),
    forkSession: (sceneId, sourceSessionId) => command('hierarchy.fork-session', {
      sceneId, sourceSessionId
    }),
    createCanvas: (taskId) => command('hierarchy.create-canvas', { taskId }),
    createShellSibling: (sceneId, sourceSessionId, parentSessionId) => command('hierarchy.create-shell-sibling', {
      sceneId, sourceSessionId, ...(parentSessionId ? { parentSessionId } : {})
    }),
    createForkChild: (sceneId, sourceSessionId, name, worktreeMode) => command('hierarchy.create-fork-child', {
      sceneId, sourceSessionId, name, worktreeMode
    }),
    createForkSibling: (sceneId, sourceSessionId, name, worktreeMode) => command('hierarchy.create-fork-sibling', {
      sceneId, sourceSessionId, name, worktreeMode
    }),
    retryFork: (sessionId) => command('hierarchy.retry-fork', { sessionId }),
    removeFailedFork: (sessionId) => command('hierarchy.remove-failed-fork', { sessionId }),
    retryProviderRestore: (sessionId) => command('hierarchy.retry-provider-restore', { sessionId }),
    reopenHistoricalSession: (sessionId) => command('hierarchy.reopen-historical-session', { sessionId }),
    removeHistoricalSession: (sceneId, sessionId, includeDescendants) => command(
      'hierarchy.remove-historical-session', { sceneId, sessionId, includeDescendants }
    ),
    getSceneSessionGraph: (sceneId) => client.request('hierarchy.get-scene-session-graph', {
      sceneId, windowId
    }),
    recordSessionInteraction: (sessionId, interactionKind) => command('hierarchy.record-session-interaction', {
      sessionId, interactionKind
    }),
    setFocusedSession: (sceneId, sessionId) => command('hierarchy.set-focused-session', {
      sceneId, sessionId
    }),
    putGeometry: (sceneId, ownerKey, layoutRevision, geometry) => client.request('geometry.put', {
      sceneId, ownerKey, layoutRevision, geometry, now: Date.now()
    }),
    activateSession: (sessionId) => command('hierarchy.activate-session', { sessionId }),
    deleteSession: (sessionId, confirmed = false) => command('hierarchy.delete-session', {
      sessionId, ...(confirmed ? { confirmedIntent: `delete-session:${sessionId}` } : {})
    }),
    detachSession: (sceneId, mountId, sessionId, sceneWindowId) => command('hierarchy.detach-session', {
      sceneId, mountId, sessionId, sceneWindowId, nativeWindowKey: sceneWindowId
    }),
    returnSession: (sceneWindowId) => command('hierarchy.return-session', { sceneWindowId })
    ,
    setPermissionMode: (sessionId, permissionMode, respawn) => command('session.set-permission-mode', {
      sessionId, provider: 'claude-code', permissionMode, respawn
    }),
    setModel: (sessionId, modelStrategy) => command('session.set-model', {
      sessionId, modelStrategy
    })
  }
}
