import { performance } from 'node:perf_hooks'

import type { DomainEventWireEnvelope } from '@matou/contracts'
import { describe, expect, it } from 'vitest'

import {
  RuntimeProjectionStore,
  type RuntimeProjectionSnapshot,
  type SessionGraphProjection
} from './RuntimeProjectionStore'

const SESSION_COUNT = 10_000
// Shared CI runners are slower and noisier than developer machines; keep the gate but loosen it there.
const CI_BUDGET_FACTOR = process.env.CI ? 2 : 1
const FRAME_BUDGET_MS = 16.7 * CI_BUDGET_FACTOR

describe('RuntimeProjectionStore 10,000-Session event budget', () => {
  it('applies consecutive cwd batches and publishes the production view within one frame at p95', () => {
    const snapshot = projectionSnapshot()
    const store = new RuntimeProjectionStore()
    const cloneProbe = probeStructuredClone()
    const replaceStartedAt = performance.now()
    store.replace(snapshot)
    const snapshotMs = performance.now() - replaceStartedAt
    const snapshotCloneMs = cloneProbe.readAndReset()
    const samples: ProjectionSample[] = []

    try {
      for (let index = 0; index < 30; index += 1) {
        const sequence = index + 2
        const applyStartedAt = performance.now()
        store.applyBatch('generation-1', [cwdEvent(sequence, index)])
        const applyMs = performance.now() - applyStartedAt
        const viewStartedAt = performance.now()
        const view = store.view()
        const viewMs = performance.now() - viewStartedAt
        const cloneMs = cloneProbe.readAndReset()
        samples.push({
          totalMs: applyMs + viewMs,
          applyMs,
          viewMs,
          cloneMs,
          materializeMs: Math.max(0, viewMs - cloneMs)
        })
        expect(view.sessions[index]?.cwd).toBe(`/workspace/${index}`)
        expect(view.sessionGraphs['scene-1']?.nodes[index]?.cwd).toBe(`/workspace/${index}`)
      }
    } finally {
      cloneProbe.restore()
    }

    const measured = samples.slice(5)
    const metrics = {
      sessions: SESSION_COUNT,
      snapshotMs: round(snapshotMs),
      snapshotCloneMs: round(snapshotCloneMs),
      totalP95Ms: percentile(measured.map(({ totalMs }) => totalMs), .95),
      applyP95Ms: percentile(measured.map(({ applyMs }) => applyMs), .95),
      viewP95Ms: percentile(measured.map(({ viewMs }) => viewMs), .95),
      cloneP95Ms: percentile(measured.map(({ cloneMs }) => cloneMs), .95),
      materializeP95Ms: percentile(measured.map(({ materializeMs }) => materializeMs), .95)
    }
    console.log(`[projection-10000-cwd] ${JSON.stringify(metrics)}`)
    expect(metrics.totalP95Ms).toBeLessThanOrEqual(FRAME_BUDGET_MS)
  })

  it('coalesces a sustained graph-bearing batch before publishing its 10,000-Session graph', () => {
    const store = new RuntimeProjectionStore()
    store.replace(projectionSnapshot())
    const samples: number[] = []
    for (let sample = 0; sample < 12; sample += 1) {
      const firstSequence = store.eventSequence + 1
      const events = Array.from({ length: 12 }, (_, eventIndex) => graphEvent(
        firstSequence + eventIndex,
        sample * 12 + eventIndex
      ))
      const startedAt = performance.now()
      store.applyBatch('generation-1', events)
      const view = store.view()
      samples.push(performance.now() - startedAt)
      expect(view.sessionGraphs['scene-1']?.nodes[9_999]?.workStatus)
        .toBe(events.at(-1)?.payload && graphFromPayload(events.at(-1)!.payload).nodes[9_999]?.workStatus)
    }
    const p95 = percentile(samples.slice(2), .95)
    console.log(`[projection-10000-graph-batch] ${JSON.stringify({
      sessions: SESSION_COUNT, eventsPerBatch: 12, p95Ms: p95
    })}`)
    expect(p95).toBeLessThanOrEqual(FRAME_BUDGET_MS)
  })
})

interface ProjectionSample {
  totalMs: number
  applyMs: number
  viewMs: number
  cloneMs: number
  materializeMs: number
}

function projectionSnapshot(): RuntimeProjectionSnapshot {
  const graph = graphProjection(0)
  const sessions = graph.nodes.map((node, index) => ({
    id: node.sessionId,
    taskId: 'task-1',
    status: 'created',
    cwd: node.cwd,
    updatedAt: index
  }))
  return {
    runtimeGeneration: 'generation-1',
    eventSequence: 1,
    workspaces: [{ id: 'workspace-1', name: 'Workspace' }],
    tasks: [{ id: 'task-1', workspaceId: 'workspace-1', status: 'active' }],
    sessions,
    relations: graph.edges.map((edge, index) => ({ id: `relation-${index}`, ...edge })),
    scenes: [{ id: 'scene-1', taskId: 'task-1', name: 'Scene' }],
    sessionGraphs: { 'scene-1': graph },
    hierarchy: {
      windowId: 'window-1',
      workspaces: [], tasks: [], sessions: [], scenes: [],
      navigation: {
        windowId: 'window-1', activeWorkspaceId: 'workspace-1',
        taskByWorkspace: { 'workspace-1': 'task-1' },
        sceneByTask: { 'task-1': 'scene-1' },
        sessionByScene: { 'scene-1': 'session-1' }
      }
    }
  }
}

function graphProjection(revision: number): SessionGraphProjection {
  const nodes = Array.from({ length: SESSION_COUNT }, (_, index) => ({
    sessionId: `session-${index + 1}`,
    sceneId: 'scene-1',
    ...(index === 0 ? {} : { parentSessionId: `session-${Math.floor((index - 1) / 4) + 1}` }),
    currentMode: 'shell',
    workStatus: index === 9_999 && revision % 2 === 1 ? 'running' : 'idle',
    providerRestoreState: 'none',
    canFork: false,
    title: `Session ${index + 1}`,
    cwd: `/workspace/original/${index}`,
    activeChildCount: 0,
    stoppedChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 },
    latestLines: [],
    lastUserInteractionSeq: revision
  }))
  return {
    sceneId: 'scene-1',
    focusedSessionId: 'session-1',
    nodes,
    edges: nodes.slice(1).map((node) => ({
      parentSessionId: node.parentSessionId,
      childSessionId: node.sessionId,
      relationKind: 'derived-from',
      createdAt: revision
    }))
  }
}

function cwdEvent(sequence: number, index: number): DomainEventWireEnvelope {
  return {
    sequence,
    eventId: `cwd-${sequence}`,
    eventType: 'session.cwd-updated',
    aggregateType: 'session',
    aggregateId: `session-${index + 1}`,
    sessionId: `session-${index + 1}`,
    payload: { cwd: `/workspace/${index}` },
    schemaVersion: 1,
    commandId: `cwd-command-${sequence}`,
    occurredAt: sequence
  }
}

function graphEvent(sequence: number, revision: number): DomainEventWireEnvelope {
  return {
    sequence,
    eventId: `graph-${sequence}`,
    eventType: 'session.graph-summary-changed',
    aggregateType: 'scene',
    aggregateId: 'scene-1',
    payload: { graph: graphProjection(revision) },
    schemaVersion: 1,
    commandId: `graph-command-${sequence}`,
    occurredAt: sequence
  }
}

function graphFromPayload(payload: unknown): SessionGraphProjection {
  return (payload as { graph: SessionGraphProjection }).graph
}

function probeStructuredClone(): {
  readAndReset(): number
  restore(): void
} {
  const original = globalThis.structuredClone
  let elapsed = 0
  globalThis.structuredClone = ((value: unknown, options?: StructuredSerializeOptions) => {
    const startedAt = performance.now()
    try {
      return original(value, options)
    } finally {
      elapsed += performance.now() - startedAt
    }
  }) as typeof structuredClone
  return {
    readAndReset() {
      const current = elapsed
      elapsed = 0
      return current
    },
    restore() {
      globalThis.structuredClone = original
    }
  }
}

function percentile(values: number[], quantile: number): number {
  const ordered = [...values].sort((left, right) => left - right)
  return round(ordered[Math.min(ordered.length - 1, Math.ceil(ordered.length * quantile) - 1)] ?? 0)
}

function round(value: number): number {
  return Math.round(value * 100) / 100
}
