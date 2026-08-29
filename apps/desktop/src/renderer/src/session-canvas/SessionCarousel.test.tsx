// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { SessionCarousel, visibleColumnsForWidth } from './SessionCarousel'

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn()
})
afterEach(cleanup)

describe('SessionCarousel', () => {
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

  it('centers the focused Session and expands hover only while ordinary scrolling is idle', () => {
    vi.useFakeTimers()
    const nodes = fixtures(5)
    render(<SessionCarousel nodes={nodes} focusedSessionId="session-5"
      onActivate={() => undefined} renderSession={(node) => <span>{node.title}</span>} />)
    const card = document.querySelector('[data-session-card="session-5"]')!
    vi.runAllTimers()
    expect(Element.prototype.scrollIntoView).toHaveBeenCalled()

    fireEvent.pointerMove(card)
    act(() => vi.advanceTimersByTime(160))
    expect(card.classList.contains('is-expanded')).toBe(true)
    fireEvent.scroll(screen.getByRole('region', { name: '同级会话列表' }))
    expect(card.classList.contains('is-expanded')).toBe(false)
    act(() => vi.advanceTimersByTime(120))
    fireEvent.pointerMove(card)
    act(() => vi.advanceTimersByTime(160))
    expect(card.classList.contains('is-expanded')).toBe(true)
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
