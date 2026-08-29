import { useCallback, useEffect, useState } from 'react'

import type { DagWindowContext } from '../../../shared/desktop-api'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import type { SessionGraphView } from '../hierarchy/hierarchy-types'
import { DagCanvas } from './DagCanvas'
import './dag.css'

export function DagWindowApp({ fixtureGraph }: { fixtureGraph?: SessionGraphView }) {
  const client = useRuntimeClient()
  const [context, setContext] = useState(readContext)
  const [graph, setGraph] = useState<SessionGraphView | null>(fixtureGraph ?? null)
  const [error, setError] = useState('')
  const refresh = useCallback(async () => {
    if (fixtureGraph || !client) return
    try {
      const next = await client.request<SessionGraphView>('hierarchy.get-scene-session-graph', {
        sceneId: context.sceneId,
        windowId: context.mainWindowId
      })
      setGraph(next)
      setError('')
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    }
  }, [client, context.mainWindowId, context.sceneId, fixtureGraph])

  useEffect(() => window.matouDesktop?.onDagContext?.((next) => {
    setContext(next)
    setGraph(null)
  }), [])
  useEffect(() => {
    void refresh()
    if (fixtureGraph) return
    const timer = window.setInterval(() => { void refresh() }, 250)
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

  if (!graph) return <main className="dag-window dag-window-state" aria-label="会话 DAG">
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
    <DagCanvas graph={graph} focusedSessionId={focusedSessionId} onSelect={(sessionId) => {
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
