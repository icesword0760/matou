import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'

/**
 * The foreground terminal boundary is the horizontal sibling level the user
 * is currently browsing. Card DOM may be virtualized inside that boundary,
 * but scrolling must not turn an offscreen sibling into a background Session.
 */
export function foregroundSiblingSessionIds(
  nodes: readonly SessionGraphNodeView[],
  parentSessionId: string | undefined
): string[] {
  return nodes.filter((node) =>
    node.parentSessionId === parentSessionId &&
    node.archivedAt === undefined &&
    node.currentMode !== 'agent-team-member'
  ).map(({ sessionId }) => sessionId)
}
