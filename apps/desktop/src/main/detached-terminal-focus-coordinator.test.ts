import { describe, expect, it, vi } from 'vitest'

import { DetachedTerminalFocusCoordinator } from './detached-terminal-focus-coordinator'

describe('DetachedTerminalFocusCoordinator', () => {
  it('settles only from the exact target Renderer with matching attempt and native focus', async () => {
    let nativeFocused = false
    const send = vi.fn()
    const coordinator = new DetachedTerminalFocusCoordinator({
      resolveTarget: (windowId) => windowId === 'detached-1' ? {
        windowId,
        mainWindowId: 'main-1',
        sessionId: 'session-1',
        webContentsId: 42,
        showAndFocus: vi.fn(),
        send,
        isFocused: () => nativeFocused
      } : undefined
    })
    const request = {
      requestId: 'nav-1', attemptId: 'attempt-1', routeWindowId: 'main-1',
      targetWindowId: 'detached-1', sessionId: 'session-1', deadlineAt: Date.now() + 1_000
    }
    const result = coordinator.request(request)

    expect(send).toHaveBeenCalledWith(request)
    expect(coordinator.acknowledge({ ...request, focused: true }, 99)).toBe(false)
    expect(coordinator.acknowledge({ ...request, attemptId: 'attempt-other', focused: true }, 42)).toBe(false)
    expect(coordinator.acknowledge({ ...request, focused: true }, 42)).toBe(false)
    nativeFocused = true
    expect(coordinator.acknowledge({ ...request, focused: true }, 42)).toBe(true)
    await expect(result).resolves.toBe(true)
  })

  it('fails closed for mismatched target context and a renderer rejection', async () => {
    const coordinator = new DetachedTerminalFocusCoordinator({
      resolveTarget: () => ({
        windowId: 'detached-1', mainWindowId: 'main-1', sessionId: 'session-1',
        webContentsId: 42, showAndFocus: vi.fn(), send: vi.fn(), isFocused: () => true
      })
    })
    await expect(coordinator.request({
      requestId: 'nav-mismatch', attemptId: 'attempt-1', routeWindowId: 'main-other',
      targetWindowId: 'detached-1', sessionId: 'session-1', deadlineAt: Date.now() + 1_000
    })).resolves.toBe(false)

    const request = {
      requestId: 'nav-2', attemptId: 'attempt-2', routeWindowId: 'main-1',
      targetWindowId: 'detached-1', sessionId: 'session-1', deadlineAt: Date.now() + 1_000
    }
    const result = coordinator.request(request)
    expect(coordinator.acknowledge({ ...request, focused: false }, 42)).toBe(true)
    await expect(result).resolves.toBe(false)
  })
})
