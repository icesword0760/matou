import {
  useEffect, useLayoutEffect, useMemo, useRef, useState,
  type PointerEvent, type ReactNode, type WheelEvent
} from 'react'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { ParentProjection } from './ParentProjection'
import { ParentPullController } from './ParentPullController'
import { SessionCard } from './SessionCard'

export function SessionCarousel(props: {
  nodes: SessionGraphNodeView[]
  focusedSessionId?: string
  renderSession(node: SessionGraphNodeView, inViewport: boolean): ReactNode
  onActivate(sessionId: string): void
  onEnsureSessionVisible?(sessionId: string): void
  parent?: SessionGraphNodeView
  onCommitParent?(parentSessionId: string): void
}) {
  const {
    nodes, focusedSessionId, renderSession, onActivate, onEnsureSessionVisible,
    parent, onCommitParent
  } = props
  const viewportRef = useRef<HTMLDivElement>(null)
  const cardsRef = useRef(new Map<string, HTMLElement>())
  const previousRectsRef = useRef(new Map<string, DOMRect>())
  const ensureVisibleRef = useRef(onEnsureSessionVisible)
  const scrollTimer = useRef<number | undefined>(undefined)
  const wheelTimer = useRef<number | undefined>(undefined)
  const wheelGesture = useRef(false)
  const pointerGesture = useRef<{
    id: number
    startX: number
    startY: number
    lastX: number
    initialScrollLeft: number
  } | null>(null)
  const pullController = useRef(new ParentPullController())
  const [firstVisible, setFirstVisible] = useState(0)
  const [hoveredSessionId, setHoveredSessionId] = useState<string | null>(null)
  const [scrolling, setScrolling] = useState(false)
  const [pull, setPull] = useState({ distance: 0, progress: 0, springBack: false })
  const visibleCount = Math.min(4, Math.max(1, nodes.length))
  const inViewport = useMemo(() => new Set(
    nodes.slice(firstVisible, firstVisible + visibleCount).map(({ sessionId }) => sessionId)
  ), [firstVisible, nodes, visibleCount])

  useEffect(() => () => {
    if (scrollTimer.current !== undefined) window.clearTimeout(scrollTimer.current)
    if (wheelTimer.current !== undefined) window.clearTimeout(wheelTimer.current)
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
  const finishPullGesture = () => {
    const viewport = viewportRef.current
    if (!viewport) return
    const result = pullController.current.end({
      scrollLeft: viewport.scrollLeft,
      viewportWidth: viewport.clientWidth
    })
    wheelGesture.current = false
    if (result.commit && parent) {
      setPull({ distance: 0, progress: 0, springBack: false })
      onCommitParent?.(parent.sessionId)
      return
    }
    if (result.springBack) {
      setPull((current) => ({ ...current, distance: 0, progress: 0, springBack: true }))
      window.setTimeout(() => setPull({ distance: 0, progress: 0, springBack: false }), reducedMotion() ? 1 : 260)
    } else {
      setPull({ distance: 0, progress: 0, springBack: false })
    }
  }
  const scheduleWheelEnd = () => {
    if (wheelTimer.current !== undefined) window.clearTimeout(wheelTimer.current)
    wheelTimer.current = window.setTimeout(() => {
      wheelTimer.current = undefined
      finishPullGesture()
    }, 110)
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
    if (!wheelGesture.current) {
      wheelGesture.current = true
      pullController.current.begin({ scrollLeft: viewport.scrollLeft, hasParent: Boolean(parent) })
    }
    const movement = pullController.current.move({
      deltaTowardParent: -delta,
      viewportWidth: viewport.clientWidth,
      verticalDominant: !horizontal
    })
    if (movement.consume) {
      event.preventDefault()
      setPull({ distance: movement.pullDistance, progress: movement.progress, springBack: false })
      scheduleWheelEnd()
      return
    }
    event.preventDefault()
    viewport.scrollLeft = Math.max(0, viewport.scrollLeft + delta)
    markScrolling()
    scheduleWheelEnd()
  }

  const pointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || (event.target as HTMLElement).closest('.terminal-surface')) return
    const viewport = viewportRef.current
    if (!viewport) return
    pointerGesture.current = {
      id: event.pointerId, startX: event.clientX, startY: event.clientY,
      lastX: event.clientX, initialScrollLeft: viewport.scrollLeft
    }
    pullController.current.begin({ scrollLeft: viewport.scrollLeft, hasParent: Boolean(parent) })
    event.currentTarget.setPointerCapture?.(event.pointerId)
  }
  const pointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const gesture = pointerGesture.current
    const viewport = viewportRef.current
    if (!gesture || !viewport || gesture.id !== event.pointerId) return
    const totalX = event.clientX - gesture.startX
    const totalY = event.clientY - gesture.startY
    if (Math.abs(totalX) < 3 && Math.abs(totalY) < 3) return
    const movement = pullController.current.move({
      deltaTowardParent: event.clientX - gesture.lastX,
      viewportWidth: viewport.clientWidth,
      verticalDominant: Math.abs(totalY) > Math.abs(totalX)
    })
    gesture.lastX = event.clientX
    if (movement.consume) {
      event.preventDefault()
      setPull({ distance: movement.pullDistance, progress: movement.progress, springBack: false })
      return
    }
    if (Math.abs(totalX) > Math.abs(totalY)) {
      event.preventDefault()
      viewport.scrollLeft = Math.max(0, gesture.initialScrollLeft - totalX)
      markScrolling()
    }
  }
  const pointerEnd = (event: PointerEvent<HTMLDivElement>) => {
    if (pointerGesture.current?.id !== event.pointerId) return
    pointerGesture.current = null
    event.currentTarget.releasePointerCapture?.(event.pointerId)
    finishPullGesture()
  }

  return <div className={`session-carousel-shell${pull.springBack ? ' is-springing' : ''}`}
    style={{ '--parent-pull-distance': `${pull.distance}px` } as React.CSSProperties}>
    {parent && pull.distance > 0 && <ParentProjection parent={parent}
      pullDistance={pull.distance} progress={pull.progress} />}
    <div className="session-carousel" ref={viewportRef} role="region" aria-label="同级会话列表"
      data-visible-columns={visibleCount} onScroll={markScrolling} onWheel={wheel}
      onPointerDown={pointerDown} onPointerMove={pointerMove}
      onPointerUp={pointerEnd} onPointerCancel={pointerEnd}
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
  </div>
}

function reducedMotion(): boolean {
  return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
}
