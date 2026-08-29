// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { SessionCarousel } from './SessionCarousel'

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn()
})
afterEach(cleanup)

describe('SessionCarousel', () => {
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

    fireEvent.pointerEnter(card)
    expect(card.classList.contains('is-expanded')).toBe(true)
    fireEvent.scroll(screen.getByRole('region', { name: '同级会话列表' }))
    expect(card.classList.contains('is-expanded')).toBe(false)
    vi.advanceTimersByTime(120)
    fireEvent.pointerEnter(card)
    expect(card.classList.contains('is-expanded')).toBe(true)
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
