import type { RuntimeClient } from '../runtime/RuntimeClient'
import type { HierarchyCommands } from './hierarchy-types'

export function createHierarchyCommands(client: RuntimeClient, windowId: string): HierarchyCommands {
  let sequence = 0
  const command = (type: string, input: Record<string, unknown>) => {
    const commandId = `${type}-${Date.now()}-${++sequence}`
    return client.request(type as Parameters<RuntimeClient['request']>[0], {
      command: { commandId, commandType: type, requestHash: JSON.stringify(input) },
      input: { ...input, windowId, now: Date.now() }
    })
  }
  return {
    activateWorkspace: (workspaceId) => command('hierarchy.activate-workspace', { workspaceId }),
    createWorkspace: (rootDirectory) => command('hierarchy.create-workspace', {
      rootDirectory, name: rootDirectory.split('/').filter(Boolean).at(-1) ?? '工作区'
    }),
    renameWorkspace: (workspaceId, name) => command('hierarchy.rename-workspace', { workspaceId, name }),
    removeWorkspace: (workspaceId) => command('hierarchy.remove-workspace', {
      workspaceId, confirmedIntent: `remove-workspace:${workspaceId}`
    }),
    activateTask: (taskId) => command('hierarchy.activate-task', { taskId }),
    createTask: (workspaceId) => command('hierarchy.create-task', { workspaceId }),
    renameTask: (taskId, title) => command('hierarchy.rename-task', { taskId, title }),
    reorderTask: (taskId, beforeTaskId) => command('hierarchy.reorder-task', {
      taskId, ...(beforeTaskId ? { beforeTaskId } : {})
    }),
    deleteTask: (taskId) => command('hierarchy.delete-task', {
      taskId, confirmedIntent: `delete-task:${taskId}`
    })
  }
}
