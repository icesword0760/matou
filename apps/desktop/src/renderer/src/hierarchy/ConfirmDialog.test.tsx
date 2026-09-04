// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ConfirmDialog } from './ConfirmDialog'

afterEach(cleanup)

describe('ConfirmDialog', () => {
  it('mounts a viewport confirmation at the document root', () => {
    const stackingContext = document.createElement('aside')
    document.body.append(stackingContext)

    render(<ConfirmDialog title="移出工作空间" body="确认范围" confirmLabel="移出"
      onConfirm={vi.fn()} onCancel={vi.fn()} />, { container: stackingContext })

    const dialog = screen.getByRole('alertdialog', { name: '移出工作空间' })
    expect(dialog.parentElement?.parentElement).toBe(document.body)
  })

  it('centers a session-scoped confirmation in the session area instead of a transformed card', () => {
    const hiddenStage = document.createElement('section')
    hiddenStage.className = 'scene-stage'
    hiddenStage.hidden = true
    const hiddenCanvas = document.createElement('main')
    hiddenCanvas.className = 'session-canvas'
    hiddenStage.append(hiddenCanvas)
    document.body.append(hiddenStage)

    const activeStage = document.createElement('section')
    activeStage.className = 'scene-stage'
    const sessionCanvas = document.createElement('main')
    sessionCanvas.className = 'session-canvas'
    const transformedCard = document.createElement('article')
    transformedCard.className = 'session-card'
    transformedCard.style.transform = 'translateX(240px)'
    sessionCanvas.append(transformedCard)
    activeStage.append(sessionCanvas)
    document.body.append(activeStage)

    render(<ConfirmDialog title="移出节点" body="确认范围" confirmLabel="移出"
      scope="session" onConfirm={vi.fn()} onCancel={vi.fn()} />, { container: transformedCard })

    const dialog = screen.getByRole('alertdialog', { name: '移出节点' })
    const overlay = dialog.parentElement
    expect(overlay?.parentElement).toBe(sessionCanvas)
    expect(overlay?.classList.contains('is-session-scoped')).toBe(true)
  })
})
