import { useCallback, useEffect, useRef, useState } from 'react'

import type { DagWindowContext, RuntimeConnectionState } from '../../../shared/desktop-api'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import type { SessionGraphView } from '../hierarchy/hierarchy-types'
import { DagCanvas, type DagTransform } from './DagCanvas'
import './dag.css'

export function DagWindowApp({ fixtureGraph }: { fixtureGraph?: SessionGraphView }) {
  const client = useRuntimeClient()
  const [context, setContext] = useState(readContext)
  const [graph, setGraph] = useState<SessionGraphView | null>(fixtureGraph ?? null)
  const [initialTransform, setInitialTransform] = useState<DagTransform | undefined>(undefined)
  const [geometryReady, setGeometryReady] = useState(Boolean(fixtureGraph))
  const latestTransform = useRef<DagTransform | undefined>(undefined)
  const layoutRevision = useRef(0)
  const graphSignature = useRef('')
  const [error, setError] = useState('')
  const [runtimeConnection, setRuntimeConnection] = useState<RuntimeConnectionState>('ready')
  const [notifiedSessionIds, setNotifiedSessionIds] = useState<string[]>(
    context.notificationSessionIds ?? []
  )
  const refresh = useCallback(async () => {
    if (fixtureGraph || !client) return
    try {
      const next = await client.request<SessionGraphView>('hierarchy.get-scene-session-graph', {
        sceneId: context.sceneId,
        windowId: context.mainWindowId
      })
      const signature = sessionGraphSignature(next)
      if (signature !== graphSignature.current) {
        graphSignature.current = signature
        setGraph(next)
      }
      layoutRevision.current = next.layoutRevision ?? layoutRevision.current
      setError('')
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    }
  }, [client, context.mainWindowId, context.sceneId, fixtureGraph])

  useEffect(() => window.matouDesktop?.onDagContext?.((next) => {
    setContext(next)
    setGraph(null)
    graphSignature.current = ''
    setInitialTransform(undefined)
    setGeometryReady(false)
    setNotifiedSessionIds(next.notificationSessionIds ?? [])
  }), [])
  useEffect(() => window.matouDesktop?.onDagNotifications?.(setNotifiedSessionIds), [])
  useEffect(() => window.matouDesktop?.onRuntimeConnectionState?.(setRuntimeConnection), [])
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
    void refresh()
    if (fixtureGraph) return
    const timer = window.setInterval(() => { void refresh() }, 500)
    return () => window.clearInterval(timer)
  }, [fixtureGraph, refresh])
  useEffect(() => {
    document.documentElement.dataset.theme = context.theme
    document.body.classList.toggle('light-theme', context.theme === 'light')
    return () => {
      delete document.documentElement.dataset.theme
      document.body.classList.remove('light-theme')
    }
  }, [context.theme])
  useEffect(() => {
    const keyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') void window.matouDesktop?.closeDagWindow?.(context.mainWindowId)
    }
    window.addEventListener('keydown', keyDown)
    return () => window.removeEventListener('keydown', keyDown)
  }, [context.mainWindowId])
  const flushGeometry = useCallback((value = latestTransform.current) => {
    if (!client || !value || fixtureGraph) return Promise.resolve()
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
  return <main className="dag-window" aria-label="会话 DAG">
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
      void window.matouDesktop?.selectDagNode?.({
        ...context,
        sessionId,
        ...(target.detachedWindowId ? { targetWindowId: target.detachedWindowId } : {})
      })
    }} />
  </main>
}

function readContext(): DagWindowContext {
  const query = new URLSearchParams(window.location.search)
  return {
    mainWindowId: query.get('mainWindowId') ?? '',
    sceneId: query.get('sceneId') ?? '',
    sessionId: query.get('sessionId') ?? '',
    theme: query.get('theme') === 'dark' ? 'dark' : 'light'
  }
}

function sessionGraphSignature(graph: SessionGraphView): string {
  return JSON.stringify([
    graph.layoutRevision,
    graph.focusedSessionId,
    graph.nodes.map((node) => [
      node.sessionId, node.parentSessionId, node.currentMode, node.workStatus,
      node.providerRestoreState, node.title, node.cwd, node.archivedAt,
      node.detachedWindowId, node.latestLines, node.lastActivityAt,
      node.activeChildCount, node.stoppedChildCount
    ]),
    graph.edges
  ])
}
