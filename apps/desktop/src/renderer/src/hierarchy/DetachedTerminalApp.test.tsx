// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { DetachedTerminalApp } from './DetachedTerminalApp'

vi.mock('../terminal/TerminalSurface', () => ({
  TerminalSurface: ({ sessionId, themeKey, fontSize }: {
    sessionId: string; themeKey?: string; fontSize?: number
  }) => <div data-testid={`terminal-${sessionId}`} data-theme={themeKey} data-font-size={fontSize} />
}))
const runtime = vi.hoisted(() => ({
  request: vi.fn(async (method: string) => method === 'projection.snapshot' ? { hierarchy: {} } : {}),
  subscribeProjection: vi.fn(() => () => {})
}))
vi.mock('../runtime/RuntimeProvider', () => ({ useRuntimeClient: () => runtime }))

afterEach(() => {
  cleanup()
  runtime.request.mockClear()
  runtime.subscribeProjection.mockClear()
  window.history.replaceState({}, '', '/')
})

describe('PRD 02 detached HUD', () => {
  it('uses the same default white skin and Kooky theme shortcut as the main terminal', () => {
    Object.defineProperty(navigator, 'platform', { configurable: true, value: 'MacIntel' })
    window.history.replaceState({}, '', '/?kind=detached-terminal&sessionId=agent-1&profile=claude-code')
    render(<DetachedTerminalApp />)

    expect(screen.getByRole('main').dataset.theme).toBe('light')
    expect(screen.getByTestId('terminal-agent-1').dataset.theme).toBe('light')

    fireEvent.keyDown(document, { key: 'i', metaKey: true })
    expect(screen.getByRole('main').dataset.theme).toBe('dark')
  })

  it('gives an Agent detached window the same bottom HUD controls immediately', () => {
    window.history.replaceState({}, '', '/?kind=detached-terminal&sessionId=agent-1&profile=claude-code&title=Claude')
    render(<DetachedTerminalApp />)

    expect(screen.getByTestId('terminal-agent-1')).toBeTruthy()
    expect(screen.getByLabelText('快捷指令栏').querySelector('[data-hud-mode="agent"]')).toBeTruthy()
    expect(screen.getByRole('button', { name: /当前权限模式：Default/ })).toBeTruthy()
    expect(screen.getByRole('button', { name: '点击切换模型' }).textContent).toBe('Opus Plan')
  })

  it('keeps the detached model control connected to the same Session command path', async () => {
    const user = userEvent.setup()
    window.history.replaceState({}, '', '/?kind=detached-terminal&sessionId=agent-1&profile=claude-code')
    render(<DetachedTerminalApp />)

    await user.click(screen.getByRole('button', { name: '点击切换模型' }))
    await user.click(screen.getByRole('menuitem', { name: 'Claude Sonnet 4.6' }))

    expect(runtime.request).toHaveBeenCalledWith('session.set-model', expect.objectContaining({
      input: expect.objectContaining({ sessionId: 'agent-1', modelStrategy: 'claude-sonnet-4-6' })
    }))
  })
})
