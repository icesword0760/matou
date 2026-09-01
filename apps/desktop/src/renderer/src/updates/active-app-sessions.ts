import type { SessionGraphView } from '../hierarchy/hierarchy-types'

const ACTIVE_WORK_STATUSES = new Set(['starting', 'running', 'needs-input'])

export function activeAppSessionCount(
  graphs: Record<string, SessionGraphView> | undefined
): number {
  const active = new Set<string>()
  for (const graph of Object.values(graphs ?? {})) {
    for (const node of graph.nodes) {
      if (node.archivedAt === undefined && ACTIVE_WORK_STATUSES.has(node.workStatus)) {
        active.add(node.sessionId)
      }
    }
  }
  return active.size
}
