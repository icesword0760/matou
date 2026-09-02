import { describe, expect, it } from 'vitest'

import type { SessionGraphNodeView, SessionGraphView } from '../hierarchy/hierarchy-types'
import { layoutGraph } from './dag-layout'
import { buildDagRenderModel } from './dag-render-model'

describe('DAG render model', () => {
  it('turns a far branch into one truthful aggregate with exact live-state counts', () => {
    const nodes = [
      node('root', undefined, 'idle'),
      node('near', 'root', 'idle'),
      node('branch', 'near', 'running'),
      node('running-child', 'branch', 'starting'),
      node('input-child', 'branch', 'needs-input'),
      node('failed-child', 'branch', 'error'),
      node('idle-child', 'branch', 'idle')
    ]
    const layout = layoutGraph(graph(nodes))

    const model = buildDagRenderModel({
      layout,
      fullDepths: new Set([0, 1]),
      worldBounds: everything(),
      centerWorldY: 0,
      previewSessionId: 'near'
    })

    expect(new Set(model.realNodes.map(({ sessionId }) => sessionId))).toEqual(new Set(['root', 'near']))
    expect(model.aggregates).toHaveLength(1)
    expect(model.aggregates[0]).toMatchObject({
      key: 'aggregate:after:branch',
      direction: 'after',
      branchRootId: 'branch',
      targetSessionId: 'branch',
      sessionCount: 5,
      counts: { running: 2, needsInput: 1, error: 1 }
    })
    expect(new Set(model.aggregates[0]?.sessionIds)).toEqual(new Set([
      'branch', 'running-child', 'input-child', 'failed-child', 'idle-child'
    ]))
  })

  it('uses stable branch aggregates and merges overflow without losing counts', () => {
    const nodes = [node('root', undefined, 'idle')]
    for (let index = 0; index < 600; index += 1) {
      nodes.push(node(`branch-${index}`, 'root', index % 3 === 0 ? 'running' : 'idle'))
      nodes.push(node(`leaf-${index}`, `branch-${index}`, index % 11 === 0 ? 'error' : 'idle'))
    }
    const layout = layoutGraph(graph(nodes))
    const input = {
      layout,
      fullDepths: new Set([0]),
      worldBounds: everything(),
      centerWorldY: layout.height / 2,
      previewSessionId: 'root',
      maxItems: 400
    }

    const model = buildDagRenderModel(input)
    const clone = buildDagRenderModel({ ...input, layout: layoutGraph(structuredClone(graph(nodes))) })

    expect(model.realNodes.length + model.aggregates.length).toBeLessThanOrEqual(400)
    expect(model.aggregates.reduce((total, item) => total + item.sessionCount, 0)).toBe(1200)
    expect(model.aggregates.reduce((total, item) => total + item.counts.running, 0)).toBe(200)
    expect(model.aggregates.reduce((total, item) => total + item.counts.error, 0)).toBe(55)
    expect(model.aggregates.map(({ key }) => key)).toEqual(clone.aggregates.map(({ key }) => key))
    expect(model.aggregates.some(({ kind }) => kind === 'layer-overflow')).toBe(true)
  })

  it('replaces the aggregate with the real search or zoom target when its layer becomes near', () => {
    const nodes = Array.from({ length: 8 }, (_, index) => node(
      `depth-${index}`, index === 0 ? undefined : `depth-${index - 1}`,
      index === 6 ? 'needs-input' : 'idle'
    ))
    const layout = layoutGraph(graph(nodes))

    const far = buildDagRenderModel({
      layout,
      fullDepths: new Set([0, 1, 2]),
      worldBounds: everything(),
      centerWorldY: 0,
      previewSessionId: 'depth-1'
    })
    expect(far.aggregates[0]).toMatchObject({ sessionCount: 5, targetSessionId: 'depth-3' })

    const revealed = buildDagRenderModel({
      layout,
      fullDepths: new Set([5, 6, 7]),
      worldBounds: everything(),
      centerWorldY: 0,
      previewSessionId: 'depth-6'
    })
    expect(revealed.realNodes.map(({ sessionId }) => sessionId)).toContain('depth-6')
    expect(revealed.aggregates.flatMap(({ sessionIds }) => sessionIds)).not.toContain('depth-6')
  })

  it('keeps a 10000-session deep graph under the DOM item and edge ceilings', () => {
    const nodes = Array.from({ length: 10_000 }, (_, index) => node(
      `depth-${index}`, index === 0 ? undefined : `depth-${index - 1}`, 'idle'
    ))
    const layout = layoutGraph(graph(nodes))
    const model = buildDagRenderModel({
      layout,
      fullDepths: new Set([4_999, 5_000, 5_001]),
      worldBounds: everything(),
      centerWorldY: layout.height / 2,
      previewSessionId: 'depth-5000'
    })

    expect(model.realNodes.length + model.aggregates.length).toBeLessThanOrEqual(400)
    expect(model.edges.length).toBeLessThanOrEqual(800)
    expect(model.aggregates.reduce((total, item) => total + item.sessionCount, 0)).toBe(9_997)
  })
})

function graph(nodes: SessionGraphNodeView[]): SessionGraphView {
  return {
    sceneId: 'scene',
    nodes,
    edges: nodes.flatMap((item, index) => item.parentSessionId ? [{
      parentSessionId: item.parentSessionId,
      childSessionId: item.sessionId,
      relationKind: 'derived-from' as const,
      createdAt: index + 1
    }] : [])
  }
}

function node(
  sessionId: string,
  parentSessionId: string | undefined,
  workStatus: SessionGraphNodeView['workStatus']
): SessionGraphNodeView {
  return {
    sessionId,
    sceneId: 'scene',
    ...(parentSessionId ? { parentSessionId } : {}),
    currentMode: 'shell',
    workStatus,
    providerRestoreState: 'none',
    canFork: false,
    title: sessionId,
    cwd: `/tmp/${sessionId}`,
    activeChildCount: 0,
    stoppedChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 },
    latestLines: [],
    lastUserInteractionSeq: 0
  }
}

function everything() {
  return { left: -Infinity, right: Infinity, top: -Infinity, bottom: Infinity }
}
