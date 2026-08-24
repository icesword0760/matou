import { randomUUID } from 'node:crypto'

export interface HierarchyIds {
  workspaceId: string
  executionContextId: string
  taskId: string
  sceneId: string
  rootNodeId: string
  sessionId: string
  mountId: string
}

export function createHierarchyIds(): HierarchyIds {
  return {
    workspaceId: randomUUID(),
    executionContextId: randomUUID(),
    taskId: randomUUID(),
    sceneId: randomUUID(),
    rootNodeId: randomUUID(),
    sessionId: randomUUID(),
    mountId: randomUUID()
  }
}
