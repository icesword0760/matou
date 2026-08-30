import {
  useEffect, useLayoutEffect, useMemo, useRef, useState,
  type PointerEvent, type ReactNode
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
  geometryKey?: string
  initialScrollLeft?: number
  initialAnchor?: { sessionId: string; viewportOffset: number }
  revealRequest?: { sessionId: string; sequence: number; historical?: boolean }
  onGeometryChange?(
    geometry: {
      scrollLeft: number
      focusedSessionId?: string
      anchorSessionId?: string
      anchorViewportOffset?: number
    },
    options?: { continuous?: boolean }
  ): void
}) {
  const {
    nodes, focusedSessionId, renderSession, onActivate, onEnsureSessionVisible,
    parent, onCommitParent, geometryKey, initialScrollLeft = 0, initialAnchor,
    revealRequest, onGeometryChange
  } = props
  const sessionOrderKey = JSON.stringify(nodes.map((node) => node.sessionId))
  const viewportRef = useRef<HTMLDivElement>(null)
  const cardsRef = useRef(new Map<string, HTMLElement>())
  const previousOffsetsRef = useRef(new Map<string, number>())
  const ensureVisibleRef = useRef(onEnsureSessionVisible)
  const hoverRetargetFrame = useRef<number | undefined>(undefined)
  const hoverVisibilityFrame = useRef<number | undefined>(undefined)
  const hoverVisibilitySessionId = useRef<string | null>(null)
  const hoverVisibilityThrough = useRef(0)
  const focusVisibilityFrame = useRef<number | undefined>(undefined)
  const focusVisibilitySessionId = useRef<string | null>(null)
  const wheelTimer = useRef<number | undefined>(undefined)
  const hoverRestoreTimer = useRef<number | undefined>(undefined)
  const hoverBaselineScrollLeft = useRef<number | undefined>(undefined)
  const hoverIntentSessionId = useRef<string | null>(null)
  const hoverRef = useRef<(sessionId: string | null) => void>(() => undefined)
  const pointerPosition = useRef<{ x: number; y: number } | null>(null)
  const wheelGesture = useRef(false)
  const pageClosing = useRef(false)
  const pointerGesture = useRef<{
    id: number
    startX: number
    startY: number
    lastX: number
    initialScrollLeft: number
  } | null>(null)
  const pullController = useRef(new ParentPullController())
  const restoringGeometry = useRef(false)
  const skipFocusScrollAfterRestore = useRef(initialScrollLeft > 0 || initialAnchor !== undefined)
  const previousFocusedSessionId = useRef(focusedSessionId)
  const [firstVisible, setFirstVisible] = useState(0)
  const [hoveredSessionId, setHoveredSessionId] = useState<string | null>(null)
  const [pull, setPull] = useState({ distance: 0, progress: 0, springBack: false })
  const [visibleCount, setVisibleCount] = useState(() => visibleColumnsForWidth(nodes.length, 0))
  const [narrow, setNarrow] = useState(false)
  const inViewport = useMemo(() => new Set(
    nodes.slice(firstVisible, firstVisible + visibleCount).map(({ sessionId }) => sessionId)
  ), [firstVisible, nodes, visibleCount])
  const revealTargetPresent = Boolean(revealRequest && nodes.some(
    ({ sessionId }) => sessionId === revealRequest.sessionId
  ))
  const currentGeometry = (sessionId = focusedSessionId) => {
    const viewport = viewportRef.current
    const card = sessionId ? cardsRef.current.get(sessionId) : undefined
    return {
      scrollLeft: viewport?.scrollLeft ?? 0,
      ...(sessionId ? { focusedSessionId: sessionId } : {}),
      ...(sessionId && viewport && card ? {
        anchorSessionId: sessionId,
        anchorViewportOffset: card.offsetLeft - viewport.scrollLeft
      } : {})
    }
  }

  useEffect(() => {
    const closePage = () => { pageClosing.current = true }
    window.addEventListener('pagehide', closePage)
    window.addEventListener('beforeunload', closePage)
    return () => {
      window.removeEventListener('pagehide', closePage)
      window.removeEventListener('beforeunload', closePage)
      if (hoverRetargetFrame.current !== undefined) cancelAnimationFrame(hoverRetargetFrame.current)
      if (hoverVisibilityFrame.current !== undefined) cancelAnimationFrame(hoverVisibilityFrame.current)
      if (focusVisibilityFrame.current !== undefined) cancelAnimationFrame(focusVisibilityFrame.current)
      if (wheelTimer.current !== undefined) window.clearTimeout(wheelTimer.current)
      if (hoverRestoreTimer.current !== undefined) window.clearTimeout(hoverRestoreTimer.current)
    }
  }, [])
  useLayoutEffect(() => {
    const viewport = viewportRef.current
    if (!viewport) return
    const measure = () => {
      setVisibleCount(visibleColumnsForWidth(nodes.length, viewport.clientWidth))
      setNarrow(viewport.clientWidth > 0 && viewport.clientWidth < 720)
    }
    measure()
    if (typeof ResizeObserver === 'undefined') {
      window.addEventListener('resize', measure)
      return () => window.removeEventListener('resize', measure)
    }
    const observer = new ResizeObserver(measure)
    observer.observe(viewport)
    return () => observer.disconnect()
  }, [nodes.length])
  useEffect(() => { ensureVisibleRef.current = onEnsureSessionVisible }, [onEnsureSessionVisible])
  useLayoutEffect(() => {
    const viewport = viewportRef.current
    if (!viewport) return
    restoringGeometry.current = true
    let frame = 0
    let attempts = 0
    let reachedFrames = 0
    const restore = () => {
      const maxScrollLeft = Math.max(0, viewport.scrollWidth - viewport.clientWidth)
      const anchorCard = initialAnchor
        ? cardsRef.current.get(initialAnchor.sessionId)
        : undefined
      const requested = anchorCard
        ? anchoredCardScrollLeft(anchorCard.offsetLeft, initialAnchor!.viewportOffset, maxScrollLeft)
        : Math.max(0, initialScrollLeft)
      viewport.scrollLeft = requested
      updateVisibleWindow()
      return { requested, expected: Math.min(requested, maxScrollLeft) }
    }
    let target = restore()
    skipFocusScrollAfterRestore.current = initialScrollLeft > 0 || initialAnchor !== undefined
    if (target.requested === 0 && initialScrollLeft === 0 && initialAnchor === undefined) {
      restoringGeometry.current = false
      return
    }
    const continueRestore = () => {
      if (!restoringGeometry.current) return
      target = restore()
      attempts += 1
      const maxScrollLeft = Math.max(0, viewport.scrollWidth - viewport.clientWidth)
      target.expected = Math.min(target.requested, maxScrollLeft)
      reachedFrames = Math.abs(viewport.scrollLeft - target.expected) < 1
        ? reachedFrames + 1 : 0
      // Terminal surfaces and responsive columns settle over multiple frames.
      // Keep applying a persisted non-zero viewport until its full scroll range
      // exists instead of permanently accepting the first clamped value.
      if (target.requested === 0 || (attempts >= 15 && reachedFrames >= 3 && maxScrollLeft > 0) || attempts >= 90) {
        restoringGeometry.current = false
        if (Math.abs(target.requested - viewport.scrollLeft) >= 1) {
          onGeometryChange?.(currentGeometry())
        }
        return
      }
      frame = requestAnimationFrame(continueRestore)
    }
    frame = requestAnimationFrame(continueRestore)
    return () => {
      cancelAnimationFrame(frame)
    }
  }, [geometryKey, initialAnchor?.sessionId, initialAnchor?.viewportOffset, initialScrollLeft, visibleCount])

  useLayoutEffect(() => {
    const next = new Map<string, number>()
    const sessionIds = JSON.parse(sessionOrderKey) as string[]
    for (const sessionId of sessionIds) {
      const element = cardsRef.current.get(sessionId)
      if (!element) continue
      // Screen rects include both the horizontal viewport scroll and any FLIP
      // transform still in flight. Feeding either value into the next FLIP
      // produces exponentially growing translations during frequent Runtime
      // projections, leaving the card far away from the pointer. offsetLeft is
      // the stable position inside the carousel content and changes only when
      // the authoritative sibling order changes.
      element.getAnimations?.().forEach((animation) => animation.cancel())
      const offset = element.offsetLeft
      next.set(sessionId, offset)
      const previous = previousOffsetsRef.current.get(sessionId)
      const deltaX = previous === undefined ? 0 : previous - offset
      if (previous !== undefined && Math.abs(deltaX) > 0.5 && !reducedMotion()) {
        element.animate?.(
          [{ transform: `translateX(${deltaX}px)` }, { transform: 'translateX(0)' }],
          { duration: 180, easing: 'cubic-bezier(.2,.8,.2,1)' }
        )
      }
    }
    previousOffsetsRef.current = next
  }, [sessionOrderKey])

  useEffect(() => {
    if (!focusedSessionId) return
    const focusChanged = previousFocusedSessionId.current !== focusedSessionId
    previousFocusedSessionId.current = focusedSessionId
    if (focusChanged) {
      // Activation is a durable navigation choice; any pointer-only preview
      // and its old viewport baseline must stop governing the carousel.
      hoverIntentSessionId.current = null
      setHoveredSessionId(null)
      hoverVisibilitySessionId.current = null
      if (hoverVisibilityFrame.current !== undefined) cancelAnimationFrame(hoverVisibilityFrame.current)
      hoverVisibilityFrame.current = undefined
      if (hoverRestoreTimer.current !== undefined) window.clearTimeout(hoverRestoreTimer.current)
      hoverRestoreTimer.current = undefined
      hoverBaselineScrollLeft.current = undefined
    }
    if (skipFocusScrollAfterRestore.current) {
      skipFocusScrollAfterRestore.current = false
      if (!focusChanged) return
    }
    if (restoringGeometry.current) {
      if (!focusChanged) return
      // A user-selected Session supersedes a stale viewport restore. The
      // restoration loop observes this flag on its next frame and exits.
      restoringGeometry.current = false
    }
    focusVisibilitySessionId.current = focusedSessionId
    const followThrough = performance.now() + 440
    const followActiveCard = () => {
      focusVisibilityFrame.current = undefined
      if (focusVisibilitySessionId.current !== focusedSessionId) return
      if (hoverIntentSessionId.current !== null) {
        focusVisibilitySessionId.current = null
        return
      }
      const viewport = viewportRef.current
      const card = cardsRef.current.get(focusedSessionId)
      if (!viewport || !card) return
      const target = centeredCardScrollLeft(
        card.offsetLeft,
        card.offsetWidth,
        viewport.clientWidth,
        Math.max(0, viewport.scrollWidth - viewport.clientWidth)
      )
      const delta = target - viewport.scrollLeft
      if (Math.abs(delta) > 0.5) {
        viewport.scrollLeft += Math.sign(delta) * Math.min(Math.abs(delta), 28)
        updateVisibleWindow()
      }
      ensureVisibleRef.current?.(focusedSessionId)
      if (performance.now() < followThrough || Math.abs(delta) > 0.5) {
        focusVisibilityFrame.current = requestAnimationFrame(followActiveCard)
        return
      }
      focusVisibilitySessionId.current = null
      if (!pageClosing.current && !restoringGeometry.current) {
        onGeometryChange?.(currentGeometry())
      }
    }
    focusVisibilityFrame.current = requestAnimationFrame(followActiveCard)
    return () => {
      if (focusVisibilityFrame.current !== undefined) cancelAnimationFrame(focusVisibilityFrame.current)
      focusVisibilityFrame.current = undefined
      if (focusVisibilitySessionId.current === focusedSessionId) focusVisibilitySessionId.current = null
    }
  }, [focusedSessionId])

  useEffect(() => {
    if (!parent) return
    const captureTerminalTab = (event: KeyboardEvent) => {
      if (event.key !== 'Tab' || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
      const target = event.target as HTMLElement | null
      if (!target?.closest('.terminal-surface')) return
      const returnButton = viewportRef.current?.closest('.session-canvas')
        ?.querySelector<HTMLButtonElement>('.session-return-parent')
      if (!returnButton) return
      event.preventDefault()
      event.stopPropagation()
      returnButton.focus()
    }
    window.addEventListener('keydown', captureTerminalTab, true)
    return () => window.removeEventListener('keydown', captureTerminalTab, true)
  }, [parent])

  useLayoutEffect(() => {
    if (!revealRequest) return
    restoringGeometry.current = false
    skipFocusScrollAfterRestore.current = false
    const frame = requestAnimationFrame(() => {
      const card = cardsRef.current.get(revealRequest.sessionId)
      const viewport = viewportRef.current
      if (!card || !viewport) return
      // DAG and notification navigation may select the already-focused Session.
      // In that case React has no focus-ID change to observe, so force the
      // carousel position from this explicit navigation request. Directly
      // setting the owning viewport also avoids Chromium scrolling the page
      // instead of the horizontal strip while the native DAG window closes.
      centerCardInViewport(viewport, card)
      const focusTarget = revealRequest.historical
        ? card.querySelector<HTMLElement>('button,[tabindex="0"]')
        : undefined
      focusTarget?.focus({ preventScroll: true })
      ensureVisibleRef.current?.(revealRequest.sessionId)
      window.setTimeout(() => {
        updateVisibleWindow()
        onGeometryChange?.(currentGeometry(revealRequest.sessionId))
      }, reducedMotion() ? 1 : 320)
    })
    return () => cancelAnimationFrame(frame)
    // The sequence is an explicit product navigation request. It must override
    // persisted geometry even when the target Session ID did not change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revealRequest?.sequence, revealTargetPresent])

  const updateVisibleWindow = () => {
    const viewport = viewportRef.current
    if (!viewport || nodes.length <= visibleCount) {
      setFirstVisible(0)
      return
    }
    const unit = viewport.clientWidth > 0 ? viewport.clientWidth / visibleCount : 1
    setFirstVisible(Math.max(0, Math.min(nodes.length - visibleCount, Math.round(viewport.scrollLeft / unit))))
  }
  const restoreHoverBaseline = (settled: boolean) => {
    const viewport = viewportRef.current
    const baseline = hoverBaselineScrollLeft.current
    if (!viewport || baseline === undefined) return false
    viewport.scrollLeft = baseline
    updateVisibleWindow()
    if (settled) {
      if (hoverRestoreTimer.current !== undefined) window.clearTimeout(hoverRestoreTimer.current)
      hoverRestoreTimer.current = undefined
      hoverBaselineScrollLeft.current = undefined
    }
    return true
  }
  const resumeHoverAtPointer = () => {
    const viewport = viewportRef.current
    const pointer = pointerPosition.current
    if (!viewport || !pointer || typeof document.elementFromPoint !== 'function') return
    const hit = document.elementFromPoint(pointer.x, pointer.y) as HTMLElement | null
    const card = hit?.closest<HTMLElement>('[data-session-card]')
    if (!card || !viewport.contains(card)) return
    const sessionId = card.dataset.sessionCard
    if (sessionId) hoverRef.current(sessionId)
  }
  const retargetHoverOnNextFrame = () => {
    if (hoverRetargetFrame.current !== undefined) cancelAnimationFrame(hoverRetargetFrame.current)
    hoverRetargetFrame.current = requestAnimationFrame(() => {
      hoverRetargetFrame.current = undefined
      resumeHoverAtPointer()
    })
  }
  const keepHoveredCardFullyVisible = (sessionId: string) => {
    hoverVisibilitySessionId.current = sessionId
    hoverVisibilityThrough.current = performance.now() + 440
    if (hoverVisibilityFrame.current !== undefined) {
      cancelAnimationFrame(hoverVisibilityFrame.current)
      hoverVisibilityFrame.current = undefined
    }
    const followPreview = () => {
      hoverVisibilityFrame.current = undefined
      if (hoverVisibilitySessionId.current !== sessionId) return
      const viewport = viewportRef.current
      const card = cardsRef.current.get(sessionId)
      if (!viewport || !card) return
      const target = fullyVisibleCardScrollLeft(
        card.offsetLeft,
        card.offsetWidth,
        viewport.scrollLeft,
        viewport.clientWidth,
        Math.max(0, viewport.scrollWidth - viewport.clientWidth)
      )
      const delta = target - viewport.scrollLeft
      if (Math.abs(delta) > 0.5) {
        // Follow the growing edge instead of jumping or launching overlapping
        // native smooth-scroll animations. The 28px frame cap keeps large edge
        // corrections physical while ordinary expansion advances by only a
        // few pixels per frame.
        viewport.scrollLeft += Math.sign(delta) * Math.min(Math.abs(delta), 28)
        updateVisibleWindow()
      }
      if (performance.now() < hoverVisibilityThrough.current || Math.abs(delta) > 0.5) {
        hoverVisibilityFrame.current = requestAnimationFrame(followPreview)
      }
    }
    hoverVisibilityFrame.current = requestAnimationFrame(followPreview)
  }
  const markScrolling = (userInitiated = false) => {
    // A wheel or drag may arrive before the two geometry-restoration frames
    // finish after a Session was added. User input is authoritative from that
    // point onward and must not be mistaken for a programmatic restore event.
    if (userInitiated) {
      restoringGeometry.current = false
      focusVisibilitySessionId.current = null
      if (focusVisibilityFrame.current !== undefined) cancelAnimationFrame(focusVisibilityFrame.current)
      focusVisibilityFrame.current = undefined
      // Direct trackpad/mouse movement owns the viewport. A hover preview may
      // still hand its expanded width to the card now under the stationary
      // pointer, but its edge-follow animation must not pull the strip back
      // toward the card the user is deliberately scrolling away from.
      hoverVisibilitySessionId.current = null
      if (hoverVisibilityFrame.current !== undefined) cancelAnimationFrame(hoverVisibilityFrame.current)
      hoverVisibilityFrame.current = undefined
      if (hoverRestoreTimer.current !== undefined) window.clearTimeout(hoverRestoreTimer.current)
      hoverRestoreTimer.current = undefined
      hoverBaselineScrollLeft.current = undefined
      hoverIntentSessionId.current = null
      // Horizontal movement changes the hit target below a stationary pointer.
      // Resolve it on the next painted frame instead of waiting for macOS
      // momentum events to end; repeated wheel events coalesce into one hit
      // test per frame and hand expansion directly from one card to the next.
      retargetHoverOnNextFrame()
    }
    updateVisibleWindow()
    // Browser layout changes can clamp scrollLeft after the original wheel or
    // drag stream has ended (for example while terminal columns settle). That
    // native scroll is still the final user-visible viewport and must replace
    // the earlier, now-unreachable checkpoint. Only explicit restore writes
    // are excluded to avoid feeding persisted geometry back into itself.
    if (!pageClosing.current && !restoringGeometry.current && viewportRef.current &&
      hoverBaselineScrollLeft.current === undefined && focusVisibilitySessionId.current === null) {
      onGeometryChange?.(currentGeometry(), { continuous: true })
    }
  }
  const hover = (sessionId: string | null) => {
    hoverIntentSessionId.current = sessionId
    if (sessionId === null) {
      hoverVisibilitySessionId.current = null
      if (hoverVisibilityFrame.current !== undefined) {
        cancelAnimationFrame(hoverVisibilityFrame.current)
        hoverVisibilityFrame.current = undefined
      }
      setHoveredSessionId(null)
      // A card boundary is not the end of the hover preview: the pointer can
      // spend any number of frames inside the carousel gap. Only leaving the
      // entire carousel reaches this branch, so there is no timing window in
      // which Chromium can collapse and re-expand the strip while handing off.
      restoreHoverBaseline(false)
      if (hoverRestoreTimer.current !== undefined) window.clearTimeout(hoverRestoreTimer.current)
      hoverRestoreTimer.current = window.setTimeout(() => restoreHoverBaseline(true), 220)
      return
    }
    if (hoverRestoreTimer.current !== undefined) window.clearTimeout(hoverRestoreTimer.current)
    hoverRestoreTimer.current = undefined
    focusVisibilitySessionId.current = null
    if (focusVisibilityFrame.current !== undefined) cancelAnimationFrame(focusVisibilityFrame.current)
    focusVisibilityFrame.current = undefined
    if (hoverBaselineScrollLeft.current === undefined) {
      hoverBaselineScrollLeft.current = viewportRef.current?.scrollLeft ?? 0
    }
    setHoveredSessionId(sessionId)
    // During a live horizontal gesture, retargeting is visual only. Starting
    // an automatic visibility correction here would compete with the next
    // wheel delta and can leave the user stuck before the far edge.
    if (!wheelGesture.current) keepHoveredCardFullyVisible(sessionId)
  }
  hoverRef.current = hover
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
    }, 240)
  }
  const wheel = (event: WheelEvent) => {
    if (event.ctrlKey || event.metaKey) return
    const viewport = viewportRef.current
    if (!viewport) return
    const overTerminal = (event.target as HTMLElement).closest('.terminal-surface') !== null
    // macOS trackpads dispatch the wheel event from inside xterm. Capture it
    // before xterm consumes horizontal movement for its own viewport. A mouse
    // user can express the same intent with Shift + wheel.
    const horizontal = event.shiftKey || Math.abs(event.deltaX) >= Math.abs(event.deltaY)
    if (!horizontal && overTerminal) return
    const delta = horizontal
      ? (Math.abs(event.deltaX) > 0 ? event.deltaX : event.deltaY)
      : event.deltaY
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
    markScrolling(true)
    scheduleWheelEnd()
  }
  useEffect(() => {
    const viewport = viewportRef.current
    if (!viewport) return
    // React registers wheel handlers passively in Chromium. This interaction
    // must cancel the browser's native scroll because Matou applies the same
    // delta itself and reserves edge movement for the parent-pull gesture.
    viewport.addEventListener('wheel', wheel, { passive: false, capture: true })
    return () => viewport.removeEventListener('wheel', wheel, { capture: true })
  })

  const pointerDown = (event: PointerEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement
    if (event.button !== 0 || target.closest(
      '.terminal-surface,button,input,textarea,select,a,[role="menuitem"]'
    )) return
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
    pointerPosition.current = { x: event.clientX, y: event.clientY }
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
      markScrolling(true)
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
    <div className={`session-carousel${nodes.length > visibleCount ? ' has-overflow' : ''}${narrow ? ' is-narrow' : ''}`}
      ref={viewportRef} role="region" aria-label="同级会话列表"
      data-visible-columns={visibleCount} onScroll={() => markScrolling()}
      onPointerDown={pointerDown} onPointerMove={pointerMove}
      onPointerUp={pointerEnd} onPointerCancel={pointerEnd}
      onPointerEnter={(event) => {
        pointerPosition.current = { x: event.clientX, y: event.clientY }
      }}
      onPointerLeave={() => {
        pointerPosition.current = null
        hover(null)
      }}
      style={{ '--session-visible-columns': visibleCount } as React.CSSProperties}>
      {nodes.map((node) => <div key={node.sessionId} ref={(element) => {
        if (element) cardsRef.current.set(node.sessionId, element)
        else cardsRef.current.delete(node.sessionId)
      }} data-session-id={node.sessionId}
      className={`session-card-slot${node.sessionId === focusedSessionId ? ' is-focused' : ''}${node.sessionId === focusedSessionId || hoveredSessionId === node.sessionId ? ' is-expanded' : ''}`}
      onTransitionEnd={(event) => {
        if (event.target !== event.currentTarget ||
          (event.propertyName !== 'flex-basis' && event.propertyName !== 'flex-grow')) return
        if (hoverBaselineScrollLeft.current !== undefined) {
          if (hoverIntentSessionId.current === null && hoveredSessionId === null) restoreHoverBaseline(true)
          else updateVisibleWindow()
          return
        }
        updateVisibleWindow()
        if (!pageClosing.current && !restoringGeometry.current) {
          onGeometryChange?.(currentGeometry(), { continuous: true })
        }
      }}>
        <SessionCard node={node} focused={node.sessionId === focusedSessionId}
          inViewport={inViewport.has(node.sessionId)}
          expanded={node.sessionId === focusedSessionId || hoveredSessionId === node.sessionId}
          onActivate={(sessionId) => {
            // Focusing or typing into the previewed card turns the current
            // position into explicit user intent. Do not later restore the
            // pre-hover viewport when the pointer leaves.
            if (hoverRestoreTimer.current !== undefined) window.clearTimeout(hoverRestoreTimer.current)
            hoverRestoreTimer.current = undefined
            hoverBaselineScrollLeft.current = undefined
            onGeometryChange?.(currentGeometry(sessionId))
            onActivate(sessionId)
          }} onHover={hover}>
          {renderSession(node, inViewport.has(node.sessionId))}
          {narrow && <div className="session-compact-summary" aria-hidden={node.sessionId === focusedSessionId}>
            <strong>{node.title}</strong>
            <span className={`status-${node.workStatus}`}>{compactStatus(node.workStatus)}</span>
            {(node.providerRestoreState === 'failed' || node.activeChildCount > 0) &&
              <div className="session-compact-summary__priority">
                {node.providerRestoreState === 'failed' && <b>Claude 恢复失败</b>}
                {node.activeChildCount > 0 && <small>子会话 {node.activeChildCount}</small>}
              </div>}
            <pre title={node.cwd}>{node.latestLines.slice(-3).join('\n') || node.cwd}</pre>
          </div>}
        </SessionCard>
      </div>)}
    </div>
  </div>
}

export function centeredCardScrollLeft(
  cardOffsetLeft: number,
  cardWidth: number,
  viewportWidth: number,
  maxScrollLeft: number
): number {
  return Math.max(0, Math.min(
    maxScrollLeft,
    cardOffsetLeft - Math.max(0, viewportWidth - cardWidth) / 2
  ))
}

export function anchoredCardScrollLeft(
  cardOffsetLeft: number,
  viewportOffset: number,
  maxScrollLeft: number
): number {
  return Math.max(0, Math.min(maxScrollLeft, cardOffsetLeft - viewportOffset))
}

export function fullyVisibleCardScrollLeft(
  cardOffsetLeft: number,
  cardWidth: number,
  viewportScrollLeft: number,
  viewportWidth: number,
  maxScrollLeft: number,
  edgeInset = 10
): number {
  const visibleLeft = viewportScrollLeft + edgeInset
  const visibleRight = viewportScrollLeft + viewportWidth - edgeInset
  if (cardOffsetLeft < visibleLeft) {
    return Math.max(0, Math.min(maxScrollLeft, cardOffsetLeft - edgeInset))
  }
  const cardRight = cardOffsetLeft + cardWidth
  if (cardRight > visibleRight) {
    return Math.max(0, Math.min(maxScrollLeft, cardRight - viewportWidth + edgeInset))
  }
  return viewportScrollLeft
}

function centerCardInViewport(viewport: HTMLElement, card: HTMLElement): void {
  const target = centeredCardScrollLeft(
    card.offsetLeft,
    card.offsetWidth,
    viewport.clientWidth,
    Math.max(0, viewport.scrollWidth - viewport.clientWidth)
  )
  viewport.scrollLeft = target
  // Reapply after responsive card widths and terminal fits settle in the same
  // navigation turn. The second write is idempotent for already-stable layouts.
  requestAnimationFrame(() => { viewport.scrollLeft = target })
}

function reducedMotion(): boolean {
  return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
}

export function visibleColumnsForWidth(nodeCount: number, width: number): number {
  const available = width > 0 ? Math.floor((width + 12) / (280 + 12)) : 4
  return Math.min(4, Math.max(1, nodeCount, 1), Math.max(1, available))
}

function compactStatus(status: SessionGraphNodeView['workStatus']): string {
  if (status === 'needs-input') return '等待输入'
  if (status === 'running' || status === 'starting') return '运行中'
  if (status === 'error') return '异常'
  if (status === 'interrupted') return '中断'
  if (status === 'exited') return '历史'
  return '空闲'
}
