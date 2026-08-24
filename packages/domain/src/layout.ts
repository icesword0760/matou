export type LayoutNode = LayoutMountNode | LayoutSplitNode

export interface LayoutMountNode {
  id: string
  kind: 'mount'
  mountId: string
}

export interface LayoutSplitNode {
  id: string
  kind: 'split'
  direction: 'horizontal' | 'vertical'
  children: LayoutNode[]
}

export function normalizeLayout(root: LayoutNode): LayoutNode {
  const nodeIds = new Set<string>()
  const mountIds = new Set<string>()

  const normalizeNode = (node: LayoutNode): LayoutNode => {
    if (!node.id.trim()) throw new Error('layout node id must not be empty')
    if (node.kind === 'mount') {
      if (!node.mountId.trim()) throw new Error('layout mount id must not be empty')
      if (mountIds.has(node.mountId)) throw new Error(`duplicate mount ${node.mountId}`)
      mountIds.add(node.mountId)
    }
    if (nodeIds.has(node.id)) throw new Error(`duplicate layout node ${node.id}`)
    nodeIds.add(node.id)
    if (node.kind === 'mount') return { ...node }
    if (node.children.length === 0) throw new Error('split must contain at least one child')
    const children = node.children
      .map(normalizeNode)
      .flatMap((child) =>
        child.kind === 'split' && child.direction === node.direction
          ? child.children
          : [child]
      )
    if (children.length === 1) return children[0]!
    return { ...node, children }
  }

  return normalizeNode(root)
}

export function splitMount(
  root: LayoutNode,
  targetMountId: string,
  newMount: LayoutMountNode,
  direction: LayoutSplitNode['direction']
): LayoutNode {
  let replaced = false
  const replace = (node: LayoutNode): LayoutNode => {
    if (node.kind === 'mount') {
      if (node.mountId !== targetMountId) return { ...node }
      replaced = true
      return {
        id: `split-${targetMountId}-${newMount.mountId}`,
        kind: 'split',
        direction,
        children: [{ ...node }, { ...newMount }]
      }
    }
    return { ...node, children: node.children.map(replace) }
  }
  const result = replace(root)
  if (!replaced) throw new Error(`mount ${targetMountId} does not exist in the layout`)
  return normalizeLayout(result)
}

export function removeMount(root: LayoutNode, mountId: string): LayoutNode | null {
  let removed = false
  const remove = (node: LayoutNode): LayoutNode | null => {
    if (node.kind === 'mount') {
      if (node.mountId !== mountId) return { ...node }
      removed = true
      return null
    }
    const children = node.children
      .map(remove)
      .filter((child): child is LayoutNode => child !== null)
    if (children.length === 0) return null
    if (children.length === 1) return children[0]!
    return { ...node, children }
  }
  const result = remove(root)
  if (!removed) throw new Error(`mount ${mountId} does not exist in the layout`)
  return result === null ? null : normalizeLayout(result)
}
