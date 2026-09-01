import type { RuntimeClient } from '../runtime/RuntimeClient'
import type { HierarchyCommands } from './hierarchy-types'
import type { HierarchyProjection } from './hierarchy-types'
import type {
  ClaudeSessionDetail, ClaudeSessionListResult, ClaudeSessionLoadResult,
  SessionEnvironmentActionResult, SessionEnvironmentOpenResult,
  SessionEnvironmentTarget
} from '@matou/contracts'

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
  const environmentCommand = async (
    type: 'session.environment-restore' | 'session.environment-locate' | 'session.environment-handoff',
    input: Record<string, unknown>
  ): Promise<SessionEnvironmentActionResult> => {
    const result = await command(type, input) as SessionEnvironmentActionResult
    if (result.kind === 'switch-session') {
      await command('hierarchy.activate-session', { sessionId: result.sessionId })
    }
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
    createShellSibling: async (sceneId, sourceSessionId, parentSessionId) => {
      const result = await command('hierarchy.create-shell-sibling', {
        sceneId, sourceSessionId, ...(parentSessionId ? { parentSessionId } : {})
      })
      const createdSessionId = mutationSessionId(result)
      if (createdSessionId) {
        // A mounted xterm may emit one last focus event while the new card is
        // entering the DOM. Reassert the user's create intent after the
        // authoritative mutation so every entry point opens keyboard-ready.
        await command('hierarchy.set-focused-session', { sceneId, sessionId: createdSessionId })
      }
      return result
    },
    createForkChild: (sceneId, sourceSessionId, name, worktreeMode) => command('hierarchy.create-fork-child', {
      sceneId, sourceSessionId, name, worktreeMode
    }),
    createForkSibling: (sceneId, sourceSessionId, name, worktreeMode) => command('hierarchy.create-fork-sibling', {
      sceneId, sourceSessionId, name, worktreeMode
    }),
    retryFork: (sceneId, sessionId) => command('hierarchy.retry-fork', { sceneId, sessionId }),
    removeFailedFork: (sceneId, sessionId) => command('hierarchy.remove-failed-fork', {
      sceneId, sessionId
    }),
    retryProviderRestore: (sessionId) => command('hierarchy.retry-provider-restore', { sessionId }),
    listClaudeSessions: (sessionId, query, providerSessionId) => client.request<ClaudeSessionListResult>(
      'claude-sessions.list', { sessionId, query, ...(providerSessionId ? { providerSessionId } : {}) }
    ),
    getClaudeSessionDetail: (sessionId, providerSessionId, query) => client.request<ClaudeSessionDetail>(
      'claude-sessions.detail', { sessionId, providerSessionId, query }
    ),
    loadClaudeSession: async (sessionId, providerSessionId) => {
      const result = await command('claude-sessions.load', { sessionId, providerSessionId }) as {
        load: ClaudeSessionLoadResult
      }
      return result.load
    },
    restartStoppedSession: (sessionId) => command('hierarchy.restart-stopped-session', {
      windowId, sessionId
    }),
    removeSessionBranch: (sceneId, sessionId, includeDescendants) => command(
      'hierarchy.remove-session-branch', { sceneId, sessionId, includeDescendants }
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
    openSessionEnvironment: (sessionId) => client.request<SessionEnvironmentOpenResult>(
      'session.environment-open', { sessionId }
    ),
    restoreSessionEnvironment: (sessionId) => environmentCommand(
      'session.environment-restore', { sessionId }
    ),
    locateSessionEnvironment: (sessionId, path) => environmentCommand(
      'session.environment-locate', { sessionId, path }
    ),
    handoffSessionEnvironment: (sessionId, target: SessionEnvironmentTarget) => environmentCommand(
      'session.environment-handoff', { sessionId, target }
    ),
    deleteSession: (sessionId, confirmed = false, preserveSceneOnLastSession = false) => command('hierarchy.delete-session', {
      sessionId, ...(confirmed ? { confirmedIntent: `delete-session:${sessionId}` } : {}),
      ...(preserveSceneOnLastSession ? { preserveSceneOnLastSession: true } : {})
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

export const READ_ONLY_RECOVERY_REASON = '数据库处于只读恢复模式'

export function createReadOnlyHierarchyCommands(
  base: HierarchyCommands,
  updateProjection: (update: (value: HierarchyProjection) => void) => void
): HierarchyCommands {
  const blocked = () => Promise.reject(Object.assign(
    new Error(READ_ONLY_RECOVERY_REASON), { code: 'STORAGE_READ_ONLY' }
  ))
  const navigate = (update: (value: HierarchyProjection) => void) => {
    updateProjection(update)
    return Promise.resolve()
  }
  const activateWorkspace = (workspaceId: string) => navigate((value) => {
    if (!value.workspaces.some(({ id }) => id === workspaceId)) return
    value.navigation.activeWorkspaceId = workspaceId
  })
  const activateTask = (taskId: string) => navigate((value) => {
    const task = value.tasks.find(({ id }) => id === taskId)
    if (!task) return
    value.navigation.activeWorkspaceId = task.workspaceId
    value.navigation.taskByWorkspace[task.workspaceId] = task.id
  })
  const activateScene = (sceneId: string) => navigate((value) => {
    const scene = value.scenes.find(({ id }) => id === sceneId)
    const task = scene && value.tasks.find(({ id }) => id === scene.taskId)
    if (!scene || !task) return
    value.navigation.activeWorkspaceId = task.workspaceId
    value.navigation.taskByWorkspace[task.workspaceId] = task.id
    value.navigation.sceneByTask[task.id] = scene.id
  })
  const setFocusedSession = (sceneId: string, sessionId: string) => navigate((value) => {
    const snapshot = value.sceneSnapshots?.find(({ scene }) => scene.id === sceneId)
    if (!snapshot?.mounts.some(({ sessionId: candidate }) => candidate === sessionId)) return
    value.navigation.sessionByScene[sceneId] = sessionId
    if (value.sessionGraphs?.[sceneId]) value.sessionGraphs[sceneId]!.focusedSessionId = sessionId
  })
  const activateSession = (sessionId: string) => navigate((value) => {
    const snapshot = value.sceneSnapshots?.find(({ mounts }) =>
      mounts.some(({ sessionId: candidate }) => candidate === sessionId))
    if (!snapshot) return
    const scene = value.scenes.find(({ id }) => id === snapshot.scene.id)
    const task = scene && value.tasks.find(({ id }) => id === scene.taskId)
    if (!scene || !task) return
    value.navigation.activeWorkspaceId = task.workspaceId
    value.navigation.taskByWorkspace[task.workspaceId] = task.id
    value.navigation.sceneByTask[task.id] = scene.id
    value.navigation.sessionByScene[scene.id] = sessionId
    if (value.sessionGraphs?.[scene.id]) value.sessionGraphs[scene.id]!.focusedSessionId = sessionId
  })

  return {
    ...base,
    activateWorkspace,
    activateTask,
    activateScene,
    activateSession,
    openSessionEnvironment: base.openSessionEnvironment,
    restoreSessionEnvironment: blocked,
    locateSessionEnvironment: blocked,
    handoffSessionEnvironment: blocked,
    setFocusedSession,
    createWorkspace: blocked,
    renameWorkspace: blocked,
    relinkWorkspace: blocked,
    removeWorkspace: blocked,
    setWorkspacePinned: blocked,
    reorderPinnedWorkspace: blocked,
    createTask: blocked,
    renameTask: blocked,
    reorderTask: blocked,
    deleteTask: blocked,
    setTaskPinned: blocked,
    reorderPinnedTask: blocked,
    createScene: blocked,
    renameScene: blocked,
    reorderScene: blocked,
    closeScene: blocked,
    reopenScene: blocked,
    splitSession: blocked,
    forkSession: blocked,
    createCanvas: blocked,
    createShellSibling: blocked,
    createForkChild: blocked,
    createForkSibling: blocked,
    retryFork: blocked,
    removeFailedFork: blocked,
    retryProviderRestore: blocked,
    loadClaudeSession: blocked,
    restartStoppedSession: blocked,
    removeSessionBranch: blocked,
    recordSessionInteraction: blocked,
    // Scrolling remains local and usable. Persistence resumes after storage is writable.
    putGeometry: () => Promise.resolve(),
    deleteSession: blocked,
    detachSession: blocked,
    returnSession: blocked,
    setPermissionMode: blocked,
    setModel: blocked
  }
}

function mutationSessionId(value: unknown): string | undefined {
  if (!value || typeof value !== 'object' || !('session' in value)) return undefined
  const session = value.session
  if (!session || typeof session !== 'object' || !('id' in session)) return undefined
  return typeof session.id === 'string' ? session.id : undefined
}
