import { vi } from 'vitest'

import type { HierarchyCommands, HierarchyProjection } from './hierarchy-types'

export function fixture(): HierarchyProjection {
  return {
    windowId: 'window-1',
    workspaces: [{ id: 'workspace-1', name: 'Frontend', rootDirectory: '/Users/demo/projects/frontend/app' }],
    tasks: [
      { id: 'task-a', workspaceId: 'workspace-1', title: '事项 A' },
      { id: 'task-b', workspaceId: 'workspace-1', title: '线上 bug' }
    ],
    scenes: [], sessions: [],
    pathStates: [{ workspaceId: 'workspace-1', status: 'invalid', reason: 'missing' }],
    navigation: {
      windowId: 'window-1', activeWorkspaceId: 'workspace-1',
      taskByWorkspace: { 'workspace-1': 'task-a' }, sceneByTask: {}, sessionByScene: {}
    },
    taskPlacements: []
    , unreadByTask: {}
  }
}

export function commands(): HierarchyCommands {
  return {
    activateWorkspace: vi.fn(), createWorkspace: vi.fn(), renameWorkspace: vi.fn(), relinkWorkspace: vi.fn(),
    removeWorkspace: vi.fn(), setWorkspacePinned: vi.fn(), reorderPinnedWorkspace: vi.fn(),
    activateTask: vi.fn(), createTask: vi.fn(),
    renameTask: vi.fn(), reorderTask: vi.fn(), deleteTask: vi.fn(),
    setTaskPinned: vi.fn(), reorderPinnedTask: vi.fn(),
    activateScene: vi.fn(), createScene: vi.fn(), renameScene: vi.fn(),
    reorderScene: vi.fn(), closeScene: vi.fn(), reopenScene: vi.fn(), splitSession: vi.fn(), forkSession: vi.fn(),
    createCanvas: vi.fn(), createShellSibling: vi.fn(), createForkChild: vi.fn(), createForkSibling: vi.fn(),
    createForkPeer: vi.fn(),
    retryFork: vi.fn(), removeFailedFork: vi.fn(),
    retryProviderRestore: vi.fn(), startFreshProvider: vi.fn(), listClaudeSessions: vi.fn(),
    searchClaudeSession: vi.fn(),
    getClaudeSessionDetail: vi.fn(), loadClaudeSession: vi.fn(), getSceneSessionGraph: vi.fn(),
    recordSessionInteraction: vi.fn(), setFocusedSession: vi.fn(),
    putGeometry: vi.fn(),
    activateSession: vi.fn(), detachSession: vi.fn(),
    openSessionEnvironment: vi.fn(), restoreSessionEnvironment: vi.fn(),
    locateSessionEnvironment: vi.fn(), handoffSessionEnvironment: vi.fn(),
    returnSession: vi.fn(), setPermissionMode: vi.fn(), setModel: vi.fn()
  }
}
