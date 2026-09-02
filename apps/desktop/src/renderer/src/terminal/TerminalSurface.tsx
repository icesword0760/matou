import { type DragEvent, useEffect, useRef, useState } from 'react'

import { MAX_CHECKPOINT_SNAPSHOT_BYTES, type RuntimeMessage } from '@matou/contracts'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { SerializeAddon } from '@xterm/addon-serialize'
import { Terminal } from '@xterm/xterm'

import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { ResizeCoalescer } from './resize-coalescer'
import { quoteDroppedPath } from './shell-path-quote'
import { replayFromSequenceForSpawn, shouldRunReplayProbe } from './terminal-replay-policy'
import {
  DEFAULT_TERMINAL_THEME, TERMINAL_THEMES, type TerminalThemeKey
} from './terminal-themes'

const SMOKE_MARKER = '__MATOU_CHANNEL_READY__'
const REFERENCE_FILE_TREE_MIME = 'application/x-file-tree-nodes'
const CHECKPOINT_QUIET_MS = 500
const CHECKPOINT_SCROLLBACK_LINES = 10_000
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

export type TerminalStorageFaultMessage = Extract<
  RuntimeMessage,
  { type: 'terminal.storage-fault' }
>

interface TerminalSurfaceProps {
  sessionId?: string
  executionContextId?: string
  profile?: 'shell' | 'claude-code' | 'codex'
  visible?: boolean
  active?: boolean
  foreground?: boolean
  inputDisabled?: boolean
  readOnly?: boolean
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
  onUserInput?: () => void
  onStorageFault?: (fault: TerminalStorageFaultMessage) => void
  onStorageRecovered?: () => void
}

interface ArchivedSearchView {
  text: string
  resultIndex: number
  resultCount: number
  gapCount: number
  hasMore: boolean
}

export function TerminalSurface(props: TerminalSurfaceProps) {
  const {
    sessionId = 'foundation-shell', executionContextId = 'local-default',
    profile = 'shell', visible = true, active = true, foreground = true,
    inputDisabled = false, readOnly = false,
    themeKey = DEFAULT_TERMINAL_THEME, fontSize = 11, onFontSizeChange = NOOP,
    searchRequest, onSearchResults = NOOP, focusRequest = 0, spawnRevision = 0,
    onStatusChange = NOOP, onRuntimeError = NOOP, onSmokeMarker = NOOP, onReplayComplete = NOOP,
    onOscNotification = NOOP, onUserInput = NOOP,
    onStorageFault = NOOP, onStorageRecovered = NOOP
  } = props
  const client = useRuntimeClient()
  const [pid, setPid] = useState<number | undefined>()
  const [isDragOverTerminal, setIsDragOverTerminal] = useState(false)
  const [archivedSearch, setArchivedSearch] = useState<ArchivedSearchView | undefined>()
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
  const searchRequestRef = useRef(searchRequest)
  const onUserInputRef = useRef(onUserInput)
  const onStorageFaultRef = useRef(onStorageFault)
  const onStorageRecoveredRef = useRef(onStorageRecovered)
  const fontSizeRef = useRef(fontSize)
  const pendingInputRef = useRef('')
  const sendInputRef = useRef<(data: string) => void>(NOOP)
  const checkpointNowRef = useRef<() => void>(NOOP)
  const dragOverCounterRef = useRef(0)

  // Runtime bytes can arrive during React's commit phase, before passive
  // effects run. Keep focus authority synchronized with the latest render so
  // late output from the previously active Session cannot reclaim focus.
  visibleRef.current = visible
  activeRef.current = active
  profileRef.current = profile
  onUserInputRef.current = onUserInput
  onStorageFaultRef.current = onStorageFault
  onStorageRecoveredRef.current = onStorageRecovered
  searchRequestRef.current = searchRequest

  useEffect(() => { pendingInputRef.current = '' }, [sessionId])
  useEffect(() => {
    client?.updateTerminalProfile(sessionId, profile)
  }, [client, profile, sessionId])

  useEffect(() => {
    if (visible) requestAnimationFrame(() => fitRef.current?.fit())
  }, [visible])

  useEffect(() => { inputDisabledRef.current = inputDisabled }, [inputDisabled])
  useEffect(() => {
    if (!foreground) checkpointNowRef.current()
  }, [foreground])
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
    const serialize = new SerializeAddon()
    terminal.loadAddon(fit)
    terminal.loadAddon(search)
    terminal.loadAddon(serialize)
    terminal.open(container)
    terminalRef.current = terminal
    fit.fit()
    const publishTerminalDimensions = () => {
      container.dataset.terminalCols = String(terminal.cols)
      container.dataset.terminalRows = String(terminal.rows)
    }
    publishTerminalDimensions()
    fitRef.current = fit
    searchRef.current = search
    const resizeCoalescer = new ResizeCoalescer((cols, rows) => {
      if (!readOnly) client.resizeTerminal(sessionId, cols, rows)
    })
    if (activeRef.current && visibleRef.current && terminalFocusAllowed(container)) {
      requestAnimationFrame(() => terminal.focus())
    }
    const decoder = new TextDecoder()
    let observed = ''
    let replayRequested = false
    let replaying = false
    let spawned = false
    let lastAppliedSequence = 0
    let screenEpoch = 0
    let lastCheckpointSequence = -1
    let checkpointTimer: ReturnType<typeof setTimeout> | undefined
    const clearCheckpointTimer = () => {
      if (checkpointTimer !== undefined) clearTimeout(checkpointTimer)
      checkpointTimer = undefined
    }
    const storeCheckpoint = () => {
      clearCheckpointTimer()
      if (
        readOnly || replaying || lastAppliedSequence <= 0 ||
        lastAppliedSequence <= lastCheckpointSequence
      ) return
      const snapshot = serializeCheckpoint(serialize)
      if (snapshot === undefined) return
      lastCheckpointSequence = lastAppliedSequence
      client.storeTerminalCheckpoint(
        sessionId,
        lastAppliedSequence,
        screenEpoch,
        snapshot
      )
    }
    const scheduleCheckpoint = (delay = CHECKPOINT_QUIET_MS) => {
      clearCheckpointTimer()
      if (
        readOnly || replaying || lastAppliedSequence <= 0 ||
        lastAppliedSequence <= lastCheckpointSequence
      ) return
      checkpointTimer = setTimeout(storeCheckpoint, delay)
    }
    checkpointNowRef.current = storeCheckpoint
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
    sendInputRef.current = sendOrBufferInput
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
          lastAppliedSequence = Math.max(lastAppliedSequence, message.sequence)
          client.acknowledgeTerminal(sessionId, message.sequence)
          scheduleCheckpoint()
          if (activeRef.current && visibleRef.current && terminalFocusAllowed(container)) terminal.focus()
        })
      } else if (message.type === 'terminal.restored-history') {
        const bytes = message.data instanceof Uint8Array
          ? message.data
          : new Uint8Array(message.data)
        terminal.write(bytes)
      } else if (message.type === 'terminal.exited') {
        spawned = false
        lastAppliedSequence = Math.max(lastAppliedSequence, message.sequence)
        scheduleCheckpoint()
        onStatusChange('exited')
      } else if (message.type === 'terminal.storage-fault') {
        onStorageFaultRef.current(message)
      } else if (message.type === 'terminal.storage-recovered') {
        onStorageRecoveredRef.current()
      } else if (message.type === 'protocol.error') {
        onStatusChange('error')
        onRuntimeError(message.message)
      } else if (message.type === 'terminal.replay-start') {
        clearCheckpointTimer()
        replaying = true
        terminal.reset()
        lastAppliedSequence = message.checkpoint?.terminalSequence ?? 0
        lastCheckpointSequence = message.checkpoint?.terminalSequence ?? -1
        screenEpoch = message.checkpoint?.screenEpoch ?? 0
        if (message.checkpoint) {
          const snapshot = message.checkpoint.snapshot instanceof Uint8Array
            ? message.checkpoint.snapshot
            : new Uint8Array(message.checkpoint.snapshot)
          terminal.write(snapshot)
        }
      } else if (message.type === 'terminal.replay-resize') {
        // Resize is part of VT history: zsh and full-screen tools emit cursor
        // movements relative to the active grid. Apply it at its original
        // sequence instead of replaying every byte at today's card width.
        terminal.write('', () => {
          terminal.resize(message.cols, message.rows)
          publishTerminalDimensions()
          lastAppliedSequence = Math.max(lastAppliedSequence, message.sequence)
        })
      } else if (message.type === 'terminal.replay-reset') {
        terminal.write('', () => {
          terminal.reset()
          screenEpoch = message.screenEpoch
          lastAppliedSequence = Math.max(lastAppliedSequence, message.sequence)
        })
      } else if (message.type === 'terminal.replay-complete') {
        terminal.write('', () => {
          replaying = false
          lastAppliedSequence = Math.max(lastAppliedSequence, message.throughSequence)
          fit.fit()
          publishTerminalDimensions()
          if (validTerminalDimensions(terminal.cols, terminal.rows)) {
            resizeCoalescer.offer(terminal.cols, terminal.rows)
          }
          scheduleCheckpoint(0)
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
      spawnRevision,
      readOnly
    }, onMessage)
    const input = terminal.onData((data) => {
      if (inputDisabledRef.current) return
      onUserInputRef.current()
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
    let archivedResult: {
      key: string
      requestSequence: number
      index: number
      matches: Array<{ text: string }>
      gapCount: number
      hasMore: boolean
    } | undefined
    let archiveQueryGeneration = 0
    const searchResults = search.onDidChangeResults((result) => {
      onSearchResultsRef.current(result)
      const request = searchRequestRef.current
      if (result.resultCount > 0 || !request?.query) {
        archivedResult = undefined
        setArchivedSearch(undefined)
        return
      }
      const key = JSON.stringify([request.query, request.options])
      if (archivedResult?.key === key && archivedResult.matches.length > 0) {
        if (archivedResult.requestSequence !== request.sequence) {
          archivedResult.index = request.direction === 'previous'
            ? (archivedResult.index - 1 + archivedResult.matches.length) % archivedResult.matches.length
            : (archivedResult.index + 1) % archivedResult.matches.length
          archivedResult.requestSequence = request.sequence
        }
        publishArchivedSearch(archivedResult, setArchivedSearch, onSearchResultsRef.current)
        return
      }
      const generation = ++archiveQueryGeneration
      void client.searchTerminalHistory(sessionId, request.query, request.options).then((history) => {
        if (generation !== archiveQueryGeneration) return
        const current = searchRequestRef.current
        if (!current || JSON.stringify([current.query, current.options]) !== key) return
        // Runtime history is newest-first so paging can stay bounded. The search
        // UI navigates in terminal row order: Next moves newer and Previous older.
        const matches = [...history.matches].reverse()
        archivedResult = {
          key,
          requestSequence: request.sequence,
          index: request.direction === 'previous' && matches.length > 0
            ? matches.length - 1 : 0,
          matches,
          gapCount: history.gaps.length,
          hasMore: history.hasMore
        }
        publishArchivedSearch(archivedResult, setArchivedSearch, onSearchResultsRef.current)
      }).catch(() => {
        if (generation !== archiveQueryGeneration) return
        setArchivedSearch(undefined)
      })
    })
    const observer = new ResizeObserver(() => {
      if (!visibleRef.current) return
      fit.fit()
      publishTerminalDimensions()
      if (validTerminalDimensions(terminal.cols, terminal.rows)) {
        resizeCoalescer.offer(terminal.cols, terminal.rows)
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
      clearCheckpointTimer()
      storeCheckpoint()
      checkpointNowRef.current = NOOP
      container.removeEventListener('wheel', wheel)
      observer.disconnect()
      resizeCoalescer.flush()
      resizeCoalescer.dispose()
      input.dispose()
      window.removeEventListener('matou:forward-terminal-tab', forwardTab)
      searchResults.dispose()
      for (const handler of oscHandlers) handler.dispose()
      detach()
      fitRef.current = null
      searchRef.current = null
      terminalRef.current = null
      sendInputRef.current = NOOP
      terminal.dispose()
    }
  }, [client, executionContextId, onReplayComplete, onRuntimeError, onSmokeMarker, onStatusChange, readOnly, sessionId, spawnRevision])

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
      setArchivedSearch(undefined)
      onSearchResultsRef.current({ resultIndex: 0, resultCount: 0 })
      return
    }
    setArchivedSearch(undefined)
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

  const handleTerminalDragEnter = (event: DragEvent<HTMLDivElement>) => {
    if (!isTerminalFileDrop(event.dataTransfer)) return
    event.preventDefault()
    dragOverCounterRef.current += 1
    setIsDragOverTerminal(true)
    event.dataTransfer.dropEffect = 'copy'
  }
  const handleTerminalDragOver = (event: DragEvent<HTMLDivElement>) => {
    if (!isTerminalFileDrop(event.dataTransfer)) return
    event.preventDefault()
    event.dataTransfer.dropEffect = 'copy'
  }
  const handleTerminalDragLeave = (event: DragEvent<HTMLDivElement>) => {
    if (!isTerminalFileDrop(event.dataTransfer)) return
    dragOverCounterRef.current -= 1
    if (dragOverCounterRef.current <= 0) {
      dragOverCounterRef.current = 0
      setIsDragOverTerminal(false)
    }
  }
  const handleTerminalDrop = (event: DragEvent<HTMLDivElement>) => {
    if (!isTerminalFileDrop(event.dataTransfer)) return
    event.preventDefault()
    dragOverCounterRef.current = 0
    setIsDragOverTerminal(false)

    const pathsText = terminalDropPaths(event.dataTransfer)
    if (pathsText && !inputDisabledRef.current) {
      onUserInputRef.current()
      sendInputRef.current(` ${pathsText}`)
    }
    requestAnimationFrame(() => terminalRef.current?.focus())
  }

  return <div className="terminal-surface" aria-hidden={!visible}
    data-session-id={sessionId} data-profile={profile} data-theme={themeKey} data-font-size={fontSize}
    {...(pid === undefined ? {} : { 'data-pid': pid })}
    onDragEnter={handleTerminalDragEnter} onDragOver={handleTerminalDragOver}
    onDragLeave={handleTerminalDragLeave} onDrop={handleTerminalDrop}>
    <div className="terminal-surface__viewport" ref={containerRef} />
    {archivedSearch && <div className="terminal-history-result" role="status"
      aria-label="归档历史搜索结果">
      <span className="terminal-history-result__source">归档历史</span>
      <span className="terminal-history-result__text">{archivedSearch.text}</span>
      <span className="terminal-history-result__meta">
        {archivedSearch.resultIndex + 1}/{archivedSearch.resultCount}
        {archivedSearch.hasMore ? '+' : ''}
        {archivedSearch.gapCount > 0 ? ` · ${archivedSearch.gapCount} 处历史缺口` : ''}
      </span>
    </div>}
    {isDragOverTerminal && <div className="terminal-drop-overlay" data-testid="terminal-drop-overlay" />}
  </div>
}

function publishArchivedSearch(
  result: {
    index: number
    matches: Array<{ text: string }>
    gapCount: number
    hasMore: boolean
  },
  publish: (value: ArchivedSearchView | undefined) => void,
  publishCount: (value: { resultIndex: number; resultCount: number }) => void
): void {
  const match = result.matches[result.index]
  if (!match) {
    publish(result.gapCount > 0 ? {
      text: '该范围存在不可读的历史片段',
      resultIndex: 0,
      resultCount: 0,
      gapCount: result.gapCount,
      hasMore: result.hasMore
    } : undefined)
    publishCount({ resultIndex: 0, resultCount: 0 })
    return
  }
  publish({
    text: match.text,
    resultIndex: result.index,
    resultCount: result.matches.length,
    gapCount: result.gapCount,
    hasMore: result.hasMore
  })
  publishCount({ resultIndex: result.index, resultCount: result.matches.length })
}

function serializeCheckpoint(serialize: SerializeAddon): string | undefined {
  let scrollback = CHECKPOINT_SCROLLBACK_LINES
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const snapshot = serialize.serialize({ scrollback })
    const bytes = new TextEncoder().encode(snapshot).byteLength
    if (bytes <= MAX_CHECKPOINT_SNAPSHOT_BYTES) {
      return snapshot
    }
    if (scrollback === 0) return undefined
    const ratio = MAX_CHECKPOINT_SNAPSHOT_BYTES / bytes
    scrollback = Math.max(0, Math.floor(scrollback * ratio * 0.9))
  }
  return undefined
}

function validTerminalDimensions(cols: number, rows: number): boolean {
  return cols >= 2 && cols <= 1000 && rows >= 1 && rows <= 500
}

function isTerminalFileDrop(dataTransfer: DataTransfer): boolean {
  const types = Array.from(dataTransfer.types ?? [])
  return types.includes(REFERENCE_FILE_TREE_MIME) || types.includes('Files') || dataTransfer.files.length > 0
}

function terminalDropPaths(dataTransfer: DataTransfer): string {
  if (Array.from(dataTransfer.types ?? []).includes(REFERENCE_FILE_TREE_MIME)) {
    return structuredFileTreePaths(dataTransfer.getData(REFERENCE_FILE_TREE_MIME))
      .map(quoteDroppedPath)
      .filter(Boolean)
      .join(' ')
  }
  return Array.from(dataTransfer.files ?? [])
    .map((file) => window.matouDesktop?.getPathForFile?.(file) ?? '')
    .filter(Boolean)
    .map(quoteDroppedPath)
    .filter(Boolean)
    .join(' ')
}

function structuredFileTreePaths(value: string): string[] {
  try {
    const nodes: unknown = JSON.parse(value)
    if (!Array.isArray(nodes)) return []
    return nodes.flatMap((node) => {
      if (!node || typeof node !== 'object' || !('path' in node)) return []
      const path = node.path
      return typeof path === 'string' && path.length > 0 ? [path] : []
    })
  } catch {
    return []
  }
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
