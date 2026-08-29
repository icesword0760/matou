import { useEffect, useState, type ReactNode } from 'react'

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
  onEnsureSessionVisible?(sessionId: string): void
}) {
  const {
    graph, levelParentSessionId, disabled = false, renderSession, onActivate,
    onCreateShellSibling, onCreateForkSibling, onReopenHistorical, onEnsureSessionVisible
  } = props
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
  const levelFocus = focused && focused.parentSessionId === parentId ? focused : activeDirect[0] ?? direct[0]
  useEffect(() => { setShowHistory(false) }, [parentId])
  if (!levelFocus) return <div className="session-canvas-empty" role="status">当前画布没有活跃会话</div>

  return <section className="session-canvas" aria-label="会话画布" data-parent-session-id={parentId ?? ''}>
    <SessionHeader {...(parent ? { parentTitle: parent.title } : {})} sessionCount={siblings.length}
      canForkSibling={parent?.canFork === true} disabled={disabled}
      historicalCount={historicalCount} showHistory={historyVisible}
      onToggleHistory={() => setShowHistory((value) => !value)}
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
      {...(onEnsureSessionVisible ? { onEnsureSessionVisible } : {})} />
  </section>
}
