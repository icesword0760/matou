import { useEffect, useRef, useState, type ReactNode } from 'react'

import type { SessionGraphNodeView, SessionGraphView } from '../hierarchy/hierarchy-types'
import { SessionCarousel } from './SessionCarousel'
import { StoppedSessionCard } from './StoppedSessionCard'

export function SessionCanvas(props: {
  graph: SessionGraphView
  levelParentSessionId?: string | null
  disabled?: boolean
  disabledReason?: string
  renderSession(node: SessionGraphNodeView, inViewport: boolean): ReactNode
  onActivate(sessionId: string): void
  onRemoveBranch?(sessionId: string, includeDescendants: boolean): unknown
  onNavigateToChildren?(sessionId: string): void
  onReturnParent?(parentSessionId: string): void
  onEnsureSessionVisible?(sessionId: string): void
  revealRequest?: { sessionId: string; sequence: number; stopped?: boolean }
  geometry?: Array<{ ownerKey: string; geometry: Record<string, unknown> }>
  onPutGeometry?(ownerKey: string, geometry: Record<string, unknown>): unknown
}) {
  const {
    graph, levelParentSessionId, disabled = false, disabledReason, renderSession, onActivate,
    onRemoveBranch, onNavigateToChildren,
    onReturnParent,
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
  const [geometryPending, setGeometryPending] = useState(false)
  const [lastSavedScrollLeft, setLastSavedScrollLeft] = useState<number | undefined>(undefined)
  const focused = graph.nodes.find(({ sessionId }) => sessionId === graph.focusedSessionId) ?? graph.nodes[0]
  const parentId = levelParentSessionId !== undefined
    ? levelParentSessionId ?? undefined
    : focused?.parentSessionId
  const direct = graph.nodes.filter((node) => node.parentSessionId === parentId)
  const siblings = direct
  const parent = parentId ? graph.nodes.find(({ sessionId }) => sessionId === parentId) : undefined
  const ownerKey = `session-group:${graph.sceneId}:${parentId ?? 'root'}`
  const storedGeometry = geometry?.find((item) => item.ownerKey === ownerKey)?.geometry
  const initialScrollLeft = typeof storedGeometry?.scrollLeft === 'number' ? storedGeometry.scrollLeft : 0
  const initialAnchor = typeof storedGeometry?.anchorSessionId === 'string' &&
    typeof storedGeometry?.anchorViewportOffset === 'number'
    ? {
        sessionId: storedGeometry.anchorSessionId,
        viewportOffset: storedGeometry.anchorViewportOffset
      }
    : undefined
  const levelFocus = focused && focused.parentSessionId === parentId ? focused : direct[0]
  useEffect(() => {
    onPutGeometryRef.current = disabled ? undefined : onPutGeometry
    if (!disabled) return
    if (geometryTimer.current !== undefined) window.clearTimeout(geometryTimer.current)
    geometryTimer.current = undefined
    pendingGeometry.current = undefined
    setGeometryPending(false)
  }, [disabled, onPutGeometry])
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
    next: {
      scrollLeft: number
      focusedSessionId?: string
      anchorSessionId?: string
      anchorViewportOffset?: number
    },
    options?: { continuous?: boolean }
  ) => {
    if (disabled) return
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
    <SessionCarousel nodes={siblings} focusedSessionId={levelFocus.sessionId}
      renderSession={(node, inViewport) => {
        if (node.archivedAt === undefined) return renderSession(node, inViewport)
        const directChildren = graph.nodes.filter(({ parentSessionId }) => parentSessionId === node.sessionId)
        const descendants = sessionDescendants(graph.nodes, node.sessionId)
        return <StoppedSessionCard node={node}
          directChildCount={directChildren.length} descendantCount={descendants.length}
          descendantImpact={{
            running: descendants.filter(({ workStatus }) =>
              workStatus === 'running' || workStatus === 'starting').length,
            needsInput: descendants.filter(({ workStatus }) => workStatus === 'needs-input').length
          }}
          disabled={disabled} {...(disabledReason ? { disabledReason } : {})}
          {...(onRemoveBranch ? { onRemoveBranch } : {})} />
      }}
      onActivate={(sessionId) => {
        const node = graph.nodes.find((candidate) => candidate.sessionId === sessionId)
        if (node?.archivedAt === undefined) onActivate(sessionId)
      }}
      {...(parent ? { parent } : {})}
      {...(parent && onReturnParent
        ? { onCommitParent: () => onReturnParent(parent.sessionId) }
        : {})}
      geometryKey={ownerKey} initialScrollLeft={initialScrollLeft}
      {...(initialAnchor ? { initialAnchor } : {})}
      {...(revealRequest ? { revealRequest } : {})}
      {...(disabled ? {} : { onGeometryChange: putGeometry })}
      {...(onEnsureSessionVisible ? { onEnsureSessionVisible } : {})} />
  </section>
}

function sessionDescendants(nodes: SessionGraphNodeView[], sessionId: string): SessionGraphNodeView[] {
  const children = nodes.filter(({ parentSessionId }) => parentSessionId === sessionId)
  return children.flatMap((child) => [child, ...sessionDescendants(nodes, child.sessionId)])
}
