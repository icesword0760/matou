import { useCallback, useEffect, useRef, useState } from 'react'

import type { DagWindowContext, RuntimeConnectionState } from '../../../shared/desktop-api'
import type { DomainEventWireEnvelope, RuntimeMessage, RuntimeMode } from '@matou/contracts'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import type { SessionGraphView } from '../hierarchy/hierarchy-types'
import { DagCanvas, type DagTransform } from './DagCanvas'
import { DagGraphFrameQueue } from './DagGraphFrameQueue'
import './dag.css'
import { READ_ONLY_REASON } from '../recovery/ReadOnlyRecoveryBanner'

export function DagWindowApp({ fixtureGraph, runtimeMode = 'normal' }: {
  fixtureGraph?: SessionGraphView
  runtimeMode?: RuntimeMode
}) {
  const client = useRuntimeClient()
  const readOnly = runtimeMode === 'read-only'
  const [context, setContext] = useState(readContext)
  const [graph, setGraph] = useState<SessionGraphView | null>(fixtureGraph ?? null)
  const [initialTransform, setInitialTransform] = useState<DagTransform | undefined>(undefined)
  const [geometryReady, setGeometryReady] = useState(Boolean(fixtureGraph))
  const latestTransform = useRef<DagTransform | undefined>(undefined)
  const readOnlyRef = useRef(readOnly)
  readOnlyRef.current = readOnly
  const layoutRevision = useRef(0)
  const graphRef = useRef<SessionGraphView | null>(fixtureGraph ?? null)
  const graphEventSequence = useRef(fixtureGraph?.eventSequence ?? -1)
  const runtimeGeneration = useRef(fixtureGraph?.runtimeGeneration ?? '')
  const projectionStarted = useRef(false)
  const refreshInFlight = useRef(false)
  const firstOperableMs = useRef<number | undefined>(undefined)
  const [error, setError] = useState('')
  const [runtimeConnection, setRuntimeConnection] = useState<RuntimeConnectionState>('ready')
  const [notifiedSessionIds, setNotifiedSessionIds] = useState<string[]>(
    context.notificationSessionIds ?? []
  )
  const applyGraph = useCallback((next: SessionGraphView, eventSequence?: number, generation?: string) => {
    const nextGeneration = generation ?? next.runtimeGeneration
    if (nextGeneration && runtimeGeneration.current && nextGeneration !== runtimeGeneration.current) {
      graphEventSequence.current = -1
    }
    if (nextGeneration) runtimeGeneration.current = nextGeneration
    const nextSequence = eventSequence ?? next.eventSequence
    if (nextSequence !== undefined && nextSequence < graphEventSequence.current) return
    if (nextSequence !== undefined) graphEventSequence.current = nextSequence
    graphRef.current = next
    setGraph(next)
    layoutRevision.current = next.layoutRevision ?? layoutRevision.current
  }, [])
  const applyGraphRef = useRef(applyGraph)
  applyGraphRef.current = applyGraph
  const graphFrameQueue = useRef<DagGraphFrameQueue | null>(null)
  if (!graphFrameQueue.current) {
    graphFrameQueue.current = new DagGraphFrameQueue(({ graph: next, sequence, runtimeGeneration: generation }) => {
      applyGraphRef.current(next, sequence, generation)
    })
  }
  const refresh = useCallback(async () => {
    if (fixtureGraph || !client || refreshInFlight.current) return
    refreshInFlight.current = true
    try {
      const next = await client.request<SessionGraphView>('hierarchy.get-scene-session-graph', {
        sceneId: context.sceneId,
        windowId: context.mainWindowId
      })
      applyGraph(next)
      if (next.eventSequence !== undefined) {
        const afterSequence = Math.max(next.eventSequence, graphEventSequence.current)
        if (!projectionStarted.current || next.runtimeGeneration !== runtimeGeneration.current) {
          client.startProjection(afterSequence)
          projectionStarted.current = true
        }
      }
      setError('')
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      refreshInFlight.current = false
    }
  }, [applyGraph, client, context.mainWindowId, context.sceneId, fixtureGraph])

  useEffect(() => window.matouDesktop?.onDagContext?.((next) => {
    graphFrameQueue.current?.cancel()
    const initialGraph = graphFromContext(next.initialGraph, next.sceneId)
    setContext({
      mainWindowId: next.mainWindowId,
      sceneId: next.sceneId,
      sessionId: next.sessionId,
      theme: next.theme,
      ...(next.notificationSessionIds ? { notificationSessionIds: next.notificationSessionIds } : {}),
      ...(next.requestedAt === undefined ? {} : { requestedAt: next.requestedAt })
    })
    setGraph(initialGraph ?? null)
    graphRef.current = initialGraph ?? null
    graphEventSequence.current = initialGraph?.eventSequence ?? -1
    runtimeGeneration.current = initialGraph?.runtimeGeneration ?? ''
    projectionStarted.current = false
    firstOperableMs.current = undefined
    setInitialTransform(undefined)
    setGeometryReady(false)
    setNotifiedSessionIds(next.notificationSessionIds ?? [])
  }), [])
  useEffect(() => window.matouDesktop?.onDagNotifications?.(setNotifiedSessionIds), [])
  useEffect(() => window.matouDesktop?.onRuntimeConnectionState?.(setRuntimeConnection), [])
  useEffect(() => {
    if (fixtureGraph || !client || typeof client.subscribeProjection !== 'function') return
    return client.subscribeProjection((message: RuntimeMessage) => {
      if (message.type !== 'events.batch') return
      if (runtimeGeneration.current && message.runtimeGeneration !== runtimeGeneration.current) {
        graphFrameQueue.current?.cancel()
        runtimeGeneration.current = message.runtimeGeneration
        graphEventSequence.current = -1
        projectionStarted.current = false
        void refresh()
        return
      }
      let latest: { graph: SessionGraphView; sequence: number } | undefined
      let requiresScopedRefresh = false
      for (const event of message.events) {
        if (event.sequence <= graphEventSequence.current) continue
        const eventGraph = graphFromEvent(event)
        if (eventGraph?.sceneId === context.sceneId) {
          latest = { graph: eventGraph, sequence: event.sequence }
        } else if (eventTouchesGraph(event, context.sceneId, graphRef.current)) {
          requiresScopedRefresh = true
        }
      }
      if (latest) {
        graphFrameQueue.current?.enqueue({
          graph: latest.graph,
          sequence: latest.sequence,
          runtimeGeneration: message.runtimeGeneration
        })
      } else if (requiresScopedRefresh) {
        void refresh()
      }
    })
  }, [applyGraph, client, context.sceneId, fixtureGraph, refresh])
  useEffect(() => () => graphFrameQueue.current?.cancel(), [])
  useEffect(() => {
    if (fixtureGraph || !client) return
    setGeometryReady(false)
    void client.request<Array<{
      ownerKey: string
      layoutRevision: number
      geometry: Record<string, unknown>
    }>>('geometry.list', {
      sceneId: context.sceneId
    }).then((items) => {
      const stored = items.find(({ ownerKey }) => ownerKey === `dag-viewport:${context.sceneId}`)
      const value = stored?.geometry
      if (stored) layoutRevision.current = Math.max(layoutRevision.current, stored.layoutRevision)
      if (typeof value?.panX === 'number' && typeof value.panY === 'number' && typeof value.zoom === 'number') {
        setInitialTransform({ x: value.panX, y: value.panY, scale: value.zoom })
      }
    }).catch(() => {}).finally(() => setGeometryReady(true))
  }, [client, context.sceneId, fixtureGraph])
  useEffect(() => {
    if (!graphRef.current) void refresh()
  }, [fixtureGraph, refresh])
  useEffect(() => {
    if (fixtureGraph || !client || projectionStarted.current || graphEventSequence.current < 0) return
    client.startProjection(graphEventSequence.current)
    projectionStarted.current = true
  }, [client, fixtureGraph, graph])
  const wasReconnecting = useRef(false)
  useEffect(() => {
    if (runtimeConnection === 'reconnecting') {
      wasReconnecting.current = true
      return
    }
    if (!wasReconnecting.current) return
    wasReconnecting.current = false
    graphFrameQueue.current?.cancel()
    projectionStarted.current = false
    graphEventSequence.current = -1
    void refresh()
  }, [refresh, runtimeConnection])
  useEffect(() => {
    document.documentElement.dataset.theme = context.theme
    document.body.classList.toggle('light-theme', context.theme === 'light')
    return () => {
      delete document.documentElement.dataset.theme
      document.body.classList.remove('light-theme')
    }
  }, [context.theme])
  const flushGeometry = useCallback((value = latestTransform.current) => {
    if (!client || !value || fixtureGraph || readOnlyRef.current) return Promise.resolve()
    return client.request('geometry.put', {
      sceneId: context.sceneId,
      ownerKey: `dag-viewport:${context.sceneId}`,
      layoutRevision: layoutRevision.current,
      geometry: { panX: value.x, panY: value.y, zoom: value.scale },
      now: Date.now()
    }).then(() => undefined).catch(() => undefined)
  }, [client, context.sceneId, fixtureGraph])
  useEffect(() => {
    const flushBeforeUnload = () => { void flushGeometry() }
    window.addEventListener('beforeunload', flushBeforeUnload)
    return () => {
      window.removeEventListener('beforeunload', flushBeforeUnload)
      void flushGeometry()
    }
  }, [flushGeometry])
  const persistTransform = (value: DagTransform) => {
    latestTransform.current = value
    // The DAG is a short-lived native window and users often close it directly
    // after one zoom or pan gesture. Send every final transform to Runtime as the
    // gesture happens so a native close cannot drop the last observation point.
    // Geometry is deliberately outside the domain outbox, so this stays UI-only.
    void flushGeometry(value)
  }
  const closeAfterGeometryFlush = useCallback(async () => {
    // A transform write may still be crossing the renderer-to-Runtime channel
    // when the user closes this short-lived window. Re-submit the latest value
    // and wait for Runtime's acknowledgement before destroying the renderer;
    // otherwise the next open can restore the previous zoom/pan observation.
    await flushGeometry()
    await window.matouDesktop?.closeDagWindow?.(context.mainWindowId)
  }, [context.mainWindowId, flushGeometry])
  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') void closeAfterGeometryFlush()
    }
    window.addEventListener('keydown', keyDown)
    return () => window.removeEventListener('keydown', keyDown)
  }, [closeAfterGeometryFlush])

  if (!graph || !geometryReady) return <main className="dag-window dag-window-state" aria-label="会话 DAG">
    <strong>{runtimeConnection === 'reconnecting' || error
      ? '会话信息暂时未更新'
      : '正在载入会话关系…'}</strong>
    {(runtimeConnection === 'reconnecting' || error) && <>
      <p>{runtimeConnection === 'reconnecting' ? '正在重新连接，已有会话与关系不会丢失。' : '正在重试载入会话关系。'}</p>
      {runtimeConnection === 'ready' && <button onClick={() => void refresh()}>立即重试</button>}
    </>}
  </main>
  const focusedSessionId = graph.nodes.some(({ sessionId }) => sessionId === context.sessionId)
    ? context.sessionId
    : graph.focusedSessionId ?? graph.nodes[0]?.sessionId ?? ''
  if (firstOperableMs.current === undefined && context.requestedAt !== undefined) {
    firstOperableMs.current = Math.max(0, Date.now() - context.requestedAt)
  }
  return <main className="dag-window" aria-label="会话 DAG"
    data-first-operable-ms={firstOperableMs.current}>
    {readOnly && <div className="dag-runtime-notice" role="status">
      <strong>{READ_ONLY_REASON}</strong>
      <span>会话关系仍可浏览和选择；画布位置变化仅在本次窗口内保留。</span>
    </div>}
    {(runtimeConnection === 'reconnecting' || error) && <div className="dag-runtime-notice" role="status">
      <strong>会话信息暂时未更新</strong>
      <span>{runtimeConnection === 'reconnecting'
        ? '正在重新连接；当前关系图保留，连接恢复后会自动刷新。'
        : '正在重试更新；当前显示上一次成功载入的关系。'}</span>
      {runtimeConnection === 'ready' && error && <button onClick={() => void refresh()}>立即重试</button>}
    </div>}
    <DagCanvas key={context.sceneId} graph={graph} focusedSessionId={focusedSessionId}
      notifiedSessionIds={notifiedSessionIds}
      {...(initialTransform ? { initialTransform } : {})} onTransformChange={persistTransform}
      onSelect={(sessionId) => {
        const target = graph.nodes.find((node) => node.sessionId === sessionId)
        if (!target) return
        void flushGeometry().then(() => window.matouDesktop?.selectDagNode?.({
          mainWindowId: context.mainWindowId,
          sceneId: context.sceneId,
          sessionId,
          theme: context.theme,
          ...(context.notificationSessionIds ? {
            notificationSessionIds: context.notificationSessionIds
          } : {}),
          ...(target.detachedWindowId ? { targetWindowId: target.detachedWindowId } : {})
        }))
      }} />
  </main>
}

function readContext(): DagWindowContext {
  const query = new URLSearchParams(window.location.search)
  const requestedAt = query.get('requestedAt')
  return {
    mainWindowId: query.get('mainWindowId') ?? '',
    sceneId: query.get('sceneId') ?? '',
    sessionId: query.get('sessionId') ?? '',
    theme: query.get('theme') === 'dark' ? 'dark' : 'light',
    ...(requestedAt !== null && Number.isFinite(Number(requestedAt)) ? {
      requestedAt: Number(requestedAt)
    } : {})
  }
}

function graphFromContext(value: unknown, sceneId: string): SessionGraphView | undefined {
  if (typeof value === 'string') {
    try {
      return graphFromContext(JSON.parse(value), sceneId)
    } catch {
      return undefined
    }
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined
  const candidate = value as Partial<SessionGraphView>
  if (candidate.sceneId !== sceneId || !Array.isArray(candidate.nodes) || !Array.isArray(candidate.edges)) {
    return undefined
  }
  return candidate as SessionGraphView
}

function graphFromEvent(event: DomainEventWireEnvelope): SessionGraphView | undefined {
  if (!event.payload || typeof event.payload !== 'object' || Array.isArray(event.payload)) return undefined
  const candidate = 'graph' in event.payload ? event.payload.graph : undefined
  if (!candidate || typeof candidate !== 'object' || Array.isArray(candidate)) return undefined
  if (!('sceneId' in candidate) || typeof candidate.sceneId !== 'string') return undefined
  if (!('nodes' in candidate) || !Array.isArray(candidate.nodes)) return undefined
  if (!('edges' in candidate) || !Array.isArray(candidate.edges)) return undefined
  return candidate as SessionGraphView
}

function eventTouchesGraph(
  event: DomainEventWireEnvelope,
  sceneId: string,
  current: SessionGraphView | null
): boolean {
  if (event.aggregateId === sceneId) return true
  if (event.sessionId && current?.nodes.some(({ sessionId: candidate }) => candidate === event.sessionId)) {
    return event.eventType.startsWith('session.') || event.eventType.startsWith('scene.')
  }
  return false
}
