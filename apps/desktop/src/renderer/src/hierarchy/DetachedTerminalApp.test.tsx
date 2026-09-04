// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { useEffect, useRef } from 'react'

import { DetachedTerminalApp } from './DetachedTerminalApp'

vi.mock('../terminal/TerminalSurface', () => ({
  TerminalSurface: (props: {
    sessionId: string; themeKey?: string; fontSize?: number; readOnly?: boolean; inputDisabled?: boolean
    focusRequest?: number
    onStorageFault?(fault: {
      type: 'terminal.storage-fault'; protocolVersion: 1; sessionId: string; sequence: number
      code: 'STORAGE_WRITE_FAILED'; message: string; retainedBytes: number
    }): void
  }) => {
    const inputRef = useRef<HTMLSpanElement>(null)
    useEffect(() => {
      if (props.focusRequest) inputRef.current?.focus()
    }, [props.focusRequest])
    return <div className="terminal-surface" data-session-id={props.sessionId}
      data-testid={`terminal-${props.sessionId}`} data-theme={props.themeKey} data-font-size={props.fontSize}
      data-read-only={props.readOnly} data-input-disabled={props.inputDisabled}>
      <span tabIndex={-1} data-terminal-input ref={inputRef} />
      <button type="button" aria-label="触发独立窗口存储异常" onClick={() => props.onStorageFault?.({
      type: 'terminal.storage-fault', protocolVersion: 1, sessionId: props.sessionId, sequence: 1,
      code: 'STORAGE_WRITE_FAILED', message: 'disk offline', retainedBytes: 128
      })} />
    </div>
  }
}))
const runtime = vi.hoisted(() => ({
  request: vi.fn(async (method: string) => method === 'projection.snapshot' ? { hierarchy: {} } : {}),
  subscribeProjection: vi.fn(() => () => {})
}))
vi.mock('../runtime/RuntimeProvider', () => ({ useRuntimeClient: () => runtime }))

afterEach(() => {
  cleanup()
  window.localStorage.clear()
  runtime.request.mockClear()
  runtime.subscribeProjection.mockClear()
  window.history.replaceState({}, '', '/')
  Object.defineProperty(window, 'matouDesktop', { configurable: true, writable: true, value: undefined })
  vi.useRealTimers()
})

describe('PRD 02 detached HUD', () => {
  it('acknowledges an exact focus attempt only after its terminal owns native and DOM focus', async () => {
    let listener: ((request: {
      requestId: string; attemptId: string; routeWindowId: string
      targetWindowId: string; sessionId: string; deadlineAt: number
    }) => void) | undefined
    const acknowledgeDetachedTerminalFocus = vi.fn(async () => true)
    window.matouDesktop = {
      onDetachedTerminalFocusRequested: vi.fn((next) => {
        listener = next
        return () => { listener = undefined }
      }),
      acknowledgeDetachedTerminalFocus,
      isCurrentWindowFocused: vi.fn(async () => true)
    } as unknown as typeof window.matouDesktop
    window.history.replaceState({}, '',
      '/?kind=detached-terminal&windowId=detached-1&mainWindowId=main-1&sceneId=scene-1&sessionId=agent-1&profile=claude-code')
    render(<DetachedTerminalApp />)
    const request = {
      requestId: 'nav-1', attemptId: 'attempt-7', routeWindowId: 'main-1',
      targetWindowId: 'detached-1', sessionId: 'agent-1', deadlineAt: Date.now() + 1_000
    }

    await waitFor(() => expect(listener).toBeTypeOf('function'))
    listener?.(request)

    await waitFor(() => expect(acknowledgeDetachedTerminalFocus).toHaveBeenCalledWith({
      ...request, focused: true
    }))
    expect(document.activeElement?.hasAttribute('data-terminal-input')).toBe(true)
  })

  it('uses and updates the persisted terminal font size', () => {
    window.localStorage.setItem('matou:terminal-font-size', '14')
    Object.defineProperty(navigator, 'platform', { configurable: true, value: 'MacIntel' })
    window.history.replaceState({}, '', '/?kind=detached-terminal&sessionId=agent-1&profile=claude-code')
    render(<DetachedTerminalApp />)

    expect(screen.getByTestId('terminal-agent-1').dataset.fontSize).toBe('14')
    fireEvent.keyDown(document, { key: '+', metaKey: true })
    expect(screen.getByTestId('terminal-agent-1').dataset.fontSize).toBe('15')
    expect(window.localStorage.getItem('matou:terminal-font-size')).toBe('15')
  })

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

  it('turns an existing independent terminal into replay-only browsing with one clear reason', async () => {
    window.matouDesktop = {
      exportDatabaseRecoveryBundle: vi.fn(async () => ({ exportedPath: '/tmp/export' }))
    } as unknown as typeof window.matouDesktop
    window.history.replaceState({}, '', '/?kind=detached-terminal&sessionId=agent-1&profile=claude-code')
    const view = render(<DetachedTerminalApp runtimeMode="normal" />)
    expect(screen.getByText('独立窗口 · 会话保持运行')).toBeTruthy()

    view.rerender(<DetachedTerminalApp runtimeMode="read-only" />)

    expect(screen.getByText('数据库处于只读恢复模式')).toBeTruthy()
    expect(screen.queryByText('独立窗口 · 会话保持运行')).toBeNull()
    expect(screen.getByTestId('terminal-agent-1').dataset.readOnly).toBe('true')
    expect(screen.getByTestId('terminal-agent-1').dataset.inputDisabled).toBe('true')
    const permission = screen.getByRole('button', { name: /当前权限模式：Default/ })
    expect(permission.hasAttribute('disabled')).toBe(true)
    expect(permission.getAttribute('title')).toBe('数据库处于只读恢复模式')
    expect(runtime.request).not.toHaveBeenCalledWith('session.set-permission-mode', expect.anything())
    expect(runtime.request).not.toHaveBeenCalledWith('session.set-model', expect.anything())
  })

  it('locks permission changes while the independent terminal is waiting for storage recovery', async () => {
    window.history.replaceState({}, '', '/?kind=detached-terminal&sessionId=agent-1&profile=claude-code')
    render(<DetachedTerminalApp />)

    await userEvent.setup().click(screen.getByRole('button', { name: '触发独立窗口存储异常' }))

    const permission = screen.getByRole('button', { name: /当前权限模式：Default/ })
    expect(permission).toHaveProperty('disabled', true)
    expect(permission.getAttribute('title')).toBe('终端存储异常，请先恢复或结束当前会话')
    expect(runtime.request).not.toHaveBeenCalledWith('session.set-permission-mode', expect.anything())
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
