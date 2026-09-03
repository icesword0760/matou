// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useLayoutEffect } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { TerminalSurface } from './TerminalSurface'
import { foregroundTerminalModels } from './terminal-model-cache'

const state = vi.hoisted(() => ({
  focus: vi.fn(),
  searchNext: vi.fn(),
  searchPrevious: vi.fn(),
  clearDecorations: vi.fn(),
  searchResultsListener: undefined as undefined | ((result: { resultIndex: number; resultCount: number }) => void),
  onMessage: undefined as undefined | ((message: unknown) => void),
  onData: undefined as undefined | ((data: string) => void),
  onRender: undefined as undefined | ((range: { start: number; end: number }) => void),
  terminalResize: vi.fn(),
  terminalRefresh: vi.fn(),
  fit: vi.fn(),
  resizeTerminal: vi.fn(),
  resizeObserverCallback: undefined as ResizeObserverCallback | undefined,
  terminalWrite: vi.fn((_data: unknown, done?: () => void) => done?.()),
  terminalReset: vi.fn(),
  terminalConstructed: vi.fn(),
  terminalDisposed: vi.fn(),
  webglConstructed: vi.fn(),
  webglDisposed: vi.fn(),
  webglContextLossListener: undefined as undefined | (() => void),
  serialize: vi.fn(() => '\u001b[2Jserialized screen'),
  attachTerminal: vi.fn(),
  sendTerminalInput: vi.fn(),
  updateTerminalProfile: vi.fn(),
  recordTerminalInteraction: vi.fn(),
  storeTerminalCheckpoint: vi.fn(),
  searchTerminalHistory: vi.fn(),
  historyAroundTerminalCursor: vi.fn(),
  acknowledgeTerminal: vi.fn(),
  requestTerminalReplay: vi.fn()
}))

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    element = document.createElement('div')
    options: Record<string, unknown> = {}
    cols = 80
    rows = 24
    buffer = {
      active: {
        length: 1,
        getLine: () => ({ translateToString: () => 'observed terminal buffer' })
      }
    }
    parser = { registerOscHandler: vi.fn(() => ({ dispose: vi.fn() })) }
    loadAddon = vi.fn()
    constructor() { state.terminalConstructed() }
    open = vi.fn((container: HTMLElement) => container.appendChild(this.element))
    focus = state.focus
    write = state.terminalWrite
    resize = state.terminalResize
    refresh = state.terminalRefresh
    onData = vi.fn((listener: (data: string) => void) => {
      state.onData = listener
      return { dispose: vi.fn() }
    })
    onRender = vi.fn((listener: (range: { start: number; end: number }) => void) => {
      state.onRender = listener
      return { dispose: vi.fn() }
    })
    reset = state.terminalReset
    dispose = state.terminalDisposed
  }
}))
vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class { fit = state.fit }
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
vi.mock('@xterm/addon-serialize', () => ({
  SerializeAddon: class { serialize = state.serialize }
}))
vi.mock('@xterm/addon-webgl', () => ({
  WebglAddon: class {
    constructor() { state.webglConstructed() }
    onContextLoss = (listener: () => void) => {
      state.webglContextLossListener = listener
      return { dispose: vi.fn() }
    }
    dispose = state.webglDisposed
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
    acknowledgeTerminal: state.acknowledgeTerminal,
    requestTerminalReplay: state.requestTerminalReplay,
    resizeTerminal: state.resizeTerminal,
    sendTerminalInput: state.sendTerminalInput,
    updateTerminalProfile: state.updateTerminalProfile,
    recordTerminalInteraction: state.recordTerminalInteraction,
    storeTerminalCheckpoint: state.storeTerminalCheckpoint,
    searchTerminalHistory: state.searchTerminalHistory,
    historyAroundTerminalCursor: state.historyAroundTerminalCursor
    }
    return () => client
  })()
}))

describe('TerminalSurface focus continuity', () => {
  beforeEach(() => {
    window.history.replaceState({}, '', '/')
    foregroundTerminalModels.setForegroundSessions([])
    state.focus.mockClear()
    state.searchNext.mockClear()
    state.searchPrevious.mockClear()
    state.clearDecorations.mockClear()
    state.searchResultsListener = undefined
    state.onMessage = undefined
    state.onData = undefined
    state.onRender = undefined
    state.sendTerminalInput.mockClear()
    state.attachTerminal.mockClear()
    state.updateTerminalProfile.mockClear()
    state.recordTerminalInteraction.mockClear()
    state.terminalResize.mockClear()
    state.terminalRefresh.mockClear()
    state.fit.mockClear()
    state.resizeTerminal.mockClear()
    state.resizeObserverCallback = undefined
    state.terminalWrite.mockClear()
    state.terminalReset.mockClear()
    state.terminalConstructed.mockClear()
    state.terminalDisposed.mockClear()
    state.webglConstructed.mockClear()
    state.webglDisposed.mockClear()
    state.webglContextLossListener = undefined
    state.serialize.mockClear()
    state.storeTerminalCheckpoint.mockClear()
    state.searchTerminalHistory.mockReset()
    state.historyAroundTerminalCursor.mockReset()
    state.historyAroundTerminalCursor.mockResolvedValue({
      lines: [], gaps: [], hasMore: false
    })
    state.acknowledgeTerminal.mockClear()
    state.requestTerminalReplay.mockClear()
    vi.stubGlobal('ResizeObserver', class {
      constructor(callback: ResizeObserverCallback) { state.resizeObserverCallback = callback }
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
    vi.useRealTimers()
    cleanup()
    Reflect.deleteProperty(window, 'matouDesktop')
    vi.unstubAllGlobals()
  })

  it('focuses a newly mounted active terminal without an extra click', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)

    await waitFor(() => expect(state.focus).toHaveBeenCalled())
  })

  it('reports visual readiness only after terminal output has been applied', async () => {
    const onVisualReady = vi.fn()
    render(<TerminalSurface sessionId="session-1" active visible onVisualReady={onVisualReady} />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))

    state.onMessage?.({
      type: 'terminal.spawned', sessionId: 'session-1', pid: 456,
      reattached: true, replayFromSequence: 1
    })
    expect(onVisualReady).not.toHaveBeenCalled()

    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 1,
      data: new TextEncoder().encode('restored terminal frame')
    })
    expect(onVisualReady).not.toHaveBeenCalled()

    state.onRender?.({ start: 0, end: 1 })
    expect(onVisualReady).toHaveBeenCalledTimes(1)

    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 2,
      data: new TextEncoder().encode('next frame')
    })
    expect(onVisualReady).toHaveBeenCalledTimes(1)
  })

  it('uses WebGL after opening xterm and falls back when the GPU context is lost', async () => {
    window.history.replaceState({}, '', '/?e2e=1')
    render(<TerminalSurface sessionId="session-webgl" active visible />)
    await waitFor(() => expect(state.webglConstructed).toHaveBeenCalledTimes(1))
    expect(document.querySelector('.e2e-terminal-observer')?.classList.contains('xterm-rows'))
      .toBe(true)

    state.webglContextLossListener?.()

    expect(state.webglDisposed).toHaveBeenCalledTimes(1)
    expect(document.querySelector('.e2e-terminal-observer')?.classList.contains('xterm-rows'))
      .toBe(false)
  })

  it('fits and resizes the PTY once after a burst of card width changes settles', () => {
    vi.useFakeTimers()
    render(<TerminalSurface sessionId="session-resize-settle" active visible />)
    act(() => { vi.runOnlyPendingTimers() })
    state.fit.mockClear()
    state.resizeTerminal.mockClear()

    act(() => {
      state.resizeObserverCallback?.([], {} as ResizeObserver)
      vi.advanceTimersByTime(40)
      state.resizeObserverCallback?.([], {} as ResizeObserver)
      vi.advanceTimersByTime(40)
      state.resizeObserverCallback?.([], {} as ResizeObserver)
      vi.advanceTimersByTime(79)
    })

    expect(state.fit).not.toHaveBeenCalled()
    expect(state.resizeTerminal).not.toHaveBeenCalled()

    act(() => { vi.advanceTimersByTime(1) })

    expect(state.fit).toHaveBeenCalledTimes(1)
    act(() => { vi.runOnlyPendingTimers() })
    expect(state.resizeTerminal).toHaveBeenCalledTimes(1)
    expect(state.resizeTerminal).toHaveBeenCalledWith('session-resize-settle', 80, 24)
  })

  it('keeps an inactive preview from resizing the PTY when its card expands', () => {
    vi.useFakeTimers()
    render(<TerminalSurface sessionId="session-preview" active={false} visible />)
    act(() => { vi.runOnlyPendingTimers() })
    state.fit.mockClear()
    state.resizeTerminal.mockClear()

    act(() => {
      state.resizeObserverCallback?.([], {} as ResizeObserver)
      vi.advanceTimersByTime(80)
      vi.runOnlyPendingTimers()
    })

    expect(state.fit).not.toHaveBeenCalled()
    expect(state.resizeTerminal).not.toHaveBeenCalled()
  })

  it('publishes the focused card dimensions when an inactive preview becomes active', () => {
    const view = render(<TerminalSurface sessionId="session-preview" active={false} visible />)
    state.fit.mockClear()
    state.resizeTerminal.mockClear()

    view.rerender(<TerminalSurface sessionId="session-preview" active visible />)

    expect(state.fit).toHaveBeenCalled()
    expect(state.resizeTerminal).toHaveBeenCalledWith('session-preview', 80, 24)
  })

  it('settles the old active card once after it loses focus, then ignores hover resizes', () => {
    vi.useFakeTimers()
    const view = render(<TerminalSurface sessionId="session-preview" active visible />)
    act(() => { vi.runAllTimers() })
    state.fit.mockClear()
    state.resizeTerminal.mockClear()

    view.rerender(<TerminalSurface sessionId="session-preview" active={false} visible />)
    act(() => {
      state.resizeObserverCallback?.([], {} as ResizeObserver)
      vi.advanceTimersByTime(80)
      vi.runOnlyPendingTimers()
    })
    expect(state.resizeTerminal).toHaveBeenCalledWith('session-preview', 80, 24)

    state.fit.mockClear()
    state.resizeTerminal.mockClear()
    act(() => {
      state.resizeObserverCallback?.([], {} as ResizeObserver)
      vi.advanceTimersByTime(80)
      vi.runOnlyPendingTimers()
    })
    expect(state.fit).not.toHaveBeenCalled()
    expect(state.resizeTerminal).not.toHaveBeenCalled()
  })

  it('spreads cold WebGL setup for inactive moving cards across animation frames', async () => {
    const animationFrames: FrameRequestCallback[] = []
    const activationMicrotasks: VoidFunction[] = []
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
      animationFrames.push(callback)
      return animationFrames.length
    })
    vi.stubGlobal('queueMicrotask', (callback: VoidFunction) => {
      activationMicrotasks.push(callback)
    })
    render(<>
      <TerminalSurface sessionId="session-moving-1" active={false} visible viewportMoving />
      <TerminalSurface sessionId="session-moving-2" active={false} visible viewportMoving />
    </>)
    expect(state.terminalConstructed).toHaveBeenCalledTimes(2)
    expect(state.webglConstructed).not.toHaveBeenCalled()

    act(() => {
      activationMicrotasks.shift()?.()
    })
    expect(state.webglConstructed).toHaveBeenCalledTimes(1)

    act(() => {
      while (animationFrames.length > 0 && state.webglConstructed.mock.calls.length === 1) {
        animationFrames.shift()?.(16)
      }
    })
    expect(state.webglConstructed).toHaveBeenCalledTimes(2)
  })

  it('reuses its xterm VT model after foreground card DOM virtualization', async () => {
    foregroundTerminalModels.setForegroundSessions(['session-1'])
    const first = render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.attachTerminal).toHaveBeenCalledTimes(1))

    first.unmount()
    expect(state.terminalDisposed).not.toHaveBeenCalled()
    render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.attachTerminal).toHaveBeenCalledTimes(2))

    expect(state.terminalConstructed).toHaveBeenCalledTimes(1)
  })

  it('keeps cached history when a restarted Runtime reattaches the foreground terminal', async () => {
    foregroundTerminalModels.setForegroundSessions(['session-1'])
    const first = render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 1,
      data: new TextEncoder().encode('history before Runtime restart')
    })
    first.unmount()

    render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.attachTerminal).toHaveBeenCalledTimes(2))
    state.onMessage?.({
      type: 'terminal.spawned', sessionId: 'session-1', pid: 456,
      reattached: true, replayFromSequence: 2
    })
    state.onMessage?.({
      type: 'terminal.replay-start', sessionId: 'session-1', source: 'tail',
      fromSequence: 2, throughSequence: 3, instantLineLimit: 10_000,
      availableFromSequence: 1, liveSequence: 3
    })

    expect(state.terminalConstructed).toHaveBeenCalledTimes(1)
    expect(state.terminalReset).not.toHaveBeenCalled()
  })

  it('continues replay after the last painted frame when a cached terminal remounts', async () => {
    foregroundTerminalModels.setForegroundSessions(['session-1'])
    const first = render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 7,
      data: new TextEncoder().encode('already painted once')
    })
    first.unmount()

    state.requestTerminalReplay.mockClear()
    render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.attachTerminal).toHaveBeenCalledTimes(2))
    state.onMessage?.({
      type: 'terminal.spawned', sessionId: 'session-1', pid: 456,
      reattached: true, replayFromSequence: 1
    })

    expect(state.requestTerminalReplay).toHaveBeenCalledWith('session-1', 8, true)
  })

  it('keeps an offscreen foreground terminal live without parsing every hidden output frame', async () => {
    render(<TerminalSurface sessionId="session-1" active={false} visible={false} foreground />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    const hiddenBytes = new TextEncoder().encode('hidden sustained output')

    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 9, data: hiddenBytes
    })

    expect(state.terminalWrite).not.toHaveBeenCalled()
    expect(state.acknowledgeTerminal).toHaveBeenCalledWith('session-1', 9)
  })

  it('keeps the focused terminal visually live when viewport bookkeeping briefly marks it hidden', async () => {
    render(<TerminalSurface sessionId="session-1" active visible={false} foreground />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))

    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 3,
      data: new TextEncoder().encode('focused-output')
    })

    expect(state.terminalWrite).toHaveBeenCalledWith(
      expect.any(Uint8Array), expect.any(Function)
    )
    expect(new TextDecoder().decode(state.terminalWrite.mock.calls[0]![0] as Uint8Array))
      .toBe('focused-output')
  })

  it('replays the bounded terminal tail when an offscreen foreground card returns to view', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 1,
      data: new TextEncoder().encode('visible')
    })
    view.rerender(<TerminalSurface sessionId="session-1" active={false} visible={false} foreground />)
    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 2,
      data: new TextEncoder().encode('hidden')
    })
    state.requestTerminalReplay.mockClear()

    view.rerender(<TerminalSurface sessionId="session-1" active visible foreground />)

    expect(state.requestTerminalReplay).toHaveBeenCalledWith('session-1', 2, true)
  })

  it('waits for an inactive preview to settle before replaying its offscreen tail', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    view.rerender(<TerminalSurface sessionId="session-1" active={false} visible={false} foreground />)
    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 4,
      data: new TextEncoder().encode('hidden')
    })
    state.requestTerminalReplay.mockClear()
    vi.useFakeTimers()

    view.rerender(<TerminalSurface sessionId="session-1" active={false} visible foreground />)
    await act(() => vi.advanceTimersByTimeAsync(499))
    expect(state.requestTerminalReplay).not.toHaveBeenCalled()
    await act(() => vi.advanceTimersByTimeAsync(1))

    expect(state.requestTerminalReplay).toHaveBeenCalledWith('session-1', 0, false)
  })

  it('defers even the focused terminal catch-up while the horizontal viewport is moving', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    view.rerender(<TerminalSurface sessionId="session-1" active={false} visible={false} foreground />)
    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 4,
      data: new TextEncoder().encode('hidden')
    })
    state.requestTerminalReplay.mockClear()

    view.rerender(<TerminalSurface sessionId="session-1" active visible foreground viewportMoving />)
    expect(state.requestTerminalReplay).not.toHaveBeenCalled()
    view.rerender(<TerminalSurface sessionId="session-1" active visible foreground viewportMoving={false} />)

    expect(state.requestTerminalReplay).toHaveBeenCalledWith('session-1', 0, false)
  })

  it('resumes live painting when a hidden-terminal catch-up reports a Journal gap', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    view.rerender(<TerminalSurface sessionId="session-1" active={false} visible={false} foreground />)
    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 4,
      data: new TextEncoder().encode('hidden')
    })
    view.rerender(<TerminalSurface sessionId="session-1" active visible foreground />)
    state.onMessage?.({
      type: 'terminal.replay-start', sessionId: 'session-1', source: 'tail',
      fromSequence: 1, throughSequence: 4, instantLineLimit: 10_000,
      availableFromSequence: 1, liveSequence: 4
    })
    state.onMessage?.({
      type: 'terminal.gap', sessionId: 'session-1', requestedFromSequence: 0,
      availableFromSequence: 0, reason: 'corruption'
    })
    state.terminalWrite.mockClear()

    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 5,
      data: new TextEncoder().encode('live-after-gap')
    })

    expect(state.terminalWrite).toHaveBeenCalledWith(
      expect.any(Uint8Array), expect.any(Function)
    )
    expect(new TextDecoder().decode(state.terminalWrite.mock.calls[0]![0] as Uint8Array))
      .toBe('live-after-gap')
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

  it('restores a serialized checkpoint before applying its Journal tail', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))

    state.onMessage?.({
      type: 'terminal.replay-start', sessionId: 'session-1', source: 'checkpoint',
      fromSequence: 8, throughSequence: 8, instantLineLimit: 10_000,
      availableFromSequence: 1, liveSequence: 8,
      checkpoint: {
        terminalSequence: 7, domainEventSequence: 2, screenEpoch: 3,
        snapshot: new TextEncoder().encode('\u001b[2Jcheckpoint screen')
      }
    })
    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 8,
      data: new TextEncoder().encode('tail')
    })

    expect(state.terminalWrite.mock.calls.slice(-2).map(([data]) =>
      typeof data === 'string' ? data : new TextDecoder().decode(data as Uint8Array)
    )).toEqual(['\u001b[2Jcheckpoint screen', 'tail'])
  })

  it('checkpoints the latest applied screen after output becomes quiet', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    vi.useFakeTimers()

    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 1,
      data: new TextEncoder().encode('first')
    })
    await act(() => vi.advanceTimersByTimeAsync(499))
    expect(state.storeTerminalCheckpoint).not.toHaveBeenCalled()
    await act(() => vi.advanceTimersByTimeAsync(1))
    expect(state.storeTerminalCheckpoint).toHaveBeenLastCalledWith(
      'session-1', 1, 0, '\u001b[2Jserialized screen'
    )

    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 2,
      data: new TextEncoder().encode('second')
    })
    await act(() => vi.advanceTimersByTimeAsync(500))
    expect(state.storeTerminalCheckpoint).toHaveBeenLastCalledWith(
      'session-1', 2, 0, '\u001b[2Jserialized screen'
    )
    expect(state.storeTerminalCheckpoint).toHaveBeenCalledTimes(2)
  })

  it('checkpoints only when the whole Scene leaves foreground, not when a sibling loses focus or viewport', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    vi.useFakeTimers()
    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 1,
      data: new TextEncoder().encode('screen')
    })

    view.rerender(<TerminalSurface sessionId="session-1" active={false} visible={false} foreground />)
    expect(state.storeTerminalCheckpoint).not.toHaveBeenCalled()
    view.rerender(<TerminalSurface sessionId="session-1" active={false} visible={false} foreground={false} />)
    expect(state.storeTerminalCheckpoint).toHaveBeenCalledTimes(1)
  })

  it('does not checkpoint unsequenced restored Shell history when its Scene moves behind', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible foreground />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    state.onMessage?.({
      type: 'terminal.restored-history', sessionId: 'session-1', blockCount: 1,
      data: new TextEncoder().encode('completed block')
    })

    view.rerender(<TerminalSurface sessionId="session-1" active={false} visible={false} foreground={false} />)
    expect(state.storeTerminalCheckpoint).not.toHaveBeenCalled()
  })

  it('creates a fresh checkpoint immediately after replay has fully applied', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
    vi.useFakeTimers()

    state.onMessage?.({
      type: 'terminal.replay-start', sessionId: 'session-1', source: 'tail',
      fromSequence: 4, throughSequence: 4, instantLineLimit: 10_000,
      availableFromSequence: 1, liveSequence: 4
    })
    state.onMessage?.({
      type: 'terminal.data', sessionId: 'session-1', sequence: 4,
      data: new TextEncoder().encode('restored')
    })
    expect(state.storeTerminalCheckpoint).not.toHaveBeenCalled()
    state.onMessage?.({
      type: 'terminal.replay-complete', sessionId: 'session-1', throughSequence: 4
    })
    await act(() => vi.advanceTimersByTimeAsync(0))

    expect(state.storeTerminalCheckpoint).toHaveBeenCalledWith(
      'session-1', 4, 0, '\u001b[2Jserialized screen'
    )
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

  it.each(['dialog', 'alertdialog'] as const)(
    'does not let delayed terminal output steal focus while a visible %s is open',
    async (role) => {
      const view = render(<TerminalSurface sessionId="session-1" active visible />)
      await waitFor(() => expect(state.onMessage).toBeTypeOf('function'))
      const dialog = document.createElement('div')
      dialog.setAttribute('role', role)
      view.container.append(dialog)
      document.body.focus()
      state.focus.mockClear()

      state.onMessage?.({ type: 'terminal.data', data: new Uint8Array([65]), sequence: 1 })

      expect(state.focus).not.toHaveBeenCalled()
      view.rerender(<TerminalSurface sessionId="session-1" active visible focusRequest={1} />)
      expect(state.focus).not.toHaveBeenCalled()
    }
  )

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

  it('falls back to archived history when the xterm buffer has no match', async () => {
    state.searchTerminalHistory.mockResolvedValue({
      matches: [
        { sequence: 3, cursor: { sequence: 3, lineIndex: 0 }, text: 'archived needle' }
      ],
      gaps: [{ segmentIndex: 1, code: 'CORRUPT_SEGMENT', message: 'damaged' }],
      hasMore: false
    })
    const onSearchResults = vi.fn()
    render(<TerminalSurface sessionId="session-1" active visible
      searchRequest={{
        query: 'needle', direction: 'next', sequence: 1,
        options: { caseSensitive: false, regex: false, wholeWord: false }
      }} onSearchResults={onSearchResults} />)
    await waitFor(() => expect(state.searchNext).toHaveBeenCalled())

    state.searchResultsListener?.({ resultIndex: 0, resultCount: 0 })

    await waitFor(() => expect(state.searchTerminalHistory).toHaveBeenCalledWith(
      'session-1', 'needle',
      { caseSensitive: false, regex: false, wholeWord: false }
    ))
    expect((await screen.findByRole('status', { name: '归档历史搜索结果' })).textContent)
      .toContain('archived needle')
    expect(screen.getByRole('status', { name: '归档历史搜索结果' }).textContent)
      .toContain('1 处历史缺口')
    expect(onSearchResults).toHaveBeenLastCalledWith({ resultIndex: 0, resultCount: 1 })
  })

  it('opens a read-only context window around a cold-history result without replacing the live terminal', async () => {
    const cursor = { sequence: 3, lineIndex: 0 }
    state.searchTerminalHistory.mockResolvedValue({
      matches: [{ sequence: 3, cursor, text: 'archived needle' }],
      gaps: [],
      hasMore: false
    })
    state.historyAroundTerminalCursor.mockResolvedValue({
      lines: [
        { sequence: 2, cursor: { sequence: 2, lineIndex: 0 }, text: 'line before' },
        { sequence: 3, cursor, text: 'archived needle' },
        { sequence: 4, cursor: { sequence: 4, lineIndex: 0 }, text: 'line after' }
      ],
      gaps: [],
      hasMore: false,
      anchorIndex: 1,
      hasMoreBefore: false,
      hasMoreAfter: false
    })
    const view = render(<TerminalSurface sessionId="session-1" active visible
      searchRequest={{
        query: 'needle', direction: 'next', sequence: 1,
        options: { caseSensitive: false, regex: false, wholeWord: false }
      }} />)
    await waitFor(() => expect(state.searchNext).toHaveBeenCalled())
    state.searchResultsListener?.({ resultIndex: 0, resultCount: 0 })

    const history = await screen.findByRole('region', { name: '终端历史记录' })
    expect(state.historyAroundTerminalCursor).toHaveBeenCalledWith('session-1', cursor, 250)
    expect(history.textContent).toContain('line before')
    expect(history.textContent).toContain('archived needle')
    expect(history.textContent).toContain('line after')
    expect(history.querySelector('[data-current-match="true"]')?.textContent)
      .toContain('archived needle')
    const liveViewport = view.container.querySelector('.terminal-surface__viewport')
    expect(liveViewport?.childElementCount).toBe(1)
    expect(liveViewport?.getAttribute('aria-hidden')).toBe('true')

    state.onData?.('blocked while reading')
    expect(state.sendTerminalInput).not.toHaveBeenCalledWith('session-1', 'blocked while reading')
    const liveBytes = new Uint8Array([76, 73, 86, 69])
    state.onMessage?.({ type: 'terminal.data', sequence: 9, data: liveBytes })
    expect(state.terminalWrite).toHaveBeenCalledWith(liveBytes, expect.any(Function))
    expect(state.acknowledgeTerminal).toHaveBeenCalledWith('session-1', 9)

    await act(async () => state.searchResultsListener?.({ resultIndex: 0, resultCount: 1 }))
    expect(screen.getByRole('region', { name: '终端历史记录' })).toBe(history)

    state.focus.mockClear()
    fireEvent.click(screen.getByRole('button', { name: '返回实时终端' }))
    expect(screen.queryByRole('region', { name: '终端历史记录' })).toBeNull()
    expect(liveViewport?.getAttribute('aria-hidden')).toBe('false')
    await waitFor(() => expect(state.focus).toHaveBeenCalled())
    state.onData?.('live input')
    expect(state.sendTerminalInput).toHaveBeenCalledWith('session-1', 'live input')

    state.searchResultsListener?.({ resultIndex: 0, resultCount: 0 })
    await act(async () => {})
    expect(screen.queryByRole('region', { name: '终端历史记录' })).toBeNull()
  })

  it('returns from the read-only history context with Escape', async () => {
    const cursor = { sequence: 3, lineIndex: 0 }
    state.searchTerminalHistory.mockResolvedValue({
      matches: [{ sequence: 3, cursor, text: 'archived needle' }], gaps: [], hasMore: false
    })
    state.historyAroundTerminalCursor.mockResolvedValue({
      lines: [{ sequence: 3, cursor, text: 'archived needle' }], gaps: [], hasMore: false,
      anchorIndex: 0, hasMoreBefore: false, hasMoreAfter: false
    })
    render(<TerminalSurface sessionId="session-1" active visible
      searchRequest={{
        query: 'needle', direction: 'next', sequence: 1,
        options: { caseSensitive: false, regex: false, wholeWord: false }
      }} />)
    await waitFor(() => expect(state.searchNext).toHaveBeenCalled())
    state.searchResultsListener?.({ resultIndex: 0, resultCount: 0 })
    await screen.findByRole('region', { name: '终端历史记录' })

    fireEvent.keyDown(window, { key: 'Escape' })

    expect(screen.queryByRole('region', { name: '终端历史记录' })).toBeNull()
  })

  it('walks multiple archived matches from newest to older with Previous', async () => {
    state.searchTerminalHistory.mockResolvedValue({
      matches: [
        { sequence: 3, cursor: { sequence: 3, lineIndex: 0 }, text: 'newest needle' },
        { sequence: 2, cursor: { sequence: 2, lineIndex: 0 }, text: 'middle needle' },
        { sequence: 1, cursor: { sequence: 1, lineIndex: 0 }, text: 'oldest needle' }
      ],
      gaps: [],
      hasMore: false
    })
    const options = { caseSensitive: false, regex: false, wholeWord: false }
    const view = render(<TerminalSurface sessionId="session-1" active visible
      searchRequest={{ query: 'needle', direction: 'previous', sequence: 1, options }} />)
    await waitFor(() => expect(state.searchPrevious).toHaveBeenCalled())

    state.searchResultsListener?.({ resultIndex: 0, resultCount: 0 })
    expect((await screen.findByRole('status', { name: '归档历史搜索结果' })).textContent)
      .toContain('newest needle')
    expect(screen.getByRole('status', { name: '归档历史搜索结果' }).textContent)
      .toContain('3/3')

    view.rerender(<TerminalSurface sessionId="session-1" active visible
      searchRequest={{ query: 'needle', direction: 'previous', sequence: 2, options }} />)
    state.searchResultsListener?.({ resultIndex: 0, resultCount: 0 })
    await waitFor(() => expect(screen.getByRole('status', { name: '归档历史搜索结果' }).textContent)
      .toContain('middle needle'))
    expect(screen.getByRole('status', { name: '归档历史搜索结果' }).textContent)
      .toContain('2/3')
  })

  it('walks multiple archived matches from oldest to newer with Next', async () => {
    state.searchTerminalHistory.mockResolvedValue({
      matches: [
        { sequence: 3, cursor: { sequence: 3, lineIndex: 0 }, text: 'newest needle' },
        { sequence: 2, cursor: { sequence: 2, lineIndex: 0 }, text: 'middle needle' },
        { sequence: 1, cursor: { sequence: 1, lineIndex: 0 }, text: 'oldest needle' }
      ],
      gaps: [],
      hasMore: false
    })
    const options = { caseSensitive: false, regex: false, wholeWord: false }
    const view = render(<TerminalSurface sessionId="session-1" active visible
      searchRequest={{ query: 'needle', direction: 'next', sequence: 1, options }} />)
    await waitFor(() => expect(state.searchNext).toHaveBeenCalled())

    state.searchResultsListener?.({ resultIndex: 0, resultCount: 0 })
    expect((await screen.findByRole('status', { name: '归档历史搜索结果' })).textContent)
      .toContain('oldest needle')
    expect(screen.getByRole('status', { name: '归档历史搜索结果' }).textContent)
      .toContain('1/3')

    view.rerender(<TerminalSurface sessionId="session-1" active visible
      searchRequest={{ query: 'needle', direction: 'next', sequence: 2, options }} />)
    state.searchResultsListener?.({ resultIndex: 0, resultCount: 0 })
    await waitFor(() => expect(screen.getByRole('status', { name: '归档历史搜索结果' }).textContent)
      .toContain('middle needle'))
    expect(screen.getByRole('status', { name: '归档历史搜索结果' }).textContent)
      .toContain('2/3')
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

  it('reattaches an existing Renderer as replay-only and stops terminal input after lifecycle downgrade', async () => {
    const view = render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.attachTerminal).toHaveBeenCalledTimes(1))
    state.onMessage?.({ type: 'terminal.spawned', pid: 123 })
    state.onData?.('before recovery')
    expect(state.sendTerminalInput).toHaveBeenCalledWith('session-1', 'before recovery')

    state.sendTerminalInput.mockClear()
    view.rerender(<TerminalSurface sessionId="session-1" active visible readOnly inputDisabled />)
    await waitFor(() => expect(state.attachTerminal).toHaveBeenCalledTimes(2))

    expect(state.attachTerminal.mock.calls[1]?.[0]).toEqual(expect.objectContaining({
      sessionId: 'session-1', readOnly: true
    }))
    state.onData?.('after recovery')
    window.dispatchEvent(new Event('matou:forward-terminal-tab'))
    expect(state.sendTerminalInput).not.toHaveBeenCalled()
  })

  it('prefers structured file-tree paths and safely quotes each path without submitting', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onMessage?.({ type: 'terminal.spawned', pid: 123 })
    const dataTransfer = {
      types: ['text/plain', 'application/x-file-tree-nodes'],
      dropEffect: 'none',
      getData: vi.fn((type: string) => {
        if (type === 'application/x-file-tree-nodes') {
          return JSON.stringify([
            { path: '/tmp/plain.txt', name: 'plain.txt', type: 'file' },
            { path: '/tmp/with space.txt', name: 'with space.txt', type: 'file' },
            { path: "/tmp/quote'and$(touch PWN).txt", name: 'special', type: 'file' }
          ])
        }
        return type === 'text/plain' ? '$(touch SHOULD_NOT_RUN)\nsecond-command' : ''
      })
    }
    const surface = document.querySelector<HTMLElement>('[data-session-id="session-1"]')!

    fireEvent.dragEnter(surface, { dataTransfer })

    expect(dataTransfer.dropEffect).toBe('copy')
    expect(screen.getByTestId('terminal-drop-overlay')).not.toBeNull()

    fireEvent.drop(surface, { dataTransfer })

    expect(state.sendTerminalInput).toHaveBeenCalledWith(
      'session-1', ` /tmp/plain.txt "/tmp/with space.txt" '/tmp/quote'\\''and$(touch PWN).txt'`
    )
    expect(state.sendTerminalInput).not.toHaveBeenCalledWith(
      'session-1', expect.stringContaining('\r')
    )
    expect(screen.queryByTestId('terminal-drop-overlay')).toBeNull()
    expect(state.focus).toHaveBeenCalled()
  })

  it('drops invalid NUL paths while preserving valid structured path order', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onMessage?.({ type: 'terminal.spawned', pid: 123 })
    const dataTransfer = {
      types: ['application/x-file-tree-nodes'], files: [], dropEffect: 'none',
      getData: vi.fn(() => JSON.stringify([
        { path: '/tmp/first.txt', type: 'file' },
        { path: '/tmp/a\0b.txt', type: 'file' },
        { path: '=ls', type: 'file' },
        { path: '/tmp/last.txt', type: 'file' }
      ]))
    }
    const surface = document.querySelector<HTMLElement>('[data-session-id="session-1"]')!

    fireEvent.drop(surface, { dataTransfer })

    expect(state.sendTerminalInput).toHaveBeenCalledWith(
      'session-1', " /tmp/first.txt '=ls' /tmp/last.txt"
    )
  })

  it('uses native file paths for Finder drops and safely quotes shell-sensitive names', async () => {
    const getPathForFile = vi.fn((file: File) => ({
      'plain.txt': '/tmp/plain.txt',
      'with space.txt': '/tmp/with space.txt',
      'special.txt': '/tmp/a$(touch PWN).txt'
    })[file.name] ?? '')
    Object.defineProperty(window, 'matouDesktop', {
      configurable: true,
      value: { getPathForFile }
    })
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onMessage?.({ type: 'terminal.spawned', pid: 123 })
    const files = [
      new File(['a'], 'plain.txt'),
      new File(['b'], 'with space.txt'),
      new File(['c'], 'special.txt')
    ]
    const dataTransfer = {
      types: ['Files'], files, dropEffect: 'none', getData: vi.fn(() => '')
    }
    const surface = document.querySelector<HTMLElement>('[data-session-id="session-1"]')!

    fireEvent.dragEnter(surface, { dataTransfer })
    expect(screen.getByTestId('terminal-drop-overlay')).not.toBeNull()
    fireEvent.drop(surface, { dataTransfer })

    expect(getPathForFile.mock.calls.map(([file]) => file)).toEqual(files)
    expect(state.sendTerminalInput).toHaveBeenCalledWith(
      'session-1', ` /tmp/plain.txt "/tmp/with space.txt" '/tmp/a$(touch PWN).txt'`
    )
  })

  it('does not treat arbitrary text/plain as a terminal path drop', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    state.onMessage?.({ type: 'terminal.spawned', pid: 123 })
    const dataTransfer = {
      types: ['text/plain'], files: [], dropEffect: 'none',
      getData: vi.fn(() => 'echo should-not-enter-terminal\nsecond-command')
    }
    const surface = document.querySelector<HTMLElement>('[data-session-id="session-1"]')!

    const accepted = fireEvent.drop(surface, { dataTransfer })

    expect(accepted).toBe(true)
    expect(state.sendTerminalInput).not.toHaveBeenCalled()
    expect(screen.queryByTestId('terminal-drop-overlay')).toBeNull()
  })

  it('keeps the reference drop overlay stable across nested drag enter and leave events', async () => {
    render(<TerminalSurface sessionId="session-1" active visible />)
    await waitFor(() => expect(state.onData).toBeTypeOf('function'))
    const dataTransfer = {
      types: ['application/x-file-tree-nodes'], dropEffect: 'none', getData: vi.fn(() => '')
    }
    const surface = document.querySelector<HTMLElement>('[data-session-id="session-1"]')!

    fireEvent.dragEnter(surface, { dataTransfer })
    fireEvent.dragEnter(surface, { dataTransfer })
    fireEvent.dragLeave(surface, { dataTransfer })
    expect(screen.getByTestId('terminal-drop-overlay')).not.toBeNull()

    fireEvent.dragLeave(surface, { dataTransfer })
    expect(screen.queryByTestId('terminal-drop-overlay')).toBeNull()
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
