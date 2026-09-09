import type { CatalogShape } from '../catalog'

export const hierarchyZhCN = {
  // workspace-path-service.ts
  workspacePathInvalid: '工作区目录不可用，请先在本地恢复原路径，或移出该工作区',

  // hierarchy-application-service.ts — default entity names, read when a row is created
  defaultTask: '默认',
  newTask: '新事项',
  newTaskNumbered: (suffix: number) => `新事项 ${suffix}`,

  // hierarchy-application-service.ts — workspace rules
  workspaceNameFollowsDirectory: '工作空间名称跟随目录名称',
  defaultWorkspaceIsHomeDirectory: '默认工作空间始终指向 macOS 用户目录',
  workspaceDirectoryTaken: '该目录已经属于另一个工作空间',
  defaultWorkspaceKept: '默认工作空间会保留在侧栏中',

  /** Thrown by hierarchy-application-service.ts and session-canvas-service.ts alike. */
  duplicateSceneName: '当前事项下已存在同名页签'
}

export const hierarchyEn: CatalogShape<typeof hierarchyZhCN> = {
  workspacePathInvalid:
    'The workspace directory is unavailable. Restore the original path locally, or remove the workspace.',

  defaultTask: 'Default',
  newTask: 'New task',
  newTaskNumbered: (suffix) => `New task ${suffix}`,

  workspaceNameFollowsDirectory: 'The workspace name follows its directory name',
  defaultWorkspaceIsHomeDirectory: 'The default workspace always points at the macOS home directory',
  workspaceDirectoryTaken: 'That directory already belongs to another workspace',
  defaultWorkspaceKept: 'The default workspace stays in the sidebar',

  duplicateSceneName: 'A canvas with this name already exists in this task'
}
