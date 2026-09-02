import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'

export interface SessionGraphIndex {
  byId: ReadonlyMap<string, SessionGraphNodeView>
  childrenByParent: ReadonlyMap<string, readonly SessionGraphNodeView[]>
  roots: readonly SessionGraphNodeView[]
  childrenOf(parentSessionId?: string): SessionGraphNodeView[]
  descendantsOf(sessionId: string): readonly SessionGraphNodeView[]
  descendantSummaryOf(sessionId: string): SessionDescendantSummary
}

export interface SessionDescendantSummary {
  count: number
  running: number
  needsInput: number
}

const EMPTY_SUMMARY: SessionDescendantSummary = { count: 0, running: 0, needsInput: 0 }

const indexesByNodes = new WeakMap<object, SessionGraphIndex>()

export function indexSessionGraph(
  nodes: readonly SessionGraphNodeView[]
): SessionGraphIndex {
  const cached = indexesByNodes.get(nodes)
  if (cached) return cached

  const byId = new Map<string, SessionGraphNodeView>()
  const childrenByParent = new Map<string, SessionGraphNodeView[]>()
  const roots: SessionGraphNodeView[] = []
  for (const node of nodes) {
    if (byId.has(node.sessionId)) {
      throw new Error(`Duplicate session graph node: ${node.sessionId}`)
    }
    byId.set(node.sessionId, node)
    if (node.parentSessionId === undefined) {
      roots.push(node)
      continue
    }
    const children = childrenByParent.get(node.parentSessionId)
    if (children) children.push(node)
    else childrenByParent.set(node.parentSessionId, [node])
  }

  assertAcyclic(nodes, childrenByParent)
  const descendantSummaryById = buildDescendantSummaries(nodes, byId, childrenByParent)

  const descendantsBySession = new Map<string, readonly SessionGraphNodeView[]>()
  const index: SessionGraphIndex = {
    byId,
    childrenByParent,
    roots,
    childrenOf(parentSessionId) {
      return parentSessionId === undefined
        ? roots
        : childrenByParent.get(parentSessionId) ?? []
    },
    descendantsOf(sessionId) {
      const cachedDescendants = descendantsBySession.get(sessionId)
      if (cachedDescendants) return cachedDescendants
      const descendants = reverseDepthFirst(childrenByParent.get(sessionId) ?? [], childrenByParent)
      descendantsBySession.set(sessionId, descendants)
      return descendants
    },
    descendantSummaryOf(sessionId) {
      return descendantSummaryById.get(sessionId) ?? EMPTY_SUMMARY
    }
  }
  indexesByNodes.set(nodes, index)
  return index
}

function buildDescendantSummaries(
  nodes: readonly SessionGraphNodeView[],
  byId: ReadonlyMap<string, SessionGraphNodeView>,
  childrenByParent: ReadonlyMap<string, readonly SessionGraphNodeView[]>
): Map<string, SessionDescendantSummary> {
  const summaries = new Map<string, SessionDescendantSummary>()
  const remainingChildren = new Map<string, number>()
  const pending: SessionGraphNodeView[] = []
  for (const node of nodes) {
    const childCount = childrenByParent.get(node.sessionId)?.length ?? 0
    summaries.set(node.sessionId, { count: 0, running: 0, needsInput: 0 })
    remainingChildren.set(node.sessionId, childCount)
    if (childCount === 0) pending.push(node)
  }
  while (pending.length > 0) {
    const node = pending.pop()!
    if (!node.parentSessionId) continue
    const parent = byId.get(node.parentSessionId)
    if (!parent) continue
    const childSummary = summaries.get(node.sessionId)!
    const parentSummary = summaries.get(parent.sessionId)!
    parentSummary.count += childSummary.count + 1
    parentSummary.running += childSummary.running + (
      node.workStatus === 'running' || node.workStatus === 'starting' ? 1 : 0
    )
    parentSummary.needsInput += childSummary.needsInput + (
      node.workStatus === 'needs-input' ? 1 : 0
    )
    const remaining = remainingChildren.get(parent.sessionId)! - 1
    remainingChildren.set(parent.sessionId, remaining)
    if (remaining === 0) pending.push(parent)
  }
  return summaries
}

function assertAcyclic(
  nodes: readonly SessionGraphNodeView[],
  childrenByParent: ReadonlyMap<string, readonly SessionGraphNodeView[]>
): void {
  const state = new Map<string, 'visiting' | 'visited'>()
  for (const start of nodes) {
    if (state.has(start.sessionId)) continue
    const stack: Array<{ node: SessionGraphNodeView; nextChildIndex: number }> = [
      { node: start, nextChildIndex: 0 }
    ]
    state.set(start.sessionId, 'visiting')
    while (stack.length > 0) {
      const frame = stack[stack.length - 1]!
      const children = childrenByParent.get(frame.node.sessionId) ?? []
      const child = children[frame.nextChildIndex]
      if (!child) {
        state.set(frame.node.sessionId, 'visited')
        stack.pop()
        continue
      }
      frame.nextChildIndex += 1
      const childState = state.get(child.sessionId)
      if (childState === 'visited') continue
      if (childState === 'visiting') {
        const cycleStart = stack.findIndex(({ node }) => node.sessionId === child.sessionId)
        const cycle = stack.slice(cycleStart).map(({ node }) => node.sessionId)
        cycle.push(child.sessionId)
        throw new Error(`Session graph cycle detected: ${cycle.join(' -> ')}`)
      }
      state.set(child.sessionId, 'visiting')
      stack.push({ node: child, nextChildIndex: 0 })
    }
  }
}

function reverseDepthFirst(
  initial: readonly SessionGraphNodeView[],
  childrenByParent: ReadonlyMap<string, readonly SessionGraphNodeView[]>
): SessionGraphNodeView[] {
  const result: SessionGraphNodeView[] = []
  const stack: Array<{ node: SessionGraphNodeView; nextChildIndex: number }> = []
  for (const node of initial) stack.push({ node, nextChildIndex: Number.POSITIVE_INFINITY })
  while (stack.length > 0) {
    const frame = stack[stack.length - 1]!
    const children = childrenByParent.get(frame.node.sessionId) ?? []
    if (frame.nextChildIndex === Number.POSITIVE_INFINITY) {
      result.push(frame.node)
      frame.nextChildIndex = children.length - 1
      continue
    }
    if (frame.nextChildIndex >= 0) {
      const child = children[frame.nextChildIndex]!
      frame.nextChildIndex -= 1
      stack.push({ node: child, nextChildIndex: Number.POSITIVE_INFINITY })
      continue
    }
    stack.pop()
  }
  return result
}
