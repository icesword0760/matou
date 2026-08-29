import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type WheelEvent } from 'react'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { SessionCard } from './SessionCard'

export function SessionCarousel(props: {
  nodes: SessionGraphNodeView[]
  focusedSessionId?: string
  renderSession(node: SessionGraphNodeView, inViewport: boolean): ReactNode
  onActivate(sessionId: string): void
  onEnsureSessionVisible?(sessionId: string): void
}) {
  const { nodes, focusedSessionId, renderSession, onActivate, onEnsureSessionVisible } = props
  const viewportRef = useRef<HTMLDivElement>(null)
  const cardsRef = useRef(new Map<string, HTMLElement>())
  const previousRectsRef = useRef(new Map<string, DOMRect>())
  const ensureVisibleRef = useRef(onEnsureSessionVisible)
  const scrollTimer = useRef<number | undefined>(undefined)
  const [firstVisible, setFirstVisible] = useState(0)
  const [hoveredSessionId, setHoveredSessionId] = useState<string | null>(null)
  const [scrolling, setScrolling] = useState(false)
  const visibleCount = Math.min(4, Math.max(1, nodes.length))
  const inViewport = useMemo(() => new Set(
    nodes.slice(firstVisible, firstVisible + visibleCount).map(({ sessionId }) => sessionId)
  ), [firstVisible, nodes, visibleCount])

  useEffect(() => () => {
    if (scrollTimer.current !== undefined) window.clearTimeout(scrollTimer.current)
  }, [])
  useEffect(() => { ensureVisibleRef.current = onEnsureSessionVisible }, [onEnsureSessionVisible])

  useLayoutEffect(() => {
    const next = new Map<string, DOMRect>()
    for (const node of nodes) {
      const element = cardsRef.current.get(node.sessionId)
      if (!element) continue
      const rect = element.getBoundingClientRect()
      next.set(node.sessionId, rect)
      const previous = previousRectsRef.current.get(node.sessionId)
      const deltaX = previous ? previous.left - rect.left : 0
      if (previous && Math.abs(deltaX) > 0.5 && !reducedMotion()) {
        element.animate?.(
          [{ transform: `translateX(${deltaX}px)` }, { transform: 'translateX(0)' }],
          { duration: 180, easing: 'cubic-bezier(.2,.8,.2,1)' }
        )
      }
    }
    previousRectsRef.current = next
  }, [nodes])

  useEffect(() => {
    if (!focusedSessionId) return
    const frame = requestAnimationFrame(() => {
      cardsRef.current.get(focusedSessionId)?.scrollIntoView?.({ behavior: 'smooth', inline: 'center', block: 'nearest' })
      ensureVisibleRef.current?.(focusedSessionId)
    })
    return () => cancelAnimationFrame(frame)
  }, [focusedSessionId])

  const updateVisibleWindow = () => {
    const viewport = viewportRef.current
    if (!viewport || nodes.length <= visibleCount) {
      setFirstVisible(0)
      return
    }
    const unit = viewport.clientWidth > 0 ? viewport.clientWidth / visibleCount : 1
    setFirstVisible(Math.max(0, Math.min(nodes.length - visibleCount, Math.round(viewport.scrollLeft / unit))))
  }
  const markScrolling = () => {
    setScrolling(true)
    setHoveredSessionId(null)
    updateVisibleWindow()
    if (scrollTimer.current !== undefined) window.clearTimeout(scrollTimer.current)
    scrollTimer.current = window.setTimeout(() => setScrolling(false), 120)
  }
  const wheel = (event: WheelEvent<HTMLDivElement>) => {
    if (event.ctrlKey || event.metaKey) return
    const viewport = viewportRef.current
    if (!viewport) return
    const overTerminal = (event.target as HTMLElement).closest('.terminal-surface') !== null
    const horizontal = Math.abs(event.deltaX) >= Math.abs(event.deltaY)
    if (!horizontal && overTerminal) return
    const delta = horizontal ? event.deltaX : event.deltaY
    if (delta === 0) return
    event.preventDefault()
    viewport.scrollLeft += delta
    markScrolling()
  }

  return <div className="session-carousel" ref={viewportRef} role="region" aria-label="同级会话列表"
    data-visible-columns={visibleCount} onScroll={markScrolling} onWheel={wheel}
    style={{ '--session-visible-columns': visibleCount } as React.CSSProperties}>
    {nodes.map((node) => <div key={node.sessionId} ref={(element) => {
      if (element) cardsRef.current.set(node.sessionId, element)
      else cardsRef.current.delete(node.sessionId)
    }} className="session-card-slot">
      <SessionCard node={node} focused={node.sessionId === focusedSessionId}
        inViewport={inViewport.has(node.sessionId)}
        expanded={!scrolling && hoveredSessionId === node.sessionId}
        onActivate={onActivate} onHover={setHoveredSessionId}>
        {renderSession(node, inViewport.has(node.sessionId))}
      </SessionCard>
    </div>)}
  </div>
}

function reducedMotion(): boolean {
  return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
}
