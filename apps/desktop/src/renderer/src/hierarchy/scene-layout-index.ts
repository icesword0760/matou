import type { LayoutNode } from '@matou/domain'

import type {
  SceneNodeView, SceneSnapshotView, SessionMountView
} from './hierarchy-types'

export interface SceneLayoutIndex {
  byId: ReadonlyMap<string, SceneNodeView>
  childrenByParent: ReadonlyMap<string, readonly SceneNodeView[]>
  mountById: ReadonlyMap<string, SessionMountView>
  mountByNode: ReadonlyMap<string, SessionMountView>
  mountBySession: ReadonlyMap<string, SessionMountView>
  windowById: ReadonlyMap<string, SceneSnapshotView['windows'][number]>
  layout: LayoutNode | undefined
  orderedSessionIds: readonly string[]
}

const indexesBySnapshot = new WeakMap<object, SceneLayoutIndex>()

export function indexSceneLayout(snapshot: SceneSnapshotView): SceneLayoutIndex {
  const cached = indexesBySnapshot.get(snapshot)
  if (cached) return cached

  const byId = new Map<string, SceneNodeView>()
  const childrenByParent = new Map<string, SceneNodeView[]>()
  let inferredRootId: string | undefined
  for (const node of snapshot.nodes) {
    if (byId.has(node.id)) throw new Error(`Duplicate scene layout node: ${node.id}`)
    byId.set(node.id, node)
    if (node.parentNodeId === undefined) {
      inferredRootId ??= node.id
      continue
    }
    const children = childrenByParent.get(node.parentNodeId)
    if (children) children.push(node)
    else childrenByParent.set(node.parentNodeId, [node])
  }
  for (const children of childrenByParent.values()) {
    children.sort((left, right) => left.ordinal - right.ordinal)
  }

  assertAcyclic(snapshot.nodes, childrenByParent)

  const mountById = new Map<string, SessionMountView>()
  const mountByNode = new Map<string, SessionMountView>()
  const mountBySession = new Map<string, SessionMountView>()
  for (const mount of snapshot.mounts) {
    mountById.set(mount.id, mount)
    mountBySession.set(mount.sessionId, mount)
    if (mount.sceneNodeId) mountByNode.set(mount.sceneNodeId, mount)
  }
  const windowById = new Map(snapshot.windows.map((window) => [window.id, window]))
  const rootId = snapshot.scene.rootNodeId ?? inferredRootId
  const layout = rootId
    ? buildLayout(rootId, byId, childrenByParent, mountByNode)
    : undefined
  const orderedIds = layout ? readOrderedSessionIds(layout, mountById) : []
  const index: SceneLayoutIndex = {
    byId,
    childrenByParent,
    mountById,
    mountByNode,
    mountBySession,
    windowById,
    layout,
    orderedSessionIds: orderedIds
  }
  indexesBySnapshot.set(snapshot, index)
  return index
}

export function layoutFromSnapshot(snapshot: SceneSnapshotView): LayoutNode | undefined {
  return indexSceneLayout(snapshot).layout
}

export function orderedSessionIds(snapshot: SceneSnapshotView): readonly string[] {
  return indexSceneLayout(snapshot).orderedSessionIds
}

function assertAcyclic(
  nodes: readonly SceneNodeView[],
  childrenByParent: ReadonlyMap<string, readonly SceneNodeView[]>
): void {
  const state = new Map<string, 'visiting' | 'visited'>()
  for (const start of nodes) {
    if (state.has(start.id)) continue
    const stack: Array<{ node: SceneNodeView; nextChildIndex: number }> = [
      { node: start, nextChildIndex: 0 }
    ]
    state.set(start.id, 'visiting')
    while (stack.length > 0) {
      const frame = stack[stack.length - 1]!
      const children = childrenByParent.get(frame.node.id) ?? []
      const child = children[frame.nextChildIndex]
      if (!child) {
        state.set(frame.node.id, 'visited')
        stack.pop()
        continue
      }
      frame.nextChildIndex += 1
      const childState = state.get(child.id)
      if (childState === 'visited') continue
      if (childState === 'visiting') {
        const cycleStart = stack.findIndex(({ node }) => node.id === child.id)
        const cycle = stack.slice(cycleStart).map(({ node }) => node.id)
        cycle.push(child.id)
        throw new Error(`Scene layout cycle detected: ${cycle.join(' -> ')}`)
      }
      state.set(child.id, 'visiting')
      stack.push({ node: child, nextChildIndex: 0 })
    }
  }
}

function buildLayout(
  rootId: string,
  byId: ReadonlyMap<string, SceneNodeView>,
  childrenByParent: ReadonlyMap<string, readonly SceneNodeView[]>,
  mountByNode: ReadonlyMap<string, SessionMountView>
): LayoutNode | undefined {
  type BuildFrame = {
    nodeId: string
    nextChildIndex: number
    children: readonly SceneNodeView[]
    builtChildren: LayoutNode[]
  }
  const stack: BuildFrame[] = [frameFor(rootId, childrenByParent)]
  let result: LayoutNode | undefined
  while (stack.length > 0) {
    const frame = stack[stack.length - 1]!
    const mount = mountByNode.get(frame.nodeId)
    if (!mount && frame.nextChildIndex < frame.children.length) {
      const child = frame.children[frame.nextChildIndex]!
      frame.nextChildIndex += 1
      stack.push(frameFor(child.id, childrenByParent))
      continue
    }

    let built: LayoutNode | undefined
    if (mount) {
      built = { id: frame.nodeId, kind: 'mount', mountId: mount.id }
    } else if (frame.builtChildren.length === 1) {
      built = frame.builtChildren[0]
    } else if (frame.builtChildren.length > 1) {
      built = {
        id: frame.nodeId,
        kind: 'split',
        direction: byId.get(frame.nodeId)?.direction ?? 'horizontal',
        children: frame.builtChildren
      }
    }
    stack.pop()
    const parent = stack[stack.length - 1]
    if (parent) {
      if (built) parent.builtChildren.push(built)
    } else {
      result = built
    }
  }
  return result
}

function frameFor(
  nodeId: string,
  childrenByParent: ReadonlyMap<string, readonly SceneNodeView[]>
): {
  nodeId: string
  nextChildIndex: number
  children: readonly SceneNodeView[]
  builtChildren: LayoutNode[]
} {
  return {
    nodeId,
    nextChildIndex: 0,
    children: childrenByParent.get(nodeId) ?? [],
    builtChildren: []
  }
}

function readOrderedSessionIds(
  layout: LayoutNode,
  mountById: ReadonlyMap<string, SessionMountView>
): string[] {
  const ordered: string[] = []
  const stack: LayoutNode[] = [layout]
  while (stack.length > 0) {
    const current = stack.pop()!
    if (current.kind === 'mount') {
      const sessionId = mountById.get(current.mountId)?.sessionId
      if (sessionId) ordered.push(sessionId)
      continue
    }
    for (let index = current.children.length - 1; index >= 0; index -= 1) {
      stack.push(current.children[index]!)
    }
  }
  return ordered
}
