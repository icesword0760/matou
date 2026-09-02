import { describe, expect, it } from 'vitest'

import type { SessionGraphView } from '../hierarchy/hierarchy-types'
import { layoutGraph, searchGraph, visibleLayers } from './dag-layout'

describe('DAG layout', () => {
  it('uses stable depth columns, creation order and directed edge endpoints', () => {
    const graph = fixture()
    const layout = layoutGraph(graph)

    expect(layout.nodes.map(({ sessionId, depth }) => [sessionId, depth])).toEqual([
      ['root', 0], ['sibling-root', 0], ['child-a', 1], ['child-b', 1], ['grandchild', 2]
    ])
    expect(layout.nodes.find(({ sessionId }) => sessionId === 'child-a')!.y)
      .toBeLessThan(layout.nodes.find(({ sessionId }) => sessionId === 'child-b')!.y)
    expect(layout.edges[0]).toMatchObject({ fromSessionId: 'root', toSessionId: 'child-a' })
    expect(layout.edges[0]!.from.x).toBeLessThan(layout.edges[0]!.to.x)
  })

  it('keeps three full layers around focus and groups farther depths as ghosts', () => {
    const graph = fixture()
    const layout = layoutGraph(graph)
    expect(visibleLayers(layout, 'child-a')).toEqual({ fullDepths: [0, 1, 2], ghostDepths: [] })
    expect(visibleLayers(layout, 'root', 0)).toEqual({ fullDepths: [0], ghostDepths: [1, 2] })
  })

  it('lays out 100 nodes deterministically and ranks title before paths and summaries', () => {
    const graph = fixture(100)
    expect(layoutGraph(graph)).toEqual(layoutGraph(structuredClone(graph)))
    const results = searchGraph(graph, 'deploy')
    expect(results[0]?.title).toBe('Deploy Agent')
    expect(results.map(({ sessionId }) => sessionId)).toContain('summary-hit')
  })

  it('lays out a 5000-deep chain iteratively without overflowing the stack', () => {
    const nodes = Array.from({ length: 5000 }, (_, index) => node(
      `deep-${index}`, `Deep ${index}`, index === 0 ? undefined : `deep-${index - 1}`, index + 1
    ))
    const graph: SessionGraphView = {
      sceneId: 'scene', nodes,
      edges: nodes.slice(1).map((item, index) => ({
        parentSessionId: `deep-${index}`, childSessionId: item.sessionId,
        relationKind: 'derived-from' as const, createdAt: index + 1
      }))
    }
    const layout = layoutGraph(graph)

    expect(layout.depthCount).toBe(5000)
    expect(layout.nodeById.get('deep-4999')?.depth).toBe(4999)
    expect(layout.nodes).toHaveLength(5000)
  })

  it('keeps a repeatable 10000-node wide DAG layout inside a stable budget', () => {
    const graph = fixture(10000)
    const durations = Array.from({ length: 5 }, () => {
      const startedAt = performance.now()
      const layout = layoutGraph(graph)
      expect(layout.nodes).toHaveLength(10001)
      return performance.now() - startedAt
    }).sort((left, right) => left - right)
    const p95 = durations[Math.ceil(durations.length * .95) - 1]!

    console.log(`[scale-dag-layout] ${JSON.stringify({ durations, p95 })}`)

    expect(p95).toBeLessThan(1500)
  })
})

function fixture(count?: number): SessionGraphView {
  if (count) {
    const nodes = Array.from({ length: count }, (_, index) => node(
      `node-${index}`, index === 0 ? 'Deploy Agent' : `Node ${index}`,
      index === 0 ? undefined : `node-${Math.floor((index - 1) / 3)}`, index + 1
    ))
    nodes.push({ ...node('summary-hit', 'Other', undefined, count + 1), latestLines: ['ready to deploy'] })
    return { sceneId: 'scene', nodes, edges: nodes.flatMap((item) => item.parentSessionId ? [{
      parentSessionId: item.parentSessionId, childSessionId: item.sessionId,
      relationKind: 'derived-from' as const, createdAt: item.siblingCreatedSeq ?? 0
    }] : []) }
  }
  const nodes = [
    node('root', 'Root', undefined, 1),
    node('sibling-root', 'Root 2', undefined, 2),
    node('child-b', 'Child B', 'root', 4),
    node('child-a', 'Child A', 'root', 3),
    node('grandchild', 'Grandchild', 'child-a', 5)
  ]
  return { sceneId: 'scene', focusedSessionId: 'child-a', nodes, edges: [
    { parentSessionId: 'root', childSessionId: 'child-a', relationKind: 'forked-from', createdAt: 3 },
    { parentSessionId: 'root', childSessionId: 'child-b', relationKind: 'derived-from', createdAt: 4 },
    { parentSessionId: 'child-a', childSessionId: 'grandchild', relationKind: 'forked-from', createdAt: 5 }
  ] }
}

function node(sessionId: string, title: string, parentSessionId?: string, siblingCreatedSeq = 0) {
  return {
    sessionId, sceneId: 'scene', ...(parentSessionId ? { parentSessionId } : {}),
    currentMode: 'shell' as const, workStatus: 'idle' as const,
    providerRestoreState: 'none' as const, canFork: false, title, cwd: `/tmp/${sessionId}`,
    activeChildCount: 0, stoppedChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
    latestLines: [] as string[], siblingCreatedSeq, lastUserInteractionSeq: 0
  }
}
