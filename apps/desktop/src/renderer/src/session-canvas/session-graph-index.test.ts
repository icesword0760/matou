import { describe, expect, it } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { indexSessionGraph } from './session-graph-index'

// Shared CI runners are slower and noisier than developer machines; keep the gate but loosen it there.
const CI_BUDGET_FACTOR = process.env.CI ? 2 : 1

describe('session graph index', () => {
  it('preserves the existing reverse sibling depth-first descendant order', () => {
    const nodes = [
      node('root'),
      node('a', 'root'),
      { ...node('b', 'root'), workStatus: 'running' as const },
      { ...node('a-1', 'a'), workStatus: 'needs-input' as const },
      { ...node('a-2', 'a'), workStatus: 'starting' as const },
      node('b-1', 'b')
    ]

    const index = indexSessionGraph(nodes)

    expect(index.byId.get('a-2')).toBe(nodes[4])
    expect(index.childrenByParent.get('root')?.map(({ sessionId }) => sessionId))
      .toEqual(['a', 'b'])
    expect(index.descendantsOf('root').map(({ sessionId }) => sessionId))
      .toEqual(['b', 'b-1', 'a', 'a-2', 'a-1'])
    expect(index.descendantSummaryOf('root')).toEqual({ count: 5, running: 2, needsInput: 1 })
    expect(indexSessionGraph(nodes)).toBe(index)
  })

  it('indexes and traverses a 5000-node chain without overflowing the stack', () => {
    const nodes = Array.from({ length: 5_000 }, (_, index) =>
      node(`session-${index}`, index === 0 ? undefined : `session-${index - 1}`))

    const startedAt = performance.now()
    const index = indexSessionGraph(nodes)
    const indexElapsed = performance.now() - startedAt
    const descendantsStartedAt = performance.now()
    const descendants = index.descendantsOf('session-0')
    const descendantsElapsed = performance.now() - descendantsStartedAt

    expect(index.byId.size).toBe(5_000)
    expect(descendants).toHaveLength(4_999)
    expect(descendants[0]?.sessionId).toBe('session-1')
    expect(descendants.at(-1)?.sessionId).toBe('session-4999')
    expect(indexElapsed).toBeLessThanOrEqual(50 * CI_BUDGET_FACTOR)
    expect(descendantsElapsed).toBeLessThanOrEqual(50 * CI_BUDGET_FACTOR)
  })

  it('indexes and traverses a 10000-node wide tree within the scale budget', () => {
    const nodes = [node('root')]
    for (let index = 1; index < 10_000; index += 1) {
      nodes.push(node(`session-${index}`, 'root'))
    }

    const samples = Array.from({ length: 20 }, () => {
      const startedAt = performance.now()
      const index = indexSessionGraph(nodes.slice())
      const descendants = index.descendantsOf('root')
      return { elapsed: performance.now() - startedAt, descendants }
    })
    const p95 = percentile(samples.map(({ elapsed }) => elapsed), 0.95)
    const descendants = samples.at(-1)!.descendants

    expect(descendants).toHaveLength(9_999)
    expect(descendants[0]?.sessionId).toBe('session-9999')
    expect(descendants.at(-1)?.sessionId).toBe('session-1')
    expect(p95).toBeLessThanOrEqual(50 * CI_BUDGET_FACTOR)
  })

  it('reports every node in a corrupt cycle instead of looping', () => {
    const nodes = [node('a', 'c'), node('b', 'a'), node('c', 'b')]

    expect(() => indexSessionGraph(nodes))
      .toThrow(/session graph cycle.*a.*b.*c/i)
  })
})

function node(sessionId: string, parentSessionId?: string): SessionGraphNodeView {
  return {
    sessionId,
    sceneId: 'scene-1',
    ...(parentSessionId ? { parentSessionId } : {}),
    currentMode: 'shell',
    workStatus: 'idle',
    providerRestoreState: 'none',
    canFork: false,
    title: sessionId,
    cwd: '/tmp',
    activeChildCount: 0,
    stoppedChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 },
    latestLines: [],
    lastUserInteractionSeq: 0
  }
}

function percentile(values: number[], quantile: number): number {
  const ordered = [...values].sort((left, right) => left - right)
  return ordered[Math.ceil(ordered.length * quantile) - 1] ?? 0
}
