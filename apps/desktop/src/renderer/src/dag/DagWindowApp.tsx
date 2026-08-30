import { useCallback, useEffect, useRef, useState } from 'react'

import type { DagWindowContext } from '../../../shared/desktop-api'
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
  const geometryTimer = useRef<number | undefined>(undefined)
  const pendingTransform = useRef<DagTransform | undefined>(undefined)
  const layoutRevision = useRef(0)
  const graphSignature = useRef('')
  const [error, setError] = useState('')
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
  }), [])
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
      if (stored) layoutRevision.current = stored.layoutRevision
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
  const flushGeometry = useCallback(() => {
    if (geometryTimer.current !== undefined) window.clearTimeout(geometryTimer.current)
    geometryTimer.current = undefined
    const value = pendingTransform.current
    pendingTransform.current = undefined
    if (!client || !value || fixtureGraph) return
    void client.request('geometry.put', {
      sceneId: context.sceneId,
      ownerKey: `dag-viewport:${context.sceneId}`,
      layoutRevision: layoutRevision.current,
      geometry: { panX: value.x, panY: value.y, zoom: value.scale },
      now: Date.now()
    }).catch(() => {})
  }, [client, context.sceneId, fixtureGraph])
  useEffect(() => {
    window.addEventListener('beforeunload', flushGeometry)
    return () => {
      window.removeEventListener('beforeunload', flushGeometry)
      flushGeometry()
    }
  }, [flushGeometry])
  const persistTransform = (value: DagTransform) => {
    pendingTransform.current = value
    if (geometryTimer.current !== undefined) window.clearTimeout(geometryTimer.current)
    geometryTimer.current = window.setTimeout(flushGeometry, 180)
  }

  if (!graph || !geometryReady) return <main className="dag-window dag-window-state" aria-label="会话 DAG">
    <strong>{error ? '会话关系载入异常' : '正在载入会话关系…'}</strong>
    {error && <><p>{error}</p><button onClick={() => void refresh()}>重试</button></>}
  </main>
  const focusedSessionId = graph.nodes.some(({ sessionId }) => sessionId === context.sessionId)
    ? context.sessionId
    : graph.focusedSessionId ?? graph.nodes[0]?.sessionId ?? ''
  return <main className="dag-window" aria-label="会话 DAG">
    <div className="dag-window-title"><span>Matou 会话画布</span>
      <button aria-label="关闭 DAG" onClick={() => void window.matouDesktop?.closeDagWindow?.(context.mainWindowId)}>×</button>
    </div>
    <DagCanvas key={context.sceneId} graph={graph} focusedSessionId={focusedSessionId}
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
      node.activeChildCount, node.historicalChildCount
    ]),
    graph.edges
  ])
}
