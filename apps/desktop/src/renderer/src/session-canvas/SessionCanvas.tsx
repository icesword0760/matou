import { useEffect, useRef, useState, type ReactNode } from 'react'

import type { SessionGraphNodeView, SessionGraphView } from '../hierarchy/hierarchy-types'
import { SessionCarousel } from './SessionCarousel'
import { SessionHeader } from './SessionHeader'
import { HistoricalSessionCard } from './HistoricalSessionCard'

export function SessionCanvas(props: {
  graph: SessionGraphView
  levelParentSessionId?: string | null
  disabled?: boolean
  renderSession(node: SessionGraphNodeView, inViewport: boolean): ReactNode
  onActivate(sessionId: string): void
  onCreateShellSibling(sourceSessionId: string, parentSessionId?: string): void
  onCreateForkSibling(source: SessionGraphNodeView, parent: SessionGraphNodeView): void
  onReopenHistorical(sessionId: string): void
  onNavigateToChildren?(sessionId: string): void
  onRemoveHistorical?(sessionId: string, includeDescendants: boolean): void
  onReturnParent?(parentSessionId: string): void
  onEnsureSessionVisible?(sessionId: string): void
  revealRequest?: { sessionId: string; sequence: number; historical?: boolean }
  geometry?: Array<{ ownerKey: string; geometry: Record<string, unknown> }>
  onPutGeometry?(ownerKey: string, geometry: Record<string, unknown>): unknown
}) {
  const {
    graph, levelParentSessionId, disabled = false, renderSession, onActivate,
    onCreateShellSibling, onCreateForkSibling, onReopenHistorical, onNavigateToChildren,
    onRemoveHistorical, onReturnParent,
    onEnsureSessionVisible, revealRequest, geometry, onPutGeometry
  } = props
  const geometryTimer = useRef<number | undefined>(undefined)
  const pendingGeometry = useRef<{ ownerKey: string; geometry: Record<string, unknown> } | undefined>(undefined)
  const lastContinuousGeometryWrite = useRef<number | undefined>(undefined)
  const geometryRetryCount = useRef(0)
  const geometryWritesInFlight = useRef(0)
  const geometryWriteSequence = useRef(0)
  const geometryAckSequence = useRef(0)
  const onPutGeometryRef = useRef(onPutGeometry)
  const [showHistory, setShowHistory] = useState(false)
  const [geometryPending, setGeometryPending] = useState(false)
  const [lastSavedScrollLeft, setLastSavedScrollLeft] = useState<number | undefined>(undefined)
  const activeNodes = graph.nodes.filter(({ archivedAt }) => archivedAt === undefined)
  const focused = activeNodes.find(({ sessionId }) => sessionId === graph.focusedSessionId) ?? activeNodes[0]
  const parentId = levelParentSessionId !== undefined
    ? levelParentSessionId ?? undefined
    : focused?.parentSessionId
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
  useEffect(() => {
    if (!revealRequest?.historical) return
    if (direct.some(({ sessionId }) => sessionId === revealRequest.sessionId)) setShowHistory(true)
  }, [direct, revealRequest?.historical, revealRequest?.sequence, revealRequest?.sessionId])
  useEffect(() => { onPutGeometryRef.current = onPutGeometry }, [onPutGeometry])
  useEffect(() => () => {
    if (geometryTimer.current !== undefined) window.clearTimeout(geometryTimer.current)
    const pending = pendingGeometry.current
    if (pending) onPutGeometryRef.current?.(pending.ownerKey, pending.geometry)
  }, [])
  const flushGeometry = () => {
    if (geometryTimer.current !== undefined) window.clearTimeout(geometryTimer.current)
    geometryTimer.current = undefined
    const pending = pendingGeometry.current
    pendingGeometry.current = undefined
    if (!pending) return
    lastContinuousGeometryWrite.current = performance.now()
    geometryWritesInFlight.current += 1
    const writeSequence = ++geometryWriteSequence.current
    const write = onPutGeometryRef.current?.(pending.ownerKey, pending.geometry)
    void Promise.resolve(write).then(() => {
      geometryRetryCount.current = 0
      if (writeSequence >= geometryAckSequence.current) {
        geometryAckSequence.current = writeSequence
        if (typeof pending.geometry.scrollLeft === 'number') {
          setLastSavedScrollLeft(pending.geometry.scrollLeft)
        }
      }
    }).catch(() => {
      // A Session added immediately before the scroll may leave this render on
      // the prior layout revision for a brief moment. Retry through the latest
      // callback after the authoritative projection catches up.
      if (geometryWriteSequence.current !== writeSequence || pendingGeometry.current !== undefined) return
      if (geometryRetryCount.current >= 3) return
      geometryRetryCount.current += 1
      pendingGeometry.current = pending
      geometryTimer.current = window.setTimeout(flushGeometry, 120)
    }).finally(() => {
      geometryWritesInFlight.current = Math.max(0, geometryWritesInFlight.current - 1)
      if (geometryWritesInFlight.current === 0 && geometryTimer.current === undefined &&
        pendingGeometry.current === undefined) setGeometryPending(false)
    })
  }
  const putGeometry = (
    next: { scrollLeft: number; focusedSessionId?: string },
    options?: { continuous?: boolean }
  ) => {
    geometryRetryCount.current = 0
    setGeometryPending(true)
    pendingGeometry.current = { ownerKey, geometry: next }
    if (geometryTimer.current !== undefined) window.clearTimeout(geometryTimer.current)
    if (options?.continuous) {
      const lastWrite = lastContinuousGeometryWrite.current
      const elapsed = lastWrite === undefined ? Number.POSITIVE_INFINITY : performance.now() - lastWrite
      if (elapsed >= 50) {
        flushGeometry()
        return
      }
      geometryTimer.current = window.setTimeout(flushGeometry, Math.max(1, 50 - elapsed))
      return
    }
    geometryTimer.current = window.setTimeout(() => {
      flushGeometry()
    }, 180)
  }
  if (!levelFocus) return <div className="session-canvas-empty" role="status">当前画布没有活跃会话</div>

  return <section className="session-canvas" aria-label="会话画布" aria-busy={geometryPending}
    data-last-saved-scroll-left={lastSavedScrollLeft}
    data-parent-session-id={parentId ?? ''}>
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
        : <HistoricalSessionCard node={node}
            directChildCount={directChildren(graph, node.sessionId).length}
            descendantCount={descendants(graph, node.sessionId).length}
            onReopen={onReopenHistorical}
            {...(onNavigateToChildren ? { onNavigateToChildren } : {})}
            onRemove={onRemoveHistorical ?? NOOP_REMOVE} />}
      onActivate={(sessionId) => {
        const node = graph.nodes.find((candidate) => candidate.sessionId === sessionId)
        if (node?.archivedAt === undefined) onActivate(sessionId)
      }}
      {...(parent ? { parent } : {})}
      {...(parent && onReturnParent
        ? { onCommitParent: () => onReturnParent(parent.sessionId) }
        : {})}
      geometryKey={ownerKey} initialScrollLeft={initialScrollLeft}
      {...(revealRequest ? { revealRequest } : {})}
      onGeometryChange={putGeometry}
      {...(onEnsureSessionVisible ? { onEnsureSessionVisible } : {})} />
  </section>
}

function directChildren(graph: SessionGraphView, sessionId: string): string[] {
  return graph.edges.filter(({ parentSessionId }) => parentSessionId === sessionId)
    .map(({ childSessionId }) => childSessionId)
}

function descendants(graph: SessionGraphView, sessionId: string): string[] {
  const found: string[] = []
  const queue = [...directChildren(graph, sessionId)]
  while (queue.length > 0) {
    const current = queue.shift()!
    if (found.includes(current)) continue
    found.push(current)
    queue.push(...directChildren(graph, current))
  }
  return found
}

function NOOP_REMOVE(): void {}
