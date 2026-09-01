// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
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
  Object.defineProperty(window, 'matouDesktop', { configurable: true, writable: true, value: undefined })
  vi.useRealTimers()
})

describe('PRD 02 detached HUD', () => {
  it('uses the same default white skin and reference product theme shortcut as the main terminal', () => {
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
    expect(screen.queryByRole('button', { name: '点击切换模型' })).toBeNull()
  })

  it('keeps the removed session model control out of detached windows', () => {
    window.history.replaceState({}, '', '/?kind=detached-terminal&sessionId=agent-1&profile=claude-code')
    render(<DetachedTerminalApp />)

    expect(screen.queryByRole('button', { name: '点击切换模型' })).toBeNull()
    expect(runtime.request).not.toHaveBeenCalledWith('session.set-model', expect.anything())
  })

  it('opens the same scene DAG after a long Option Tab hold from a detached session', () => {
    vi.useFakeTimers()
    const openDagWindow = vi.fn(async () => undefined)
    window.matouDesktop = { openDagWindow } as unknown as typeof window.matouDesktop
    window.history.replaceState({}, '',
      '/?kind=detached-terminal&windowId=detached-1&mainWindowId=main-1&sceneId=scene-1&sessionId=agent-1&profile=claude-code')
    render(<DetachedTerminalApp />)

    fireEvent.keyDown(window, { key: 'Tab', altKey: true })
    vi.advanceTimersByTime(450)

    expect(openDagWindow).toHaveBeenCalledWith({
      mainWindowId: 'main-1', sceneId: 'scene-1', sessionId: 'agent-1', theme: 'light'
    })
  })

  it('keeps an Agent Teams teammate read-only when it opens in an independent window', async () => {
    runtime.request.mockImplementationOnce(async () => ({
      hierarchy: {
        sessionGraphs: {
          'scene-1': {
            sceneId: 'scene-1', nodes: [{
              sessionId: 'teammate-1', sceneId: 'scene-1', currentMode: 'agent-team-member',
              workStatus: 'idle', providerRestoreState: 'none', canFork: false,
              title: 'MATOU_QA_TEAMMATE', cwd: '/tmp', activeChildCount: 0,
              stoppedChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
              latestLines: ['TEAMMATE_REAL_READY'], lastUserInteractionSeq: 0
            }], edges: []
          }
        }
      }
    }))
    window.history.replaceState({}, '',
      '/?kind=detached-terminal&windowId=detached-1&mainWindowId=main-1&sceneId=scene-1&sessionId=teammate-1&profile=agent-team-member&title=MATOU_QA_TEAMMATE')
    render(<DetachedTerminalApp />)

    await waitFor(() => expect(screen.getByRole('status', { name: '队友会话摘要' }).textContent)
      .toContain('TEAMMATE_REAL_READY'))
    expect(screen.queryByTestId('terminal-teammate-1')).toBeNull()
    expect(screen.queryByLabelText('快捷指令栏')).toBeNull()
  })
})
