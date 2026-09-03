import type { LayoutNode } from '@matou/domain'
import { describe, expect, it } from 'vitest'

import type { SceneSnapshotView } from './hierarchy-types'
import { layoutFromSnapshot, orderedSessionIds } from './scene-layout-index'

// Shared CI runners are slower and noisier than developer machines; keep the gate but loosen it there.
const CI_BUDGET_FACTOR = process.env.CI ? 2 : 1

describe('scene layout index performance', () => {
  it('restores a 5000-level split and orders its sessions within the scale budget', () => {
    const snapshot = deepSplitSnapshot(5_000)
    const coldLayoutCpuSamples: number[] = []
    const cachedOrderCpuSamples: number[] = []
    let layout: LayoutNode | undefined
    let ordered: readonly string[] = []

    for (let sample = 0; sample < 20; sample += 1) {
      // A projection update creates one new snapshot identity. All consumers of
      // that update then share its WeakMap-backed index. The dedicated single-
      // worker performance suite keeps unrelated tests out of the measurement.
      const sampleSnapshot = { ...snapshot }
      let startedAt = process.cpuUsage()
      layout = layoutFromSnapshot(sampleSnapshot)
      coldLayoutCpuSamples.push(elapsedCpuMs(startedAt))
      startedAt = process.cpuUsage()
      ordered = orderedSessionIds(sampleSnapshot)
      cachedOrderCpuSamples.push(elapsedCpuMs(startedAt))
    }

    expect(splitDepth(layout)).toBe(5_000)
    expect(ordered).toHaveLength(5_001)
    expect(ordered.slice(0, 3)).toEqual(['session-final-a', 'session-final-b', 'session-side-4998'])
    expect(ordered.at(-1)).toBe('session-side-0')
    const coldLayoutCpuP95 = percentile(coldLayoutCpuSamples, 0.95)
    const cachedOrderCpuP95 = percentile(cachedOrderCpuSamples, 0.95)
    console.info(`[scene-layout-5000] ${JSON.stringify({
      coldLayoutCpuP95: Number(coldLayoutCpuP95.toFixed(2)),
      cachedOrderCpuP95: Number(cachedOrderCpuP95.toFixed(2))
    })}`)
    expect(coldLayoutCpuP95).toBeLessThanOrEqual(50 * CI_BUDGET_FACTOR)
    expect(cachedOrderCpuP95).toBeLessThanOrEqual(1 * CI_BUDGET_FACTOR)
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

function elapsedCpuMs(startedAt: NodeJS.CpuUsage): number {
  const elapsed = process.cpuUsage(startedAt)
  return (elapsed.user + elapsed.system) / 1_000
}
