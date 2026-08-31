import { useEffect, useRef, useState } from 'react'

import type { RuntimeMessage } from '@matou/contracts'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { Terminal } from '@xterm/xterm'

import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { replayFromSequenceForSpawn, shouldRunReplayProbe } from './terminal-replay-policy'
import {
  DEFAULT_TERMINAL_THEME, TERMINAL_THEMES, type TerminalThemeKey
} from './terminal-themes'

const SMOKE_MARKER = '__MATOU_CHANNEL_READY__'
const NOOP = () => {}

export type RuntimeStatus =
  | 'waiting-for-port' | 'handshaking' | 'starting-session'
  | 'streaming' | 'error' | 'exited'

export interface TerminalSearchRequest {
  query: string
  options: { caseSensitive: boolean; regex: boolean; wholeWord: boolean }
  direction: 'next' | 'previous'
  sequence: number
}

interface TerminalSurfaceProps {
  sessionId?: string
  executionContextId?: string
  profile?: 'shell' | 'claude-code' | 'codex'
  visible?: boolean
  active?: boolean
  inputDisabled?: boolean
  themeKey?: TerminalThemeKey
  fontSize?: number
  onFontSizeChange?: (fontSize: number) => void
  searchRequest?: TerminalSearchRequest
  onSearchResults?: (result: { resultIndex: number; resultCount: number }) => void
  focusRequest?: number
  spawnRevision?: number
  onStatusChange?: (status: RuntimeStatus) => void
  onRuntimeError?: (message: string) => void
  onSmokeMarker?: (marker: string) => void
  onReplayComplete?: (marker: string) => void
  onOscNotification?: (oscId: number, content: string) => void
}

export function TerminalSurface(props: TerminalSurfaceProps) {
  const {
    sessionId = 'foundation-shell', executionContextId = 'local-default',
    profile = 'shell', visible = true, active = true, inputDisabled = false,
    themeKey = DEFAULT_TERMINAL_THEME, fontSize = 11, onFontSizeChange = NOOP,
    searchRequest, onSearchResults = NOOP, focusRequest = 0, spawnRevision = 0,
    onStatusChange = NOOP, onRuntimeError = NOOP, onSmokeMarker = NOOP, onReplayComplete = NOOP,
    onOscNotification = NOOP
  } = props
  const client = useRuntimeClient()
  const [pid, setPid] = useState<number | undefined>()
  const containerRef = useRef<HTMLDivElement>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const searchRef = useRef<SearchAddon | null>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const visibleRef = useRef(visible)
  const activeRef = useRef(active)
  const profileRef = useRef(profile)
  const inputDisabledRef = useRef(inputDisabled)
  const onOscNotificationRef = useRef(onOscNotification)
  const onFontSizeChangeRef = useRef(onFontSizeChange)
  const onSearchResultsRef = useRef(onSearchResults)
  const fontSizeRef = useRef(fontSize)
  const pendingInputRef = useRef('')

  // Runtime bytes can arrive during React's commit phase, before passive
  // effects run. Keep focus authority synchronized with the latest render so
  // late output from the previously active Session cannot reclaim focus.
  visibleRef.current = visible
  activeRef.current = active
  profileRef.current = profile

  useEffect(() => { pendingInputRef.current = '' }, [sessionId])
  useEffect(() => {
    client?.updateTerminalProfile(sessionId, profile)
  }, [client, profile, sessionId])

  useEffect(() => {
    if (visible) requestAnimationFrame(() => fitRef.current?.fit())
  }, [visible])

  useEffect(() => { inputDisabledRef.current = inputDisabled }, [inputDisabled])
  useEffect(() => { onOscNotificationRef.current = onOscNotification }, [onOscNotification])
  useEffect(() => { onFontSizeChangeRef.current = onFontSizeChange }, [onFontSizeChange])
  useEffect(() => { onSearchResultsRef.current = onSearchResults }, [onSearchResults])
  useEffect(() => {
    fontSizeRef.current = fontSize
    if (!terminalRef.current) return
    terminalRef.current.options.fontSize = fontSize
    requestAnimationFrame(() => fitRef.current?.fit())
  }, [fontSize])
  useEffect(() => {
    if (!terminalRef.current) return
    terminalRef.current.options.theme = TERMINAL_THEMES[themeKey]
  }, [themeKey])

  useEffect(() => {
    if (!active || !visible) return
    const focused = document.activeElement as HTMLElement | null
    if (focused?.closest('[role="dialog"], [role="alertdialog"]') ||
      (focused && /^(INPUT|TEXTAREA|SELECT)$/.test(focused.tagName) &&
        !focused.classList.contains('xterm-helper-textarea'))) return
    const frame = requestAnimationFrame(() => terminalRef.current?.focus())
    return () => cancelAnimationFrame(frame)
  }, [active, sessionId, visible])
  useEffect(() => {
    const container = containerRef.current
    if (!container || !client) {
      onStatusChange('waiting-for-port')
      return
    }
    setPid(undefined)
    const terminal = new Terminal({
      cursorBlink: true,
      cursorStyle: 'bar',
      fontFamily: "'SF Mono', 'Monaco', 'Menlo', 'Consolas', monospace",
      fontSize,
      scrollback: 10_000,
      allowProposedApi: true,
      theme: TERMINAL_THEMES[themeKey]
    })
    const fit = new FitAddon()
    const search = new SearchAddon()
    terminal.loadAddon(fit)
    terminal.loadAddon(search)
    terminal.open(container)
    terminalRef.current = terminal
    fit.fit()
    fitRef.current = fit
    searchRef.current = search
    if (activeRef.current && visibleRef.current && terminalFocusAllowed(container)) {
      requestAnimationFrame(() => terminal.focus())
    }
    const decoder = new TextDecoder()
    let observed = ''
    let replayRequested = false
    let replaying = false
    let spawned = false
    const markSpawned = () => {
      spawned = true
      if (pendingInputRef.current) {
        client.sendTerminalInput(sessionId, pendingInputRef.current)
        pendingInputRef.current = ''
      }
    }
    const sendOrBufferInput = (data: string) => {
      if (spawned) client.sendTerminalInput(sessionId, data)
      else pendingInputRef.current += data
    }
    const replayProbe = shouldRunReplayProbe(
      sessionId,
      new URLSearchParams(window.location.search).get('e2e') === '1'
    )
    onStatusChange('starting-session')

    const onMessage = (message: RuntimeMessage) => {
      if (message.type === 'terminal.spawned') {
        markSpawned()
        setPid(message.pid)
        onStatusChange('streaming')
        const replayFromSequence = replayFromSequenceForSpawn(message)
        if (replayFromSequence !== undefined && !replayRequested) {
          replayRequested = true
          client.requestTerminalReplay(sessionId, replayFromSequence)
        }
        if (replayProbe) {
          client.sendTerminalInput(sessionId, `printf '${SMOKE_MARKER}\\n'\r`)
        }
      } else if (message.type === 'terminal.data') {
        // A transferred/re-attached stream can deliver replay or live output
        // before this renderer observes its spawned notification. Receiving
        // bytes is itself proof that the input channel is attached.
        markSpawned()
        const bytes = message.data instanceof Uint8Array
          ? message.data
          : new Uint8Array(message.data)
        observed = (observed + decoder.decode(bytes, { stream: true })).slice(-8192)
        if (observed.includes(SMOKE_MARKER)) {
          onSmokeMarker(SMOKE_MARKER)
          if (!replayRequested && replayProbe) {
            replayRequested = true
            client.requestTerminalReplay(sessionId)
          }
        }
        terminal.write(bytes, () => {
          client.acknowledgeTerminal(sessionId, message.sequence)
          if (activeRef.current && visibleRef.current && terminalFocusAllowed(container)) terminal.focus()
        })
      } else if (message.type === 'terminal.restored-history') {
        const bytes = message.data instanceof Uint8Array
          ? message.data
          : new Uint8Array(message.data)
        terminal.write(bytes)
      } else if (message.type === 'terminal.exited') {
        spawned = false
        onStatusChange('exited')
      } else if (message.type === 'protocol.error') {
        onStatusChange('error')
        onRuntimeError(message.message)
      } else if (message.type === 'terminal.replay-start') {
        replaying = true
        terminal.reset()
      } else if (message.type === 'terminal.replay-resize') {
        // Resize is part of VT history: zsh and full-screen tools emit cursor
        // movements relative to the active grid. Apply it at its original
        // sequence instead of replaying every byte at today's card width.
        terminal.write('', () => terminal.resize(message.cols, message.rows))
      } else if (message.type === 'terminal.replay-reset') {
        terminal.write('', () => terminal.reset())
      } else if (message.type === 'terminal.replay-complete') {
        replaying = false
        terminal.write('', () => {
          fit.fit()
          client.resizeTerminal(sessionId, terminal.cols, terminal.rows)
          onReplayComplete(`replayed-through:${message.throughSequence}`)
        })
      }
    }
    const detach = client.attachTerminal({
      sessionId,
      executionContextId,
      profile: profileRef.current,
      cols: terminal.cols,
      rows: terminal.rows,
      spawnRevision
    }, onMessage)
    const input = terminal.onData((data) => {
      if (inputDisabledRef.current) return
      const interactionKind = classifyCompletedUserInteraction(
        data,
        profileRef.current !== 'shell'
      )
      // Deliver bytes first. Reordering the surrounding Session carousel may
      // cause a fit/resize; doing that before Enter can reset full-screen CLI
      // choices before the confirmation reaches the PTY.
      sendOrBufferInput(data)
      if (interactionKind !== undefined) {
        // Pointer activation and the focus RPC can settle a frame after xterm
        // already receives Enter. Treat the visibly active or keyboard-owned
        // card as active immediately so submitting `cc` never reorders the
        // carousel underneath the user.
        const deferOrdering = activeRef.current || container.contains(document.activeElement)
        client.recordTerminalInteraction(sessionId, interactionKind, deferOrdering)
      }
    })
    const forwardTab = () => {
      if (!activeRef.current || !visibleRef.current || inputDisabledRef.current) return
      sendOrBufferInput('\t')
    }
    window.addEventListener('matou:forward-terminal-tab', forwardTab)
    const oscHandlers = [9, 99, 777].map((oscId) => terminal.parser.registerOscHandler(oscId, (content) => {
      if (!replaying) onOscNotificationRef.current(oscId, content)
      return false
    }))
    const searchResults = search.onDidChangeResults((result) => onSearchResultsRef.current(result))
    const observer = new ResizeObserver(() => {
      if (!visibleRef.current) return
      fit.fit()
      if (terminal.cols >= 2 && terminal.cols <= 1000 && terminal.rows >= 1 && terminal.rows <= 500) {
        client.resizeTerminal(sessionId, terminal.cols, terminal.rows)
      }
    })
    observer.observe(container)
    const wheel = (event: WheelEvent) => {
      const zoom = (isMacPlatform() && (event.metaKey || event.ctrlKey)) || (!isMacPlatform() && event.ctrlKey)
      if (!zoom || event.deltaY === 0) return
      event.preventDefault()
      event.stopPropagation()
      onFontSizeChangeRef.current(Math.max(10, Math.min(24, fontSizeRef.current + (event.deltaY < 0 ? 1 : -1))))
    }
    container.addEventListener('wheel', wheel, { passive: false })
    return () => {
      container.removeEventListener('wheel', wheel)
      observer.disconnect()
      input.dispose()
      window.removeEventListener('matou:forward-terminal-tab', forwardTab)
      searchResults.dispose()
      for (const handler of oscHandlers) handler.dispose()
      detach()
      fitRef.current = null
      searchRef.current = null
      terminalRef.current = null
      terminal.dispose()
    }
  }, [client, executionContextId, onReplayComplete, onRuntimeError, onSmokeMarker, onStatusChange, sessionId, spawnRevision])

  useEffect(() => {
    if (!active || !visible || focusRequest <= 0) return
    const frame = requestAnimationFrame(() => terminalRef.current?.focus())
    return () => cancelAnimationFrame(frame)
  }, [active, focusRequest, visible])

  useEffect(() => {
    const search = searchRef.current
    if (!search || !searchRequest) return
    if (!searchRequest.query) {
      search.clearDecorations()
      onSearchResultsRef.current({ resultIndex: 0, resultCount: 0 })
      return
    }
    const options = {
      ...searchRequest.options,
      incremental: searchRequest.direction === 'next',
      decorations: {
        matchBackground: '#ffe792', matchBorder: '#d6a800', matchOverviewRuler: '#d6a800',
        activeMatchBackground: '#ff9632', activeMatchBorder: '#c86400',
        activeMatchColorOverviewRuler: '#ff9632'
      }
    }
    if (searchRequest.direction === 'previous') search.findPrevious(searchRequest.query, options)
    else search.findNext(searchRequest.query, options)
  }, [searchRequest])

  return <div className="terminal-surface" ref={containerRef} aria-hidden={!visible}
    data-session-id={sessionId} data-profile={profile} data-theme={themeKey} data-font-size={fontSize}
    {...(pid === undefined ? {} : { 'data-pid': pid })} />
}

function terminalFocusAllowed(container: HTMLElement): boolean {
  const focused = document.activeElement as HTMLElement | null
  return !focused || focused === document.body || container.contains(focused)
}

function isMacPlatform(): boolean {
  return /Mac/.test(navigator.platform ?? '') || /Mac/.test(navigator.userAgent ?? '')
}

export function classifyCompletedUserInteraction(
  data: string,
  providerMode = false
): 'submit' | 'control' | 'provider-action' | undefined {
  if (data === '\u0003' || data === '\u0004') return 'control'
  if (data === '\r' || data === '\n') return 'submit'
  // In agent CLIs Escape commits a visible user decision (cancel/reject/leave
  // the current picker). In a normal shell it is merely the prefix for many
  // navigation sequences and must not reorder the Session carousel.
  if (providerMode && data === '\u001b') return 'provider-action'
  return undefined
}
