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
