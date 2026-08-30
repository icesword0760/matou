// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import {
  anchoredCardScrollLeft,
  centeredCardScrollLeft,
  SessionCarousel,
  visibleColumnsForWidth
} from './SessionCarousel'

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn()
})
afterEach(cleanup)

describe('SessionCarousel', () => {
  it('centers an explicit navigation target inside the horizontal viewport bounds', () => {
    expect(centeredCardScrollLeft(0, 280, 900, 900)).toBe(0)
    expect(centeredCardScrollLeft(900, 280, 900, 900)).toBe(590)
    expect(centeredCardScrollLeft(1_800, 280, 900, 900)).toBe(900)
  })

  it('restores a stable card viewport offset when responsive geometry changed', () => {
    expect(anchoredCardScrollLeft(992, 60, 1_280)).toBe(932)
    expect(anchoredCardScrollLeft(992, 60, 800)).toBe(800)
    expect(anchoredCardScrollLeft(30, 60, 1_280)).toBe(0)
  })

  it('restores the focused card to its persisted viewport position after layout settles', () => {
    vi.useFakeTimers()
    render(<SessionCarousel nodes={fixtures(5)} focusedSessionId="session-3"
      initialScrollLeft={563} initialAnchor={{ sessionId: 'session-3', viewportOffset: 60 }}
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' }) as HTMLDivElement
    const focusedSlot = document.querySelector<HTMLElement>('[data-session-id="session-3"]')!
    let position = 0
    Object.defineProperties(viewport, {
      clientWidth: { configurable: true, value: 720 },
      scrollWidth: { configurable: true, value: 2_000 },
      scrollLeft: {
        configurable: true,
        get: () => position,
        set: (value: number) => { position = Math.max(0, Math.min(1_280, value)) }
      }
    })
    Object.defineProperty(focusedSlot, 'offsetLeft', { configurable: true, value: 992 })

    act(() => vi.advanceTimersByTime(600))

    expect(viewport.scrollLeft).toBe(932)
    vi.useRealTimers()
  })

  it('reduces visible columns before terminal cards become unreadable in a narrow window', () => {
    expect(visibleColumnsForWidth(7, 1440)).toBe(4)
    expect(visibleColumnsForWidth(7, 900)).toBe(3)
    expect(visibleColumnsForWidth(7, 700)).toBe(2)
    expect(visibleColumnsForWidth(7, 420)).toBe(1)
  })

  it('shows at most four cards in the viewport while retaining every stable Session card', () => {
    const nodes = fixtures(7)
    render(<SessionCarousel nodes={nodes} focusedSessionId="session-1"
      onActivate={() => undefined} renderSession={(node, visible) =>
        <div data-testid={`content-${node.sessionId}`} data-visible={visible} />} />)

    expect(screen.getAllByLabelText(/^会话：/)).toHaveLength(7)
    expect(document.querySelectorAll('[data-in-viewport="true"]')).toHaveLength(4)
    expect(screen.getByRole('region', { name: '同级会话列表' }).getAttribute('data-visible-columns')).toBe('4')
  })

  it('presents every Session as a complete card with status and position context', () => {
    const nodes = fixtures(3)
    nodes[1] = { ...nodes[1]!, workStatus: 'needs-input' }
    render(<SessionCarousel nodes={nodes} focusedSessionId="session-2"
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)

    const focusedCard = document.querySelector('[data-session-card="session-2"]')!
    const footer = focusedCard.querySelector('[data-session-card-footer]')
    expect(footer).not.toBeNull()
    expect(footer?.textContent).toContain('待输入')
    expect(footer?.textContent).toContain('2/3')
    expect(footer?.querySelector('[data-session-status-dot]')).not.toBeNull()
  })

  it('keeps card DOM identity while authoritative interaction order changes', () => {
    const nodes = fixtures(3)
    const view = render(<SessionCarousel nodes={nodes} focusedSessionId="session-2"
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)
    const original = document.querySelector('[data-session-card="session-2"]')

    view.rerender(<SessionCarousel nodes={[nodes[1]!, nodes[0]!, nodes[2]!]} focusedSessionId="session-2"
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)

    expect(document.querySelector('[data-session-card="session-2"]')).toBe(original)
    expect([...document.querySelectorAll('[data-session-card]')].map((node) => node.getAttribute('data-session-card')))
      .toEqual(['session-2', 'session-1', 'session-3'])
  })

  it('animates reordering from stable content offsets instead of transient transformed screen rects', () => {
    const positions = new Map([['session-1', 0], ['session-2', 300]])
    const offsetLeft = vi.spyOn(HTMLElement.prototype, 'offsetLeft', 'get').mockImplementation(function (this: HTMLElement) {
      return positions.get(this.getAttribute('data-session-id') ?? '') ?? 0
    })
    const animate = vi.fn()
    const originalAnimate = HTMLElement.prototype.animate
    HTMLElement.prototype.animate = animate
    const nodes = fixtures(2)
    const view = render(<SessionCarousel nodes={nodes} focusedSessionId="session-1"
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)

    positions.set('session-1', 300)
    positions.set('session-2', 0)
    view.rerender(<SessionCarousel nodes={[nodes[1]!, nodes[0]!]} focusedSessionId="session-1"
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)

    expect(animate).toHaveBeenCalledWith(
      [{ transform: 'translateX(300px)' }, { transform: 'translateX(0)' }],
      { duration: 180, easing: 'cubic-bezier(.2,.8,.2,1)' }
    )
    expect(animate).toHaveBeenCalledWith(
      [{ transform: 'translateX(-300px)' }, { transform: 'translateX(0)' }],
      { duration: 180, easing: 'cubic-bezier(.2,.8,.2,1)' }
    )

    offsetLeft.mockRestore()
    HTMLElement.prototype.animate = originalAnimate
  })

  it('centers the focused Session and expands hover only while ordinary scrolling is idle', () => {
    vi.useFakeTimers()
    const nodes = fixtures(5)
    render(<SessionCarousel nodes={nodes} focusedSessionId="session-5"
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)
    const card = document.querySelector('[data-session-card="session-5"]') as HTMLElement
    const viewport = screen.getByRole('region', { name: '同级会话列表' }) as HTMLElement
    Object.defineProperties(card.parentElement!, {
      offsetLeft: { configurable: true, value: 900 },
      offsetWidth: { configurable: true, value: 280 }
    })
    Object.defineProperties(viewport, {
      clientWidth: { configurable: true, value: 900 },
      scrollWidth: { configurable: true, value: 1_800 }
    })
    vi.runAllTimers()
    expect(viewport.scrollLeft).toBe(590)

    fireEvent.mouseEnter(card)
    act(() => vi.advanceTimersByTime(160))
    expect(card.classList.contains('is-expanded')).toBe(true)
    fireEvent.wheel(screen.getByRole('region', { name: '同级会话列表' }), { deltaX: 20, deltaY: 0 })
    expect(card.classList.contains('is-expanded')).toBe(false)
    act(() => vi.advanceTimersByTime(120))
    expect(card.classList.contains('is-expanded')).toBe(false)
    fireEvent.mouseEnter(card)
    act(() => vi.advanceTimersByTime(160))
    expect(card.classList.contains('is-expanded')).toBe(true)
    vi.useRealTimers()
  })

  it('expands from a stable pointer entry even when the pointer moves inside the same card', () => {
    vi.useFakeTimers()
    render(<SessionCarousel nodes={fixtures(5)} focusedSessionId="session-1"
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)
    const card = document.querySelector('[data-session-card="session-3"]')!

    fireEvent.mouseEnter(card)
    act(() => vi.advanceTimersByTime(80))
    fireEvent.pointerMove(card, { clientX: 100, clientY: 50 })
    act(() => vi.advanceTimersByTime(80))

    expect(card.classList.contains('is-expanded')).toBe(true)
    vi.useRealTimers()
  })

  it('keeps hover expansion through the native scroll emitted by its width change', () => {
    vi.useFakeTimers()
    render(<SessionCarousel nodes={fixtures(5)} focusedSessionId="session-1"
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)
    const card = document.querySelector('[data-session-card="session-3"]')!
    const viewport = screen.getByRole('region', { name: '同级会话列表' })

    act(() => vi.advanceTimersByTime(20))
    fireEvent.mouseEnter(card)
    act(() => vi.advanceTimersByTime(160))
    expect(card.classList.contains('is-expanded')).toBe(true)

    // Chromium reports a native scroll when flex-basis expansion changes the
    // available horizontal range. That layout notification is not a new user
    // gesture and must not collapse the card under the pointer.
    fireEvent.scroll(viewport)
    expect(card.classList.contains('is-expanded')).toBe(true)

    vi.useRealTimers()
  })

  it('checkpoints the focused card anchor after a hover width transition settles', () => {
    vi.useFakeTimers()
    const onGeometryChange = vi.fn()
    render(<SessionCarousel nodes={fixtures(5)} focusedSessionId="session-3"
      onGeometryChange={onGeometryChange} onActivate={() => undefined}
      renderSession={(node) => <span>{node.title}</span>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' }) as HTMLDivElement
    const focusedSlot = document.querySelector<HTMLElement>('[data-session-id="session-3"]')!
    Object.defineProperties(viewport, {
      scrollLeft: { configurable: true, value: 320, writable: true },
      clientWidth: { configurable: true, value: 720 },
      scrollWidth: { configurable: true, value: 2_000 }
    })
    Object.defineProperty(focusedSlot, 'offsetLeft', { configurable: true, value: 742 })
    act(() => vi.advanceTimersByTime(600))
    viewport.scrollLeft = 320
    onGeometryChange.mockClear()

    fireEvent.transitionEnd(focusedSlot, { propertyName: 'flex-basis' })

    expect(onGeometryChange).toHaveBeenCalledWith(expect.objectContaining({
      scrollLeft: 320,
      focusedSessionId: 'session-3',
      anchorSessionId: 'session-3',
      anchorViewportOffset: 422
    }), { continuous: true })
    vi.useRealTimers()
  })

  it('keeps a focused terminal beside compact sibling summaries in a narrow window', () => {
    const nodes = fixtures(4)
    nodes[1] = {
      ...nodes[1]!, providerRestoreState: 'failed', providerRestoreError: 'missing provider',
      workStatus: 'error', activeChildCount: 3
    }
    render(<SessionCarousel nodes={nodes} focusedSessionId="session-1"
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' })
    Object.defineProperty(viewport, 'clientWidth', { configurable: true, value: 600 })

    fireEvent(window, new Event('resize'))

    expect(viewport.classList.contains('is-narrow')).toBe(true)
    expect(document.querySelectorAll('.session-card')).toHaveLength(4)
    const sibling = document.querySelector('[data-session-id="session-2"] .session-compact-summary')!
    expect(sibling.closest('.session-card')).toBeTruthy()
    expect(sibling.getAttribute('aria-hidden')).toBe('false')
    expect(sibling.textContent).toContain('恢复失败')
    expect(sibling.textContent).toContain('子会话 3')
  })

  it('finishes restoring at the browser-clamped reachable position', () => {
    vi.useFakeTimers()
    const onGeometryChange = vi.fn()
    render(<SessionCarousel nodes={fixtures(5)} focusedSessionId="session-3"
      initialScrollLeft={320} onGeometryChange={onGeometryChange}
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' }) as HTMLDivElement
    let position = 0
    Object.defineProperties(viewport, {
      clientWidth: { configurable: true, value: 800 },
      scrollWidth: { configurable: true, value: 908 },
      scrollLeft: {
        configurable: true,
        get: () => position,
        set: (value: number) => { position = Math.max(0, Math.min(108, value)) }
      }
    })

    act(() => vi.advanceTimersByTime(600))

    expect(viewport.scrollLeft).toBe(108)
    expect(onGeometryChange).toHaveBeenCalledWith(expect.objectContaining({
      scrollLeft: 108,
      focusedSessionId: 'session-3'
    }))
    vi.useRealTimers()
  })

  it('does not turn header actions or portal menu items into carousel drag gestures', () => {
    render(<SessionCarousel nodes={fixtures(1)} focusedSessionId="session-1"
      onActivate={() => undefined}
      renderSession={() => <button type="button" role="menuitem">⑂ Fork 会话</button>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' }) as HTMLDivElement
    viewport.setPointerCapture = vi.fn()

    fireEvent.pointerDown(screen.getByRole('menuitem', { name: '⑂ Fork 会话' }), {
      button: 0, pointerId: 7, clientX: 30, clientY: 30
    })

    expect(viewport.setPointerCapture).not.toHaveBeenCalled()
  })

  it('separates an oversized list scroll from a fresh edge pull that returns to the parent', () => {
    vi.useFakeTimers()
    const onCommitParent = vi.fn()
    render(<SessionCarousel nodes={fixtures(5)} focusedSessionId="session-3"
      parent={{ ...fixtures(1)[0]!, sessionId: 'parent', title: '父会话' }}
      onCommitParent={onCommitParent} onActivate={() => undefined}
      renderSession={(node) => <span>{node.title}</span>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' }) as HTMLDivElement
    Object.defineProperty(viewport, 'clientWidth', { configurable: true, value: 800 })
    viewport.scrollLeft = 320

    fireEvent.wheel(viewport, { deltaX: -600, deltaY: 0 })
    expect(viewport.scrollLeft).toBe(0)
    expect(screen.queryByTestId('parent-projection')).toBeNull()
    vi.advanceTimersByTime(240)
    expect(onCommitParent).not.toHaveBeenCalled()

    fireEvent.wheel(viewport, { deltaX: -500, deltaY: 0 })
    expect(screen.getByTestId('parent-projection').getAttribute('data-ready')).toBe('true')
    vi.advanceTimersByTime(240)
    expect(onCommitParent).toHaveBeenCalledWith('parent')
    vi.useRealTimers()
  })

  it('persists an immediate user scroll even while initial geometry frames are settling', () => {
    vi.useFakeTimers()
    const onGeometryChange = vi.fn()
    render(<SessionCarousel nodes={fixtures(5)} focusedSessionId="session-1"
      initialScrollLeft={120} onGeometryChange={onGeometryChange}
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' }) as HTMLDivElement
    Object.defineProperty(viewport, 'clientWidth', { configurable: true, value: 800 })
    viewport.scrollLeft = 120

    fireEvent.wheel(viewport, { deltaX: 200, deltaY: 0 })

    expect(viewport.scrollLeft).toBe(320)
    expect(onGeometryChange).toHaveBeenCalledWith(expect.objectContaining({
      scrollLeft: 320,
      focusedSessionId: 'session-1'
    }), { continuous: true })
    vi.useRealTimers()
  })

  it('checkpoints the final browser-clamped position after card widths settle', () => {
    vi.useFakeTimers()
    const onGeometryChange = vi.fn()
    render(<SessionCarousel nodes={fixtures(5)} focusedSessionId="session-5"
      onGeometryChange={onGeometryChange}
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' }) as HTMLDivElement
    Object.defineProperty(viewport, 'clientWidth', { configurable: true, value: 800 })
    act(() => vi.advanceTimersByTime(1_000))
    onGeometryChange.mockClear()

    // Chromium emits a native scroll after a shrinking card layout clamps a
    // previously valid larger scrollLeft. This final visible position is the
    // one the user expects to return to after restart.
    viewport.scrollLeft = 240
    fireEvent.scroll(viewport)

    expect(onGeometryChange).toHaveBeenCalledWith(expect.objectContaining({
      scrollLeft: 240,
      focusedSessionId: 'session-5'
    }), { continuous: true })
    vi.useRealTimers()
  })
})

function fixtures(count: number): SessionGraphNodeView[] {
  return Array.from({ length: count }, (_, index) => ({
    sessionId: `session-${index + 1}`, sceneId: 'scene-1', currentMode: 'shell',
    workStatus: 'idle', providerRestoreState: 'none', canFork: false,
    title: `Shell ${index + 1}`, cwd: '/tmp', activeChildCount: 0,
    historicalChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
    latestLines: [], lastUserInteractionSeq: 0
  }))
}
