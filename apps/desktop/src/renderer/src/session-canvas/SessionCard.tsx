import type { FocusEvent, ReactNode } from 'react'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'

export function SessionCard(props: {
  node: SessionGraphNodeView
  focused: boolean
  inViewport: boolean
  expanded: boolean
  children: ReactNode
  onActivate(sessionId: string): void
  onHover(sessionId: string): void
}) {
  const { node, focused, inViewport, expanded, children, onActivate, onHover } = props
  return <article className={`session-card${focused ? ' is-focused' : ''}${expanded ? ' is-expanded' : ''}`}
    data-session-card={node.sessionId} data-in-viewport={inViewport}
    aria-label={`会话：${node.title}`} aria-current={focused ? 'true' : undefined}
    onMouseEnter={() => onHover(node.sessionId)}
    onPointerDownCapture={(event) => {
      // A mounted terminal may keep its hidden textarea focused even after
      // another Session becomes active. A fresh click must still activate the
      // card immediately instead of depending on a second focus event.
      if (!focused && (event.target as HTMLElement).closest('.terminal-surface')) {
        onActivate(node.sessionId)
      }
    }}
    onFocusCapture={(event: FocusEvent<HTMLElement>) => {
      // Header actions must complete on their original DOM node. Activating the
      // card on button focus refreshes the projection between pointer-down and
      // click, which otherwise drops the user's requested action.
      if (!focused && !(event.target as HTMLElement).closest('button,[role="menuitem"]')) {
        onActivate(node.sessionId)
      }
    }}>
    {children}
  </article>
}
