import type { ReactNode } from 'react'

import type { SessionGraphNodeView, SessionGraphView } from '../hierarchy/hierarchy-types'
import { SessionCarousel } from './SessionCarousel'
import { SessionHeader } from './SessionHeader'

export function SessionCanvas(props: {
  graph: SessionGraphView
  disabled?: boolean
  renderSession(node: SessionGraphNodeView, inViewport: boolean): ReactNode
  onActivate(sessionId: string): void
  onCreateShellSibling(sourceSessionId: string): void
  onCreateForkSibling(source: SessionGraphNodeView, parent: SessionGraphNodeView): void
  onEnsureSessionVisible?(sessionId: string): void
}) {
  const {
    graph, disabled = false, renderSession, onActivate,
    onCreateShellSibling, onCreateForkSibling, onEnsureSessionVisible
  } = props
  const activeNodes = graph.nodes.filter(({ archivedAt }) => archivedAt === undefined)
  const focused = activeNodes.find(({ sessionId }) => sessionId === graph.focusedSessionId) ?? activeNodes[0]
  if (!focused) return <div className="session-canvas-empty" role="status">当前画布没有活跃会话</div>
  const parentId = focused.parentSessionId
  const siblings = activeNodes.filter((node) => node.parentSessionId === parentId)
  const parent = parentId ? graph.nodes.find(({ sessionId }) => sessionId === parentId) : undefined

  return <section className="session-canvas" aria-label="会话画布" data-parent-session-id={parentId ?? ''}>
    <SessionHeader {...(parent ? { parentTitle: parent.title } : {})} sessionCount={siblings.length}
      canForkSibling={parent?.canFork === true} disabled={disabled}
      onAddShell={() => onCreateShellSibling(focused.sessionId)}
      onAddForkSibling={() => parent && onCreateForkSibling(focused, parent)} />
    <SessionCarousel nodes={siblings} focusedSessionId={focused.sessionId}
      renderSession={renderSession} onActivate={onActivate}
      {...(onEnsureSessionVisible ? { onEnsureSessionVisible } : {})} />
  </section>
}
