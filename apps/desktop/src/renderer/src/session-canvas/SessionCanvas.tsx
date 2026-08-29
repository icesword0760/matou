import { useEffect, useRef, useState, type ReactNode } from 'react'

import type { SessionGraphNodeView, SessionGraphView } from '../hierarchy/hierarchy-types'
import { SessionCarousel } from './SessionCarousel'
import { SessionHeader } from './SessionHeader'
import { HistoricalSessionCard } from './HistoricalSessionCard'

export function SessionCanvas(props: {
  graph: SessionGraphView
  levelParentSessionId?: string
  disabled?: boolean
  renderSession(node: SessionGraphNodeView, inViewport: boolean): ReactNode
  onActivate(sessionId: string): void
  onCreateShellSibling(sourceSessionId: string, parentSessionId?: string): void
  onCreateForkSibling(source: SessionGraphNodeView, parent: SessionGraphNodeView): void
  onReopenHistorical(sessionId: string): void
  onReturnParent?(parentSessionId: string): void
  onEnsureSessionVisible?(sessionId: string): void
  geometry?: Array<{ ownerKey: string; geometry: Record<string, unknown> }>
  onPutGeometry?(ownerKey: string, geometry: Record<string, unknown>): void
}) {
  const {
    graph, levelParentSessionId, disabled = false, renderSession, onActivate,
    onCreateShellSibling, onCreateForkSibling, onReopenHistorical, onReturnParent,
    onEnsureSessionVisible, geometry, onPutGeometry
  } = props
  const geometryTimer = useRef<number | undefined>(undefined)
  const pendingGeometry = useRef<{ ownerKey: string; geometry: Record<string, unknown> } | undefined>(undefined)
  const onPutGeometryRef = useRef(onPutGeometry)
  const [showHistory, setShowHistory] = useState(false)
  const activeNodes = graph.nodes.filter(({ archivedAt }) => archivedAt === undefined)
  const focused = activeNodes.find(({ sessionId }) => sessionId === graph.focusedSessionId) ?? activeNodes[0]
  const parentId = levelParentSessionId !== undefined ? levelParentSessionId : focused?.parentSessionId
  const direct = graph.nodes.filter((node) => node.parentSessionId === parentId)
  const activeDirect = direct.filter(({ archivedAt }) => archivedAt === undefined)
  const historicalCount = direct.length - activeDirect.length
  const historyVisible = showHistory || (activeDirect.length === 0 && historicalCount > 0)
  const siblings = historyVisible ? direct : activeDirect
  const parent = parentId ? graph.nodes.find(({ sessionId }) => sessionId === parentId) : undefined
  const ownerKey = `session-group:${graph.sceneId}:${parentId ?? 'root'}`
  const storedGeometry = geometry?.find((item) => item.ownerKey === ownerKey)?.geometry
  const initialScrollLeft = typeof storedGeometry?.scrollLeft === 'number' ? storedGeometry.scrollLeft : 0
  const levelFocus = focused && focused.parentSessionId === parentId ? focused : activeDirect[0] ?? direct[0]
  useEffect(() => { setShowHistory(false) }, [parentId])
  useEffect(() => { onPutGeometryRef.current = onPutGeometry }, [onPutGeometry])
  useEffect(() => () => {
    if (geometryTimer.current !== undefined) window.clearTimeout(geometryTimer.current)
    const pending = pendingGeometry.current
    if (pending) onPutGeometryRef.current?.(pending.ownerKey, pending.geometry)
  }, [])
  const putGeometry = (next: { scrollLeft: number; focusedSessionId?: string }) => {
    pendingGeometry.current = { ownerKey, geometry: next }
    if (geometryTimer.current !== undefined) window.clearTimeout(geometryTimer.current)
    geometryTimer.current = window.setTimeout(() => {
      geometryTimer.current = undefined
      const pending = pendingGeometry.current
      pendingGeometry.current = undefined
      if (pending) onPutGeometryRef.current?.(pending.ownerKey, pending.geometry)
    }, 180)
  }
  if (!levelFocus) return <div className="session-canvas-empty" role="status">当前画布没有活跃会话</div>

  return <section className="session-canvas" aria-label="会话画布" data-parent-session-id={parentId ?? ''}>
    <SessionHeader {...(parent ? { parentTitle: parent.title } : {})} sessionCount={siblings.length}
      canForkSibling={parent?.canFork === true} disabled={disabled}
      historicalCount={historicalCount} showHistory={historyVisible}
      onToggleHistory={() => setShowHistory((value) => !value)}
      {...(parent && onReturnParent ? { onReturnParent: () => onReturnParent(parent.sessionId) } : {})}
      onAddShell={() => onCreateShellSibling(levelFocus.sessionId, parentId)}
      onAddForkSibling={() => parent && onCreateForkSibling(levelFocus, parent)} />
    <SessionCarousel nodes={siblings} focusedSessionId={levelFocus.sessionId}
      renderSession={(node, inViewport) => node.archivedAt === undefined
        ? renderSession(node, inViewport)
        : <HistoricalSessionCard node={node} onReopen={onReopenHistorical} />}
      onActivate={(sessionId) => {
        const node = graph.nodes.find((candidate) => candidate.sessionId === sessionId)
        if (node?.archivedAt === undefined) onActivate(sessionId)
      }}
      {...(parent ? { parent } : {})}
      {...(parent && onReturnParent
        ? { onCommitParent: () => onReturnParent(parent.sessionId) }
        : {})}
      geometryKey={ownerKey} initialScrollLeft={initialScrollLeft}
      onGeometryChange={putGeometry}
      {...(onEnsureSessionVisible ? { onEnsureSessionVisible } : {})} />
  </section>
}
