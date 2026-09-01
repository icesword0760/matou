// @vitest-environment jsdom
import { cleanup, render, waitFor } from '@testing-library/react'
import { useLayoutEffect } from 'react'
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
  terminalResize: vi.fn(),
  terminalWrite: vi.fn((_data: unknown, done?: () => void) => done?.()),
  attachTerminal: vi.fn(),
  sendTerminalInput: vi.fn(),
  updateTerminalProfile: vi.fn(),
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
    write = state.terminalWrite
    resize = state.terminalResize
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
  useRuntimeClient: (() => {
    const client = {
    attachTerminal: (_descriptor: unknown, onMessage: (message: unknown) => void) => {
      state.attachTerminal(_descriptor)
      state.onMessage = onMessage
      return vi.fn()
    },
    acknowledgeTerminal: vi.fn(),
    requestTerminalReplay: vi.fn(),
    resizeTerminal: vi.fn(),
    sendTerminalInput: state.sendTerminalInput,
    updateTerminalProfile: state.updateTerminalProfile,
    recordTerminalInteraction: state.recordTerminalInteraction
    }
    return () => client
  })()
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
    state.attachTerminal.mockClear()
    state.updateTerminalProfile.mockClear()
    state.recordTerminalInteraction.mockClear()
    state.terminalResize.mockClear()
    state.terminalWrite.mockClear()
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

  it('replays historical terminal resize frames before continuing output', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))

    state.onMessage?.({
      type: 'terminal.replay-resize', sessionId: 'session-1', sequence: 2,
      cols: 100, rows: 40
    })

    expect(state.terminalResize).toHaveBeenCalledWith(100, 40)
  })

  it('shows durable completed Shell Blocks before the fresh live prompt', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    const history = new TextEncoder().encode('❯ previous\r\ndone\r\n── 会话已恢复 ──\r\n')

    state.onMessage?.({
      type: 'terminal.restored-history', sessionId: 'session-1', data: history, blockCount: 1
    })
    state.onMessage?.({ type: 'terminal.data', data: new TextEncoder().encode('% '), sequence: 1 })

    expect(state.terminalWrite.mock.calls.map(([data]) =>
      new TextDecoder().decode(data as Uint8Array)
    )).toEqual([
      '❯ previous\r\ndone\r\n── 会话已恢复 ──\r\n',
      '% '
    ])
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

  it('does not let late output from a terminal that became inactive steal focus during the same commit', async () => {
    function OutputDuringCommit({ active }: { active: boolean }) {
      useLayoutEffect(() => {
        if (active) return
        state.focus.mockClear()
        state.onMessage?.({ type: 'terminal.data', data: new Uint8Array([65]), sequence: 1 })
      }, [active])
      return <TerminalSurface sessionId="session-1" active={active} visible />
    }

    const view = render(<OutputDuringCommit active />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))

    view.rerender(<OutputDuringCommit active={false} />)

    expect(state.focus).not.toHaveBeenCalled()
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

  it('moves a session only after submitted or control input has reached the terminal', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onMessage?.({ type: 'terminal.spawned', pid: 123 })

    state.onData?.('draft text')
    expect(state.recordTerminalInteraction).not.toHaveBeenCalled()
    expect(state.sendTerminalInput).toHaveBeenLastCalledWith('session-1', 'draft text')

    const callOrder: string[] = []
    state.recordTerminalInteraction.mockImplementation(() => callOrder.push('interaction'))
    state.sendTerminalInput.mockImplementation(() => callOrder.push('input'))
    state.onData?.('\r')
    expect(state.recordTerminalInteraction).toHaveBeenLastCalledWith('session-1', 'submit', true)
    expect(callOrder).toEqual(['input', 'interaction'])

    callOrder.length = 0
    state.onData?.('\u0003')
    expect(state.recordTerminalInteraction).toHaveBeenLastCalledWith('session-1', 'control', true)
    expect(callOrder).toEqual(['input', 'interaction'])

    state.recordTerminalInteraction.mockClear()
    state.onData?.('\u001b[200~pasted\ntext\u001b[201~')
    expect(state.recordTerminalInteraction).not.toHaveBeenCalled()
  })

  it('reports accepted terminal input to its owner', async () => {
    const onUserInput = vi.fn()
    const view = render(<TerminalSurface sessionId="session-1" active visible onUserInput={onUserInput} />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))

    state.onData?.('pwd')
    expect(onUserInput).toHaveBeenCalledTimes(1)

    view.rerender(<TerminalSurface sessionId="session-1" active visible inputDisabled onUserInput={onUserInput} />)
    await waitFor(() => state.onData?.('ignored'))
    expect(onUserInput).toHaveBeenCalledTimes(1)
  })

  it('treats Escape as a completed Claude action but not as Shell navigation', async () => {
    const view = render(
      <TerminalSurface sessionId="session-1" profile="claude-code" active visible />
    )
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onMessage?.({ type: 'terminal.spawned', pid: 123 })

    state.onData?.('\u001b')
    expect(state.sendTerminalInput).toHaveBeenLastCalledWith('session-1', '\u001b')
    expect(state.recordTerminalInteraction).toHaveBeenLastCalledWith(
      'session-1', 'provider-action', true
    )

    state.recordTerminalInteraction.mockClear()
    view.rerender(<TerminalSurface sessionId="session-1" profile="shell" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onData?.('\u001b')
    expect(state.recordTerminalInteraction).not.toHaveBeenCalled()
  })

  it('keeps one terminal stream while a Shell card is promoted to Claude', async () => {
    const view = render(
      <TerminalSurface sessionId="session-1" profile="shell" active visible />
    )
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onMessage?.({ type: 'terminal.spawned', pid: 123 })
    const inputBeforePromotion = state.onData
    const messageBeforePromotion = state.onMessage
    const attachesBeforePromotion = state.attachTerminal.mock.calls.length

    state.onMessage?.({
      type: 'terminal.data', data: new TextEncoder().encode('(base) % cc'), sequence: 1
    })
    view.rerender(
      <TerminalSurface sessionId="session-1" profile="claude-code" active visible />
    )
    state.onMessage?.({
      type: 'terminal.data', data: new TextEncoder().encode('Claude Code ready'), sequence: 2
    })

    expect(state.attachTerminal).toHaveBeenCalledTimes(attachesBeforePromotion)
    expect(state.onData).toBe(inputBeforePromotion)
    expect(state.onMessage).toBe(messageBeforePromotion)
    expect(state.terminalWrite.mock.calls.map(([data]) =>
      new TextDecoder().decode(data as Uint8Array)
    )).toEqual(expect.arrayContaining(['(base) % cc', 'Claude Code ready']))
    expect(state.updateTerminalProfile).toHaveBeenLastCalledWith(
      'session-1', 'claude-code'
    )
    state.recordTerminalInteraction.mockClear()
    state.onData?.('\u001b')
    expect(state.recordTerminalInteraction).toHaveBeenCalledWith(
      'session-1', 'provider-action', true
    )
  })

  it('buffers keystrokes during an agent-to-Shell respawn and flushes them after the new PTY is attached', async () => {
    render(<TerminalSurface sessionId="session-1" profile="shell" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))

    state.onData?.("printf '%s\\n' 808")
    expect(state.sendTerminalInput).not.toHaveBeenCalled()
    state.onMessage?.({ type: 'terminal.spawned', pid: 456 })

    expect(state.sendTerminalInput).toHaveBeenCalledWith('session-1', "printf '%s\\n' 808")
  })

  it('keeps a partially typed command intact while the same Session changes provider mode', async () => {
    const view = render(<TerminalSurface sessionId="session-1" profile="claude-code" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onData?.("printf '%s\\n' \"$((800 + 8)")

    const providerInput = state.onData
    view.rerender(<TerminalSurface sessionId="session-1" profile="shell" active visible />)
    expect(state.onData).toBe(providerInput)
    state.onData?.(")\"\r")
    state.onMessage?.({ type: 'terminal.spawned', pid: 789 })

    expect(state.sendTerminalInput).toHaveBeenCalledWith(
      'session-1', "printf '%s\\n' \"$((800 + 8))\"\r"
    )
  })

  it('treats live terminal output as proof that a reattached input channel is ready', async () => {
    render(<TerminalSurface sessionId="session-1" profile="claude-code" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onMessage?.({ type: 'terminal.data', data: new Uint8Array([65]), sequence: 1 })

    state.onData?.('\u001b[B')
    expect(state.sendTerminalInput).toHaveBeenCalledWith('session-1', '\u001b[B')
  })

  it('forwards a short Option+Tab only to the active visible terminal', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onMessage?.({ type: 'terminal.spawned', pid: 123 })

    window.dispatchEvent(new Event('matou:forward-terminal-tab'))
    expect(state.sendTerminalInput).toHaveBeenLastCalledWith('session-1', '\t')

    state.sendTerminalInput.mockClear()
    view.rerender(<TerminalSurface sessionId="session-1" active={false} visible />)
    window.dispatchEvent(new Event('matou:forward-terminal-tab'))
    expect(state.sendTerminalInput).not.toHaveBeenCalled()
  })
})
