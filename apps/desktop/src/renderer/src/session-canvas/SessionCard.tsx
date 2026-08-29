import type { ReactNode } from 'react'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'

export function SessionCard(props: {
  node: SessionGraphNodeView
  focused: boolean
  inViewport: boolean
  expanded: boolean
  children: ReactNode
  onActivate(sessionId: string): void
  onHover(sessionId: string | null): void
}) {
  const { node, focused, inViewport, expanded, children, onActivate, onHover } = props
  return <article className={`session-card${focused ? ' is-focused' : ''}${expanded ? ' is-expanded' : ''}`}
    data-session-card={node.sessionId} data-in-viewport={inViewport}
    aria-label={`会话：${node.title}`} aria-current={focused ? 'true' : undefined}
    onPointerEnter={() => onHover(node.sessionId)} onPointerLeave={() => onHover(null)}
    onFocusCapture={() => onActivate(node.sessionId)}>
    {children}
  </article>
}
