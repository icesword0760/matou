import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import type { DagLayout, DagLayoutEdge, DagLayoutNode } from './dag-layout'

export interface DagWorldBounds {
  left: number
  right: number
  top: number
  bottom: number
}

export interface DagAggregateCounts {
  running: number
  needsInput: number
  error: number
}

export interface DagAggregateItem {
  key: string
  kind: 'branch' | 'layer-overflow'
  direction: 'before' | 'after'
  branchRootId: string
  targetSessionId: string
  sessionIds: string[]
  sessionCount: number
  counts: DagAggregateCounts
  x: number
  y: number
  width: number
  height: number
  minimumDepth: number
  maximumDepth: number
}

export interface DagRenderModel {
  realNodes: DagLayoutNode[]
  aggregates: DagAggregateItem[]
  edges: DagLayoutEdge[]
}

interface AggregateGroup {
  direction: DagAggregateItem['direction']
  branchRootId: string
  members: DagLayoutNode[]
}

const DEFAULT_MAX_ITEMS = 400
const DEFAULT_MAX_EDGES = 800

/**
 * Converts the complete iterative layout into a bounded render model. Near
 * layers stay as real session cards. Every farther descendant branch (or
 * farther root branch on the ancestor side) becomes one truthful aggregate.
 */
export function buildDagRenderModel(input: {
  layout: DagLayout
  fullDepths: Set<number>
  worldBounds: DagWorldBounds
  centerWorldY: number
  previewSessionId: string
  maxItems?: number
  maxEdges?: number
}): DagRenderModel {
  const {
    layout, fullDepths, worldBounds, centerWorldY, previewSessionId,
    maxItems = DEFAULT_MAX_ITEMS, maxEdges = DEFAULT_MAX_EDGES
  } = input
  if (fullDepths.size === 0 || maxItems <= 0) return { realNodes: [], aggregates: [], edges: [] }

  const minimumFullDepth = Math.min(...fullDepths)
  const maximumFullDepth = Math.max(...fullDepths)
  const groups = aggregateFarNodes(layout, minimumFullDepth, maximumFullDepth)
  const realCandidates = [...fullDepths].sort((left, right) => left - right)
    .flatMap((depth) => layout.nodesByDepth.get(depth) ?? [])
    .filter((node) => node.sessionId === previewSessionId || intersects(node, worldBounds))
    .sort((left, right) => {
      if (left.sessionId === previewSessionId) return -1
      if (right.sessionId === previewSessionId) return 1
      return Math.abs(centerY(left) - centerWorldY) - Math.abs(centerY(right) - centerWorldY) ||
        left.depth - right.depth || left.y - right.y || left.sessionId.localeCompare(right.sessionId)
    })

  const realBudget = groups.length > 0 ? Math.max(0, maxItems - 1) : maxItems
  const realNodes = realCandidates.slice(0, realBudget)
  const aggregateBudget = Math.max(0, maxItems - realNodes.length)
  const aggregates = capAggregates(groups, aggregateBudget, centerWorldY)
  const visibleIds = new Set([
    ...realNodes.map(({ sessionId }) => sessionId),
    ...aggregates.map(({ targetSessionId }) => targetSessionId)
  ])
  const edges = layout.edges.filter((edge) =>
    visibleIds.has(edge.fromSessionId) && visibleIds.has(edge.toSessionId)
  ).slice(0, maxEdges)

  return { realNodes, aggregates, edges }
}

function aggregateFarNodes(
  layout: DagLayout,
  minimumFullDepth: number,
  maximumFullDepth: number
): AggregateGroup[] {
  const rootBySessionId = new Map<string, string>()
  const descendantBranchBySessionId = new Map<string, string>()
  const groups = new Map<string, AggregateGroup>()

  for (const positioned of layout.nodes) {
    const parentSessionId = positioned.node.parentSessionId
    rootBySessionId.set(positioned.sessionId,
      parentSessionId ? rootBySessionId.get(parentSessionId) ?? parentSessionId : positioned.sessionId)

    if (positioned.depth > maximumFullDepth) {
      const branchRootId = positioned.depth === maximumFullDepth + 1
        ? positioned.sessionId
        : (parentSessionId ? descendantBranchBySessionId.get(parentSessionId) : undefined) ?? positioned.sessionId
      descendantBranchBySessionId.set(positioned.sessionId, branchRootId)
      addToGroup(groups, 'after', branchRootId, positioned)
    } else if (positioned.depth < minimumFullDepth) {
      addToGroup(groups, 'before', rootBySessionId.get(positioned.sessionId) ?? positioned.sessionId, positioned)
    }
  }

  return [...groups.values()].sort((left, right) =>
    left.direction.localeCompare(right.direction) || left.branchRootId.localeCompare(right.branchRootId)
  )
}

function addToGroup(
  groups: Map<string, AggregateGroup>,
  direction: DagAggregateItem['direction'],
  branchRootId: string,
  member: DagLayoutNode
) {
  const key = `${direction}:${branchRootId}`
  const existing = groups.get(key)
  if (existing) existing.members.push(member)
  else groups.set(key, { direction, branchRootId, members: [member] })
}

function capAggregates(
  groups: AggregateGroup[],
  budget: number,
  centerWorldY: number
): DagAggregateItem[] {
  if (budget <= 0 || groups.length === 0) return []
  const sorted = groups.map((group) => toAggregate(group, centerWorldY))
    .sort((left, right) =>
      Math.abs(centerY(left) - centerWorldY) - Math.abs(centerY(right) - centerWorldY) ||
      left.key.localeCompare(right.key)
    )
  if (sorted.length <= budget) return sorted
  if (budget === 1) return [mergeAggregates(sorted, centerWorldY)]
  return [...sorted.slice(0, budget - 1), mergeAggregates(sorted.slice(budget - 1), centerWorldY)]
}

function toAggregate(group: AggregateGroup, centerWorldY: number): DagAggregateItem {
  const target = chooseTarget(group, centerWorldY)
  const depths = group.members.map(({ depth }) => depth)
  const sessionIds = group.members.map(({ sessionId }) => sessionId)
  return {
    key: `aggregate:${group.direction}:${group.branchRootId}`,
    kind: 'branch',
    direction: group.direction,
    branchRootId: group.branchRootId,
    targetSessionId: target.sessionId,
    sessionIds,
    sessionCount: sessionIds.length,
    counts: countStatuses(group.members.map(({ node }) => node)),
    x: target.x,
    y: target.y,
    width: target.width,
    height: target.height,
    minimumDepth: Math.min(...depths),
    maximumDepth: Math.max(...depths)
  }
}

function mergeAggregates(items: DagAggregateItem[], centerWorldY: number): DagAggregateItem {
  const target = items.reduce((nearest, item) =>
    Math.abs(centerY(item) - centerWorldY) < Math.abs(centerY(nearest) - centerWorldY) ? item : nearest
  )
  const direction = items.every((item) => item.direction === items[0]!.direction)
    ? items[0]!.direction
    : target.direction
  const minimumDepth = Math.min(...items.map(({ minimumDepth }) => minimumDepth))
  const maximumDepth = Math.max(...items.map(({ maximumDepth }) => maximumDepth))
  const sessionIds = items.flatMap(({ sessionIds }) => sessionIds)
  return {
    key: `aggregate:${direction}:layer-overflow:${minimumDepth}-${maximumDepth}`,
    kind: 'layer-overflow',
    direction,
    branchRootId: `layer:${minimumDepth}-${maximumDepth}`,
    targetSessionId: target.targetSessionId,
    sessionIds,
    sessionCount: sessionIds.length,
    counts: items.reduce((counts, item) => ({
      running: counts.running + item.counts.running,
      needsInput: counts.needsInput + item.counts.needsInput,
      error: counts.error + item.counts.error
    }), emptyCounts()),
    x: target.x,
    y: target.y,
    width: target.width,
    height: target.height,
    minimumDepth,
    maximumDepth
  }
}

function chooseTarget(group: AggregateGroup, centerWorldY: number): DagLayoutNode {
  const boundaryDepth = group.direction === 'after'
    ? Math.min(...group.members.map(({ depth }) => depth))
    : Math.max(...group.members.map(({ depth }) => depth))
  return group.members.filter(({ depth }) => depth === boundaryDepth)
    .reduce((nearest, member) =>
      Math.abs(centerY(member) - centerWorldY) < Math.abs(centerY(nearest) - centerWorldY) ? member : nearest
    )
}

function countStatuses(nodes: SessionGraphNodeView[]): DagAggregateCounts {
  return nodes.reduce((counts, node) => {
    const status = node.archivedAt === undefined ? node.workStatus : 'exited'
    if (status === 'running' || status === 'starting') counts.running += 1
    else if (status === 'needs-input') counts.needsInput += 1
    else if (status === 'error') counts.error += 1
    return counts
  }, emptyCounts())
}

function emptyCounts(): DagAggregateCounts {
  return { running: 0, needsInput: 0, error: 0 }
}

function intersects(node: DagLayoutNode, bounds: DagWorldBounds): boolean {
  return node.x + node.width >= bounds.left && node.x <= bounds.right &&
    node.y + node.height >= bounds.top && node.y <= bounds.bottom
}

function centerY(item: { y: number; height: number }): number {
  return item.y + item.height / 2
}
