import type { LayoutNode } from '@matou/domain'
import { describe, expect, it } from 'vitest'

import type { SceneSnapshotView } from './hierarchy-types'
import { layoutFromSnapshot, orderedSessionIds } from './scene-layout-index'

describe('scene layout index', () => {
  it('preserves ordinal ordering, empty-node collapse, directions, and session order', () => {
    const snapshot = sceneSnapshot({
      rootNodeId: 'root',
      nodes: [
        sceneNode('root', undefined, 0, 'root', 'vertical'),
        sceneNode('mount-c', 'root', 2, 'mount'),
        sceneNode('branch', 'root', 1, 'split', 'horizontal'),
        sceneNode('mount-b', 'branch', 2, 'mount'),
        sceneNode('empty', 'branch', 1, 'group'),
        sceneNode('mount-a', 'branch', 0, 'mount')
      ],
      mounts: [
        mount('mount-b', 'session-b'),
        mount('mount-c', 'session-c'),
        mount('mount-a', 'session-a')
      ]
    })

    expect(layoutFromSnapshot(snapshot)).toEqual({
      id: 'root',
      kind: 'split',
      direction: 'vertical',
      children: [
        {
          id: 'branch',
          kind: 'split',
          direction: 'horizontal',
          children: [
            { id: 'mount-a', kind: 'mount', mountId: 'mount-mount-a' },
            { id: 'mount-b', kind: 'mount', mountId: 'mount-mount-b' }
          ]
        },
        { id: 'mount-c', kind: 'mount', mountId: 'mount-mount-c' }
      ]
    })
    expect(orderedSessionIds(snapshot)).toEqual(['session-a', 'session-b', 'session-c'])
  })

  it('restores a 5000-level split and orders its sessions without recursion', () => {
    const snapshot = deepSplitSnapshot(5_000)
    const layoutSamples: number[] = []
    const orderSamples: number[] = []
    let layout: LayoutNode | undefined
    let ordered: readonly string[] = []

    for (let sample = 0; sample < 20; sample += 1) {
      let startedAt = performance.now()
      layout = layoutFromSnapshot({ ...snapshot })
      layoutSamples.push(performance.now() - startedAt)
      startedAt = performance.now()
      ordered = orderedSessionIds({ ...snapshot })
      orderSamples.push(performance.now() - startedAt)
    }

    expect(splitDepth(layout)).toBe(5_000)
    expect(ordered).toHaveLength(5_001)
    expect(ordered.slice(0, 3)).toEqual(['session-final-a', 'session-final-b', 'session-side-4998'])
    expect(ordered.at(-1)).toBe('session-side-0')
    expect(percentile(layoutSamples, 0.95)).toBeLessThanOrEqual(50)
    expect(percentile(orderSamples, 0.95)).toBeLessThanOrEqual(50)
  })

  it('reports every node in a corrupt split cycle instead of looping', () => {
    const snapshot = sceneSnapshot({
      rootNodeId: 'a',
      nodes: [
        sceneNode('a', 'c', 0, 'split'),
        sceneNode('b', 'a', 0, 'split'),
        sceneNode('c', 'b', 0, 'split')
      ],
      mounts: []
    })

    expect(() => layoutFromSnapshot(snapshot))
      .toThrow(/scene layout cycle.*a.*b.*c/i)
  })
})

function deepSplitSnapshot(depth: number): SceneSnapshotView {
  const nodes: SceneSnapshotView['nodes'] = []
  const mounts: SceneSnapshotView['mounts'] = []
  for (let index = 0; index < depth; index += 1) {
    const splitId = `split-${index}`
    nodes.push(sceneNode(
      splitId,
      index === 0 ? undefined : `split-${index - 1}`,
      0,
      'split',
      index % 2 === 0 ? 'horizontal' : 'vertical'
    ))
    if (index < depth - 1) {
      const sideId = `side-${index}`
      nodes.push(sceneNode(sideId, splitId, 1, 'mount'))
      mounts.push(mount(sideId, `session-side-${index}`))
    }
  }
  const lastSplitId = `split-${depth - 1}`
  nodes.push(
    sceneNode('final-a', lastSplitId, 0, 'mount'),
    sceneNode('final-b', lastSplitId, 1, 'mount')
  )
  mounts.push(
    mount('final-a', 'session-final-a'),
    mount('final-b', 'session-final-b')
  )
  return sceneSnapshot({ rootNodeId: 'split-0', nodes, mounts })
}

function splitDepth(root: LayoutNode | undefined): number {
  let depth = 0
  let current = root
  while (current?.kind === 'split') {
    depth += 1
    current = current.children[0]
  }
  return depth
}

function sceneSnapshot(input: {
  rootNodeId?: string
  nodes: SceneSnapshotView['nodes']
  mounts: SceneSnapshotView['mounts']
}): SceneSnapshotView {
  return {
    scene: {
      id: 'scene-1',
      taskId: 'task-1',
      name: 'scene',
      ...(input.rootNodeId ? { rootNodeId: input.rootNodeId } : {})
    },
    nodes: input.nodes,
    mounts: input.mounts,
    windows: []
  }
}

function sceneNode(
  id: string,
  parentNodeId: string | undefined,
  ordinal: number,
  kind: SceneSnapshotView['nodes'][number]['kind'],
  direction?: 'horizontal' | 'vertical'
): SceneSnapshotView['nodes'][number] {
  return {
    id,
    sceneId: 'scene-1',
    ...(parentNodeId ? { parentNodeId } : {}),
    kind,
    ordinal,
    ...(direction ? { direction } : {})
  }
}

function mount(sceneNodeId: string, sessionId: string): SceneSnapshotView['mounts'][number] {
  return {
    id: `mount-${sceneNodeId}`,
    sceneId: 'scene-1',
    sceneNodeId,
    sessionId
  }
}

function percentile(values: number[], quantile: number): number {
  const ordered = [...values].sort((left, right) => left - right)
  return ordered[Math.ceil(ordered.length * quantile) - 1] ?? 0
}
