// @vitest-environment jsdom
import { cleanup, render, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { TerminalSurface } from './TerminalSurface'

const state = vi.hoisted(() => ({
  focus: vi.fn(),
  searchNext: vi.fn(),
  searchPrevious: vi.fn(),
  clearDecorations: vi.fn(),
  searchResultsListener: undefined as undefined | ((result: { resultIndex: number; resultCount: number }) => void),
  onMessage: undefined as undefined | ((message: unknown) => void),
  onData: undefined as undefined | ((data: string) => void),
  sendTerminalInput: vi.fn(),
  recordTerminalInteraction: vi.fn()
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
    onData = vi.fn((listener: (data: string) => void) => {
      state.onData = listener
      return { dispose: vi.fn() }
    })
    reset = vi.fn()
    dispose = vi.fn()
  }
}))
vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class { fit = vi.fn() }
}))
vi.mock('@xterm/addon-search', () => ({
  SearchAddon: class {
    findNext = state.searchNext
    findPrevious = state.searchPrevious
    clearDecorations = state.clearDecorations
    onDidChangeResults = (listener: (result: { resultIndex: number; resultCount: number }) => void) => {
      state.searchResultsListener = listener
      return { dispose: vi.fn() }
    }
  }
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
    sendTerminalInput: state.sendTerminalInput,
    recordTerminalInteraction: state.recordTerminalInteraction
  })
}))

describe('TerminalSurface focus continuity', () => {
  beforeEach(() => {
    state.focus.mockClear()
    state.searchNext.mockClear()
    state.searchPrevious.mockClear()
    state.clearDecorations.mockClear()
    state.searchResultsListener = undefined
    state.onMessage = undefined
    state.onData = undefined
    state.sendTerminalInput.mockClear()
    state.recordTerminalInteraction.mockClear()
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

  it('searches the real terminal buffer in both directions and reports result counts', async () => {
    const onSearchResults = vi.fn()
    const view = render(<TerminalSurface sessionId="session-1" active visible
      searchRequest={{
        query: 'MATOU_TOKEN', direction: 'next', sequence: 1,
        options: { caseSensitive: true, regex: false, wholeWord: true }
      }} onSearchResults={onSearchResults} />)

    await waitFor(() => expect(state.searchNext).toHaveBeenCalledWith('MATOU_TOKEN', expect.objectContaining({
      caseSensitive: true, regex: false, wholeWord: true, incremental: true
    })))
    state.searchResultsListener?.({ resultIndex: 2, resultCount: 5 })
    expect(onSearchResults).toHaveBeenCalledWith({ resultIndex: 2, resultCount: 5 })

    view.rerender(<TerminalSurface sessionId="session-1" active visible
      searchRequest={{
        query: 'MATOU_TOKEN', direction: 'previous', sequence: 2,
        options: { caseSensitive: true, regex: false, wholeWord: true }
      }} onSearchResults={onSearchResults} />)
    await waitFor(() => expect(state.searchPrevious).toHaveBeenCalled())
  })

  it('clears terminal search decorations when the query becomes empty', async () => {
    render(<TerminalSurface sessionId="session-1" active visible
      searchRequest={{
        query: '', direction: 'next', sequence: 1,
        options: { caseSensitive: false, regex: false, wholeWord: false }
      }} />)

    await waitFor(() => expect(state.clearDecorations).toHaveBeenCalled())
  })

  it('focuses the active terminal when its owner requests focus restoration', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible focusRequest={0} />)
    await waitFor(() => expect(state.focus).toHaveBeenCalled())
    const button = document.createElement('button')
    view.container.append(button)
    button.focus()
    state.focus.mockClear()
    expect(state.focus).not.toHaveBeenCalled()

    view.rerender(<TerminalSurface sessionId="session-1" active visible focusRequest={1} />)

    await waitFor(() => expect(state.focus).toHaveBeenCalledTimes(1))
  })

  it('moves a session only for submitted or control input, before sending related bytes', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))

    state.onData?.('draft text')
    expect(state.recordTerminalInteraction).not.toHaveBeenCalled()
    expect(state.sendTerminalInput).toHaveBeenLastCalledWith('session-1', 'draft text')

    const callOrder: string[] = []
    state.recordTerminalInteraction.mockImplementation(() => callOrder.push('interaction'))
    state.sendTerminalInput.mockImplementation(() => callOrder.push('input'))
    state.onData?.('\r')
    expect(state.recordTerminalInteraction).toHaveBeenLastCalledWith('session-1', 'submit')
    expect(callOrder).toEqual(['interaction', 'input'])

    callOrder.length = 0
    state.onData?.('\u0003')
    expect(state.recordTerminalInteraction).toHaveBeenLastCalledWith('session-1', 'control')
    expect(callOrder).toEqual(['interaction', 'input'])

    state.recordTerminalInteraction.mockClear()
    state.onData?.('\u001b[200~pasted\ntext\u001b[201~')
    expect(state.recordTerminalInteraction).not.toHaveBeenCalled()
  })

  it('forwards a short Option+Tab only to the active visible terminal', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))

    window.dispatchEvent(new Event('matou:forward-terminal-tab'))
    expect(state.sendTerminalInput).toHaveBeenLastCalledWith('session-1', '\t')

    state.sendTerminalInput.mockClear()
    view.rerender(<TerminalSurface sessionId="session-1" active={false} visible />)
    window.dispatchEvent(new Event('matou:forward-terminal-tab'))
    expect(state.sendTerminalInput).not.toHaveBeenCalled()
  })
})
