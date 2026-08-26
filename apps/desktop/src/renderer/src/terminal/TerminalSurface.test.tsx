// @vitest-environment jsdom
import { cleanup, render, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { TerminalSurface } from './TerminalSurface'

const state = vi.hoisted(() => ({
  focus: vi.fn(),
  onMessage: undefined as undefined | ((message: unknown) => void)
}))

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80
    rows = 24
    parser = { registerOscHandler: vi.fn(() => ({ dispose: vi.fn() })) }
    loadAddon = vi.fn()
    open = vi.fn()
    focus = state.focus
    write = vi.fn((_data: unknown, done?: () => void) => done?.())
    onData = vi.fn(() => ({ dispose: vi.fn() }))
    reset = vi.fn()
    dispose = vi.fn()
  }
}))
vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class { fit = vi.fn() }
}))
vi.mock('../runtime/RuntimeProvider', () => ({
  useRuntimeClient: () => ({
    attachTerminal: (_descriptor: unknown, onMessage: (message: unknown) => void) => {
      state.onMessage = onMessage
      return vi.fn()
    },
    acknowledgeTerminal: vi.fn(),
    requestTerminalReplay: vi.fn(),
    resizeTerminal: vi.fn(),
    sendTerminalInput: vi.fn()
  })
}))

describe('TerminalSurface focus continuity', () => {
  beforeEach(() => {
    state.focus.mockClear()
    state.onMessage = undefined
    vi.stubGlobal('ResizeObserver', class {
      observe() {}
      disconnect() {}
    })
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
      callback(0)
      return 1
    })
    vi.stubGlobal('cancelAnimationFrame', vi.fn())
  })
  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
  })

  it('focuses a newly mounted active terminal without an extra click', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)

    await waitFor(() => expect(state.focus).toHaveBeenCalled())
  })

  it('restores the active terminal after agent output while leaving dialogs and controls alone', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    state.focus.mockClear()

    document.body.focus()
    state.onMessage?.({ type: 'terminal.data', data: new Uint8Array([65]), sequence: 1 })
    expect(state.focus).toHaveBeenCalledTimes(1)

    const button = document.createElement('button')
    view.container.append(button)
    button.focus()
    state.onMessage?.({ type: 'terminal.data', data: new Uint8Array([66]), sequence: 2 })
    expect(state.focus).toHaveBeenCalledTimes(1)
  })
})
