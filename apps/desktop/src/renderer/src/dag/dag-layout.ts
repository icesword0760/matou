import type { SessionGraphNodeView, SessionGraphView } from '../hierarchy/hierarchy-types'

export interface DagLayoutNode {
  sessionId: string
  depth: number
  x: number
  y: number
  width: number
  height: number
  node: SessionGraphNodeView
}

export interface DagLayoutEdge {
  fromSessionId: string
  toSessionId: string
  relationKind: 'derived-from' | 'forked-from'
  from: { x: number; y: number }
  to: { x: number; y: number }
}

export interface DagLayout {
  nodes: DagLayoutNode[]
  edges: DagLayoutEdge[]
  nodeById: Map<string, DagLayoutNode>
  nodesByDepth: Map<number, DagLayoutNode[]>
  width: number
  height: number
  depthCount: number
}

const NODE_WIDTH = 260
const NODE_BASE_HEIGHT = 174
const TITLE_LINE_HEIGHT = 18
const TITLE_UNITS_PER_LINE = 18
const X_GAP = 110
const Y_GAP = 26

export function layoutGraph(graph: SessionGraphView): DagLayout {
  const byId = new Map(graph.nodes.map((node) => [node.sessionId, node]))
  const depthMemo = new Map<string, number>()
  const depthFor = (node: SessionGraphNodeView): number => {
    const known = depthMemo.get(node.sessionId)
    if (known !== undefined) return known
    const path: SessionGraphNodeView[] = []
    const visiting = new Set<string>()
    let cursor: SessionGraphNodeView | undefined = node
    let baseDepth = 0
    while (cursor) {
      const memo = depthMemo.get(cursor.sessionId)
      if (memo !== undefined) {
        baseDepth = memo
        break
      }
      if (visiting.has(cursor.sessionId)) {
        baseDepth = 0
        break
      }
      visiting.add(cursor.sessionId)
      path.push(cursor)
      cursor = cursor.parentSessionId ? byId.get(cursor.parentSessionId) : undefined
    }
    if (!cursor) baseDepth = -1
    for (let index = path.length - 1; index >= 0; index -= 1) {
      baseDepth += 1
      depthMemo.set(path[index]!.sessionId, baseDepth)
    }
    return depthMemo.get(node.sessionId) ?? 0
  }
  const groups = new Map<number, SessionGraphNodeView[]>()
  for (const node of graph.nodes) {
    const depth = depthFor(node)
    const peers = groups.get(depth) ?? []
    peers.push(node)
    groups.set(depth, peers)
  }
  for (const peers of groups.values()) peers.sort(stableNodeOrder)
  const columns = [...groups.entries()].sort(([left], [right]) => left - right).map(([depth, peers]) => {
    const heights = peers.map(({ title }) => nodeHeight(title))
    const height = heights.reduce((sum, value) => sum + value, 0) + Math.max(0, peers.length - 1) * Y_GAP
    return { depth, peers, heights, height }
  })
  const maxColumnHeight = Math.max(NODE_BASE_HEIGHT, ...columns.map(({ height }) => height))
  const nodes = columns.flatMap(({ depth, peers, heights, height }) => {
    let y = 50 + (maxColumnHeight - height) / 2
    return peers.map((node, index) => {
      const nodeHeight = heights[index]!
      const positioned = {
        sessionId: node.sessionId, depth,
        x: 50 + depth * (NODE_WIDTH + X_GAP), y,
        width: NODE_WIDTH, height: nodeHeight, node
      }
      y += nodeHeight + Y_GAP
      return positioned
    })
  })
  const positioned = new Map(nodes.map((node) => [node.sessionId, node]))
  const nodesByDepth = new Map<number, DagLayoutNode[]>()
  for (const node of nodes) {
    const peers = nodesByDepth.get(node.depth) ?? []
    peers.push(node)
    nodesByDepth.set(node.depth, peers)
  }
  const edges = graph.edges.flatMap((edge) => {
    const parent = positioned.get(edge.parentSessionId)
    const child = positioned.get(edge.childSessionId)
    if (!parent || !child) return []
    return [{
      fromSessionId: parent.sessionId, toSessionId: child.sessionId,
      relationKind: edge.relationKind,
      from: { x: parent.x + parent.width, y: parent.y + parent.height / 2 },
      to: { x: child.x, y: child.y + child.height / 2 }
    }]
  })
  const depthCount = Math.max(1, ...nodes.map(({ depth }) => depth + 1))
  return {
    nodes, edges, nodeById: positioned, nodesByDepth, depthCount,
    width: 100 + depthCount * NODE_WIDTH + Math.max(0, depthCount - 1) * X_GAP,
    height: 100 + maxColumnHeight
  }
}

function nodeHeight(title: string): number {
  const units = [...title].reduce((sum, character) => sum + (isWideCharacter(character) ? 1 : .55), 0)
  const lines = Math.max(1, Math.ceil(units / TITLE_UNITS_PER_LINE))
  return NODE_BASE_HEIGHT + (lines - 1) * TITLE_LINE_HEIGHT
}

function isWideCharacter(character: string): boolean {
  return /[\u2e80-\u9fff\uf900-\ufaff\uff01-\uff60\uffe0-\uffe6]/u.test(character)
}

export function visibleLayers(layout: DagLayout, focusSessionId: string, radius = 1): {
  fullDepths: number[]
  ghostDepths: number[]
} {
  const focusDepth = layout.nodeById.get(focusSessionId)?.depth ?? 0
  const all = Array.from({ length: layout.depthCount }, (_, depth) => depth)
  const fullDepths = all.filter((depth) => Math.abs(depth - focusDepth) <= radius)
  return { fullDepths, ghostDepths: all.filter((depth) => !fullDepths.includes(depth)) }
}

export function searchGraph(graph: SessionGraphView, query: string): SessionGraphNodeView[] {
  const normalized = query.trim().toLocaleLowerCase()
  if (!normalized) return []
  return graph.nodes.map((node) => ({ node, score: searchScore(node, normalized) }))
    .filter(({ score }) => score > 0)
    .sort((left, right) => right.score - left.score || stableNodeOrder(left.node, right.node))
    .map(({ node }) => node)
}

function stableNodeOrder(left: SessionGraphNodeView, right: SessionGraphNodeView): number {
  return (left.siblingCreatedSeq ?? Number.MAX_SAFE_INTEGER) -
    (right.siblingCreatedSeq ?? Number.MAX_SAFE_INTEGER) || left.sessionId.localeCompare(right.sessionId)
}

function searchScore(node: SessionGraphNodeView, query: string): number {
  const title = node.title.toLocaleLowerCase()
  if (title === query) return 100
  if (title.startsWith(query)) return 80
  if (title.includes(query)) return 60
  if (node.worktree?.branch.toLocaleLowerCase().includes(query)) return 45
  if (node.cwd.toLocaleLowerCase().includes(query)) return 30
  if (node.latestLines.join('\n').toLocaleLowerCase().includes(query)) return 20
  return 0
}
