import { type DragEvent, useEffect, useRef, useState } from 'react'

import {
  MAX_CHECKPOINT_SNAPSHOT_BYTES,
  type RuntimeMessage,
  type TerminalHistoryCursor,
  type TerminalHistoryLine
} from '@matou/contracts'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { SerializeAddon } from '@xterm/addon-serialize'
import { WebglAddon } from '@xterm/addon-webgl'
import { Terminal } from '@xterm/xterm'

import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { ResizeCoalescer } from './resize-coalescer'
import { quoteDroppedPath } from './shell-path-quote'
import { foregroundTerminalModels } from './terminal-model-cache'
import { TerminalOutputCoalescer } from './terminal-output-coalescer'
import { replayFromSequenceForSpawn, shouldRunReplayProbe } from './terminal-replay-policy'
import {
  DEFAULT_TERMINAL_THEME, TERMINAL_THEMES, type TerminalThemeKey
} from './terminal-themes'

const SMOKE_MARKER = '__MATOU_CHANNEL_READY__'
const REFERENCE_FILE_TREE_MIME = 'application/x-file-tree-nodes'
const CHECKPOINT_QUIET_MS = 500
const CHECKPOINT_SCROLLBACK_LINES = 10_000
const INACTIVE_VIEWPORT_SETTLE_MS = 500
const TERMINAL_RESIZE_SETTLE_MS = 80
const RECOVERY_LAYOUT_SETTLE_WINDOW_MS = 800
const NOOP = () => {}

interface QueuedWebglActivation {
  cancelled: boolean
  activate(): void
}

const queuedWebglActivations: QueuedWebglActivation[] = []
let webglActivationFrameScheduled = false
let webglActivationMicrotaskScheduled = false

// Cold cards can enter the carousel in batches. Promote at most one fallback
// renderer to WebGL per frame so GPU setup never lands as a single scroll hitch.
function flushQueuedWebglActivation() {
  webglActivationFrameScheduled = false
  webglActivationMicrotaskScheduled = false
  let next = queuedWebglActivations.shift()
  while (next?.cancelled) next = queuedWebglActivations.shift()
  next?.activate()
  while (queuedWebglActivations[0]?.cancelled) queuedWebglActivations.shift()
  if (queuedWebglActivations.length > 0) scheduleQueuedWebglActivationFrame()
}

function scheduleQueuedWebglActivationFrame() {
  if (webglActivationFrameScheduled || webglActivationMicrotaskScheduled) return
  webglActivationFrameScheduled = true
  requestAnimationFrame(flushQueuedWebglActivation)
}

function scheduleInitialWebglActivation() {
  if (webglActivationFrameScheduled || webglActivationMicrotaskScheduled) return
  webglActivationMicrotaskScheduled = true
  queueMicrotask(flushQueuedWebglActivation)
}

function queueWebglActivation(activate: () => void) {
  const entry: QueuedWebglActivation = { cancelled: false, activate }
  queuedWebglActivations.push(entry)
  scheduleInitialWebglActivation()
  return () => { entry.cancelled = true }
}

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
  viewportMoving?: boolean
  inputDisabled?: boolean
  readOnly?: boolean
  diagnosticsProbe?: boolean
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
  onVisualReady?: () => void
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

interface HistoryContextView {
  state: 'loading' | 'ready'
  match: TerminalHistoryLine
  resultIndex: number
  resultCount: number
  lines: TerminalHistoryLine[]
  anchorIndex?: number
  gapCount: number
  hasMoreBefore: boolean
  hasMoreAfter: boolean
}

interface CachedTerminalModel {
  terminal: Terminal
  fit: FitAddon
  search: SearchAddon
  serialize: SerializeAddon
  webgl: WebglAddon | undefined
  webglAttempted: boolean
  opened: boolean
  lastAppliedSequence: number
  lastCheckpointSequence: number
  screenEpoch: number
  suspend(): void
  dispose(): void
}

export function TerminalSurface(props: TerminalSurfaceProps) {
  const {
    sessionId = 'foundation-shell', executionContextId = 'local-default',
    profile = 'shell', visible = true, active = true, foreground = true, viewportMoving = false,
    inputDisabled = false, readOnly = false, diagnosticsProbe = false,
    themeKey = DEFAULT_TERMINAL_THEME, fontSize = 11, onFontSizeChange = NOOP,
    searchRequest, onSearchResults = NOOP, focusRequest = 0, spawnRevision = 0,
    onStatusChange = NOOP, onRuntimeError = NOOP, onSmokeMarker = NOOP, onReplayComplete = NOOP,
    onVisualReady = NOOP,
    onOscNotification = NOOP, onUserInput = NOOP,
    onStorageFault = NOOP, onStorageRecovered = NOOP
  } = props
  const client = useRuntimeClient()
  const [pid, setPid] = useState<number | undefined>()
  const [isDragOverTerminal, setIsDragOverTerminal] = useState(false)
  const [archivedSearch, setArchivedSearch] = useState<ArchivedSearchView | undefined>()
  const [historyContext, setHistoryContext] = useState<HistoryContextView | undefined>()
  const containerRef = useRef<HTMLDivElement>(null)
  const e2eRowsRef = useRef<HTMLDivElement>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const searchRef = useRef<SearchAddon | null>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const visibleRef = useRef(visible)
  const activeRef = useRef(active)
  const viewportMovingRef = useRef(viewportMoving)
  const profileRef = useRef(profile)
  const inputDisabledRef = useRef(inputDisabled)
  const onOscNotificationRef = useRef(onOscNotification)
  const onFontSizeChangeRef = useRef(onFontSizeChange)
  const onSearchResultsRef = useRef(onSearchResults)
  const onVisualReadyRef = useRef(onVisualReady)
  const searchRequestRef = useRef(searchRequest)
  const onUserInputRef = useRef(onUserInput)
  const onStorageFaultRef = useRef(onStorageFault)
  const onStorageRecoveredRef = useRef(onStorageRecovered)
  const fontSizeRef = useRef(fontSize)
  const pendingInputRef = useRef('')
  const sendInputRef = useRef<(data: string) => void>(NOOP)
  const checkpointNowRef = useRef<() => void>(NOOP)
  const flushOutputRef = useRef<() => void>(NOOP)
  const resumeVisualRef = useRef<() => void>(NOOP)
  const activateWebglRef = useRef<() => void>(NOOP)
  const scheduleWebglRef = useRef<() => void>(NOOP)
  const cancelWebglRef = useRef<() => void>(NOOP)
  const dragOverCounterRef = useRef(0)
  const historyModeRef = useRef(false)
  const historyRequestGenerationRef = useRef(0)
  const historyDismissedSearchSequenceRef = useRef<number | undefined>(undefined)
  const historyOpenedSearchSequenceRef = useRef<number | undefined>(undefined)
  const historyAnchorRef = useRef<HTMLDivElement>(null)
  const interactiveRef = useRef(active && visible)
  const activationReadyPendingRef = useRef(false)
  const activationResizeObservedRef = useRef(false)
  const activationReadyFrameRef = useRef<number | undefined>(undefined)
  const reportActivatedVisualReady = () => {
    if (
      !activationReadyPendingRef.current || !activeRef.current ||
      !visibleRef.current || viewportMovingRef.current
    ) return
    const terminal = terminalRef.current
    if (!terminal) return
    terminal.refresh(0, Math.max(0, terminal.rows - 1))
    if (activationReadyFrameRef.current !== undefined) {
      cancelAnimationFrame(activationReadyFrameRef.current)
    }
    activationReadyFrameRef.current = requestAnimationFrame(() => {
      activationReadyFrameRef.current = undefined
      if (
        !activationReadyPendingRef.current || !activeRef.current ||
        !visibleRef.current || viewportMovingRef.current
      ) return
      activationReadyPendingRef.current = false
      activationResizeObservedRef.current = false
      onVisualReadyRef.current()
    })
  }

  // Runtime bytes can arrive during React's commit phase, before passive
  // effects run. Keep focus authority synchronized with the latest render so
  // late output from the previously active Session cannot reclaim focus.
  visibleRef.current = visible
  activeRef.current = active
  viewportMovingRef.current = viewportMoving
  profileRef.current = profile
  onUserInputRef.current = onUserInput
  onVisualReadyRef.current = onVisualReady
  onStorageFaultRef.current = onStorageFault
  onStorageRecoveredRef.current = onStorageRecovered
  searchRequestRef.current = searchRequest
  inputDisabledRef.current = inputDisabled || historyContext !== undefined

  useEffect(() => {
    pendingInputRef.current = ''
    historyRequestGenerationRef.current += 1
    historyModeRef.current = false
    historyDismissedSearchSequenceRef.current = undefined
    historyOpenedSearchSequenceRef.current = undefined
    setHistoryContext(undefined)
    setArchivedSearch(undefined)
  }, [sessionId])
  useEffect(() => {
    client?.updateTerminalProfile(sessionId, profile)
  }, [client, profile, sessionId])

  useEffect(() => {
    const becameInteractive = active && visible && !interactiveRef.current
    interactiveRef.current = active && visible
    if (becameInteractive) activationReadyPendingRef.current = true
    if (!active || !visible) {
      activationReadyPendingRef.current = false
      activationResizeObservedRef.current = false
      if (activationReadyFrameRef.current !== undefined) {
        cancelAnimationFrame(activationReadyFrameRef.current)
        activationReadyFrameRef.current = undefined
      }
    }
    let activationTimer: ReturnType<typeof setTimeout> | undefined
    if (visible || active) {
      if (active) {
        cancelWebglRef.current()
        activateWebglRef.current()
      } else scheduleWebglRef.current()
      const catchupTimer = active || viewportMoving
        ? undefined
        : setTimeout(() => resumeVisualRef.current(), INACTIVE_VIEWPORT_SETTLE_MS)
      if (active && !viewportMoving) resumeVisualRef.current()
      if (activationReadyPendingRef.current && !viewportMoving) {
        // A wide, content-heavy xterm can take a full layout turn to reflow
        // after its card becomes focused. Keep the card's loading cover until
        // the settled grid has been fitted and a fresh frame is requested.
        activationResizeObservedRef.current = false
        activationTimer = setTimeout(() => {
          if (!activationResizeObservedRef.current) reportActivatedVisualReady()
        }, TERMINAL_RESIZE_SETTLE_MS + 20)
      }
      flushOutputRef.current()
      return () => {
        if (catchupTimer !== undefined) clearTimeout(catchupTimer)
        if (activationTimer !== undefined) clearTimeout(activationTimer)
      }
    }
    return undefined
  }, [active, viewportMoving, visible])

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
    if (historyContext?.state !== 'ready') return
    const frame = requestAnimationFrame(() => {
      historyAnchorRef.current?.scrollIntoView?.({ block: 'center' })
    })
    return () => cancelAnimationFrame(frame)
  }, [historyContext])

  const exitHistoryView = () => {
    historyRequestGenerationRef.current += 1
    historyModeRef.current = false
    historyOpenedSearchSequenceRef.current = undefined
    historyDismissedSearchSequenceRef.current = searchRequestRef.current?.sequence
    setHistoryContext(undefined)
    setArchivedSearch(undefined)
    requestAnimationFrame(() => {
      fitRef.current?.fit()
      if (activeRef.current && visibleRef.current) terminalRef.current?.focus()
    })
  }

  useEffect(() => {
    if (!historyContext) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      event.stopPropagation()
      exitHistoryView()
    }
    window.addEventListener('keydown', onKeyDown, true)
    return () => window.removeEventListener('keydown', onKeyDown, true)
  }, [historyContext])

  useEffect(() => {
    if (!active || !visible) return
    const container = containerRef.current
    if (!container || !terminalFocusAllowed(container)) return
    const focused = document.activeElement as HTMLElement | null
    if (focused?.closest('[role="dialog"], [role="alertdialog"]') ||
      (focused && /^(INPUT|TEXTAREA|SELECT)$/.test(focused.tagName) &&
        !focused.classList.contains('xterm-helper-textarea'))) return
    const frame = requestAnimationFrame(() => {
      if (terminalFocusAllowed(container)) terminalRef.current?.focus()
    })
    return () => cancelAnimationFrame(frame)
  }, [active, sessionId, visible])
  useEffect(() => {
    const container = containerRef.current
    if (!container || !client) {
      onStatusChange('waiting-for-port')
      return
    }
    setPid(undefined)
    const model = foregroundTerminalModels.acquire(sessionId, () => {
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
      const model: CachedTerminalModel = {
        terminal, fit, search, serialize, webgl: undefined, webglAttempted: false, opened: false,
        lastAppliedSequence: 0, lastCheckpointSequence: -1, screenEpoch: 0,
        suspend: () => {
          model.webgl?.dispose()
          model.webgl = undefined
          model.webglAttempted = false
          terminal.element?.remove()
        },
        dispose: () => terminal.dispose()
      }
      return model
    }) as CachedTerminalModel
    const { terminal, fit, search, serialize } = model
    const e2eRows = new URLSearchParams(window.location.search).get('e2e') === '1'
      ? e2eRowsRef.current
      : null
    const reusedTerminalModel = model.opened
    let webglScheduled = false
    let cancelQueuedWebgl = NOOP
    const cancelWebgl = () => {
      if (!webglScheduled) return
      webglScheduled = false
      cancelQueuedWebgl()
      cancelQueuedWebgl = NOOP
    }
    const activateWebgl = () => {
      cancelWebgl()
      if (model.webglAttempted) return
      model.webglAttempted = true
      try {
        const webgl = new WebglAddon()
        model.webgl = webgl
        webgl.onContextLoss(() => {
          webgl.dispose()
          if (model.webgl === webgl) model.webgl = undefined
          e2eRows?.classList.remove('xterm-rows')
        })
        terminal.loadAddon(webgl)
        e2eRows?.classList.add('xterm-rows')
      } catch {
        // xterm keeps its built-in renderer when WebGL is unavailable. This is
        // expected on remote desktops and after Chromium exhausts GPU contexts.
        model.webgl = undefined
      }
    }
    const scheduleWebgl = () => {
      if (model.webglAttempted || webglScheduled) return
      webglScheduled = true
      cancelQueuedWebgl = queueWebglActivation(() => {
        webglScheduled = false
        cancelQueuedWebgl = NOOP
        activateWebgl()
      })
    }
    activateWebglRef.current = activateWebgl
    scheduleWebglRef.current = scheduleWebgl
    cancelWebglRef.current = cancelWebgl
    terminal.options.fontSize = fontSize
    terminal.options.theme = TERMINAL_THEMES[themeKey]
    if (!model.opened) {
      terminal.open(container)
      model.opened = true
    } else if (terminal.element) {
      container.appendChild(terminal.element)
      if (model.webgl) e2eRows?.classList.add('xterm-rows')
    }
    activeRef.current ? activateWebgl() : scheduleWebgl()
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
      requestAnimationFrame(() => {
        if (terminalFocusAllowed(container)) terminal.focus()
      })
    }
    let replayRequested = false
    let replaying = false
    let preserveExistingModelForReplay = false
    let spawned = false
    let visualCatchupPending = false
    let visualCatchupRequested = false
    let surfaceDisposed = false
    let visualReadyReported = false
    let terminalContentApplied = false
    let visualReplayPending = false
    let recoveryLayoutResizeDeadline = 0
    let checkpointTimer: ReturnType<typeof setTimeout> | undefined
    let e2eRowsTimer: ReturnType<typeof setTimeout> | undefined
    const reportVisualReady = () => {
      if (visualReadyReported || surfaceDisposed) return
      visualReadyReported = true
      requestAnimationFrame(() => {
        if (!surfaceDisposed) onVisualReadyRef.current()
      })
    }
    const rendered = terminal.onRender(() => {
      if (
        terminalContentApplied && !visualReplayPending &&
        terminalViewportHasContent(terminal)
      ) reportVisualReady()
    })
    const awaitRenderedTerminalFrame = () => {
      terminalContentApplied = true
      terminal.refresh(0, Math.max(0, terminal.rows - 1))
    }
    if (reusedTerminalModel && model.lastAppliedSequence > 0) awaitRenderedTerminalFrame()
    const publishE2eRows = () => {
      e2eRowsTimer = undefined
      if (!e2eRows?.classList.contains('xterm-rows')) return
      const buffer = terminal.buffer.active
      const lines: string[] = []
      const first = Math.max(0, buffer.length - 10_000)
      for (let index = first; index < buffer.length; index += 1) {
        lines.push(buffer.getLine(index)?.translateToString(true) ?? '')
      }
      const row = e2eRows.firstElementChild
      if (row) row.textContent = lines.join('\n')
    }
    const scheduleE2eRows = () => {
      if (!e2eRows?.classList.contains('xterm-rows')) return
      if (e2eRowsTimer !== undefined) clearTimeout(e2eRowsTimer)
      // Publish only after output becomes quiet. This is an observation of the
      // parsed xterm buffer for real Electron acceptance tests; debouncing keeps
      // sustained-output performance measurements representative.
      e2eRowsTimer = setTimeout(publishE2eRows, 80)
    }
    scheduleE2eRows()
    const clearCheckpointTimer = () => {
      if (checkpointTimer !== undefined) clearTimeout(checkpointTimer)
      checkpointTimer = undefined
    }
    const storeCheckpoint = () => {
      clearCheckpointTimer()
      if (
        readOnly || replaying || model.lastAppliedSequence <= 0 ||
        model.lastAppliedSequence <= model.lastCheckpointSequence
      ) return
      const snapshot = serializeCheckpoint(serialize)
      if (snapshot === undefined) return
      model.lastCheckpointSequence = model.lastAppliedSequence
      client.storeTerminalCheckpoint(
        sessionId,
        model.lastAppliedSequence,
        model.screenEpoch,
        terminal.cols,
        terminal.rows,
        snapshot
      )
    }
    const scheduleCheckpoint = (delay = CHECKPOINT_QUIET_MS) => {
      clearCheckpointTimer()
      if (
        readOnly || replaying || model.lastAppliedSequence <= 0 ||
        model.lastAppliedSequence <= model.lastCheckpointSequence
      ) return
      checkpointTimer = setTimeout(storeCheckpoint, delay)
    }
    checkpointNowRef.current = storeCheckpoint
    const writeLiveOutput = (bytes: Uint8Array, sequence: number) => {
      terminal.write(bytes, () => {
        model.lastAppliedSequence = Math.max(model.lastAppliedSequence, sequence)
        client.acknowledgeTerminal(sessionId, sequence)
        if (surfaceDisposed) return
        awaitRenderedTerminalFrame()
        scheduleE2eRows()
        scheduleCheckpoint()
        if (!historyModeRef.current && activeRef.current && visibleRef.current && terminalFocusAllowed(container)) {
          terminal.focus()
        }
      })
    }
    const output = new TerminalOutputCoalescer(writeLiveOutput)
    flushOutputRef.current = () => output.flush()
    const requestVisualCatchup = () => {
      if (!visualCatchupPending || visualCatchupRequested || replaying) return
      visualCatchupRequested = true
      clearCheckpointTimer()
      // Rebuild from Runtime's bounded tail rather than parsing an unbounded
      // offscreen byte stream. Runtime and its Journal stay live throughout;
      // only hidden xterm painting is suspended.
      const fromSequence = model.lastAppliedSequence > 0
        ? model.lastAppliedSequence + 1
        : 0
      preserveExistingModelForReplay = fromSequence > 0
      client.requestTerminalReplay(sessionId, fromSequence, preserveExistingModelForReplay)
    }
    resumeVisualRef.current = requestVisualCatchup
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
      new URLSearchParams(window.location.search).get('e2e') === '1',
      diagnosticsProbe
    )
    onStatusChange('starting-session')

    const onMessage = (message: RuntimeMessage) => {
      if (message.type === 'terminal.spawned') {
        markSpawned()
        setPid(message.pid)
        onStatusChange('streaming')
        const replayFromSequence = replayFromSequenceForSpawn(
          message, reusedTerminalModel, profileRef.current, model.lastAppliedSequence
        )
        if (replayFromSequence !== undefined && !replayRequested) {
          visualReplayPending = true
          if (visibleRef.current) {
            preserveExistingModelForReplay = reusedTerminalModel && replayFromSequence > 0
            replayRequested = true
            client.requestTerminalReplay(
              sessionId,
              replayFromSequence,
              preserveExistingModelForReplay
            )
          } else {
            visualCatchupPending = true
          }
        }
        if (replayProbe) {
          onSmokeMarker(SMOKE_MARKER)
          if (!replayRequested) {
            visualReplayPending = true
            replayRequested = true
            client.requestTerminalReplay(sessionId)
          }
        }
      } else if (message.type === 'terminal.data') {
        // A transferred/re-attached stream can deliver replay or live output
        // before this renderer observes its spawned notification. Receiving
        // bytes is itself proof that the input channel is attached.
        markSpawned()
        const bytes = message.data instanceof Uint8Array
          ? message.data
          : new Uint8Array(message.data)
        if ((!visibleRef.current && !activeRef.current || visualCatchupPending) && !replaying) {
          visualCatchupPending = true
          client.acknowledgeTerminal(sessionId, message.sequence)
        } else if (visualCatchupRequested && !replaying) {
          // The replay request has not detached the live stream yet. These
          // bytes are included by the replay watermark, so acknowledge them
          // without briefly painting content that replay-start will reset.
          visualCatchupPending = true
          client.acknowledgeTerminal(sessionId, message.sequence)
        } else {
          output.offer(bytes, message.sequence, true)
        }
      } else if (message.type === 'terminal.restored-history') {
        if (reusedTerminalModel) return
        const bytes = message.data instanceof Uint8Array
          ? message.data
          : new Uint8Array(message.data)
        terminal.write(bytes, () => {
          awaitRenderedTerminalFrame()
          scheduleE2eRows()
        })
      } else if (message.type === 'terminal.exited') {
        spawned = false
        model.lastAppliedSequence = Math.max(model.lastAppliedSequence, message.sequence)
        scheduleCheckpoint()
        onStatusChange('exited')
      } else if (message.type === 'terminal.storage-fault') {
        onStorageFaultRef.current(message)
      } else if (message.type === 'terminal.storage-recovered') {
        onStorageRecoveredRef.current()
      } else if (message.type === 'terminal.gap') {
        // Runtime always reattaches the live PTY after a replay failure. End
        // the visual replay state as well; otherwise every later live frame is
        // acknowledged but intentionally withheld from xterm forever.
        replaying = false
        visualReplayPending = false
        visualCatchupPending = false
        visualCatchupRequested = false
        preserveExistingModelForReplay = false
        terminal.write(
          message.reason === 'corruption'
            ? '\r\n[部分终端历史损坏，已继续显示实时输出]\r\n'
            : '\r\n[较早的终端历史已清理，已继续显示实时输出]\r\n',
          () => {
            awaitRenderedTerminalFrame()
            scheduleE2eRows()
          }
        )
      } else if (message.type === 'protocol.error') {
        visualCatchupRequested = false
        onStatusChange('error')
        onRuntimeError(message.message)
      } else if (message.type === 'terminal.replay-start') {
        clearCheckpointTimer()
        replaying = true
        visualReplayPending = true
        terminalContentApplied = false
        visualCatchupPending = false
        const preservingExistingModel = preserveExistingModelForReplay && !message.checkpoint
        if (!preservingExistingModel) {
          terminal.reset()
          model.lastAppliedSequence = message.checkpoint?.terminalSequence ?? 0
          model.lastCheckpointSequence = message.checkpoint?.terminalSequence ?? -1
          model.screenEpoch = message.checkpoint?.screenEpoch ?? 0
        }
        preserveExistingModelForReplay = false
        if (message.checkpoint) {
          // SerializeAddon snapshots contain cursor movement and wrapped rows
          // for the grid they were captured in. Restore that exact grid before
          // applying the VT stream, then fit to today's card after replay.
          terminal.resize(message.checkpoint.cols, message.checkpoint.rows)
          publishTerminalDimensions()
          const snapshot = message.checkpoint.snapshot instanceof Uint8Array
            ? message.checkpoint.snapshot
            : new Uint8Array(message.checkpoint.snapshot)
          terminal.write(snapshot, () => {
            awaitRenderedTerminalFrame()
            scheduleE2eRows()
          })
        }
      } else if (message.type === 'terminal.replay-resize') {
        // Resize is part of VT history: zsh and full-screen tools emit cursor
        // movements relative to the active grid. Apply it at its original
        // sequence instead of replaying every byte at today's card width.
        terminal.write('', () => {
          terminal.resize(message.cols, message.rows)
          scheduleE2eRows()
          publishTerminalDimensions()
          model.lastAppliedSequence = Math.max(model.lastAppliedSequence, message.sequence)
        })
      } else if (message.type === 'terminal.replay-reset') {
        terminal.write('', () => {
          terminal.reset()
          scheduleE2eRows()
          model.screenEpoch = message.screenEpoch
          model.lastAppliedSequence = Math.max(model.lastAppliedSequence, message.sequence)
        })
      } else if (message.type === 'terminal.replay-complete') {
        terminal.write('', () => {
          replaying = false
          visualReplayPending = false
          visualCatchupRequested = false
          model.lastAppliedSequence = Math.max(model.lastAppliedSequence, message.throughSequence)
          fit.fit()
          // The carousel can still be completing its responsive flex transition
          // when an inactive Session finishes replay. Permit exactly one later
          // settled resize so the restored grid fills that card, without making
          // ordinary inactive hover previews resize the live PTY repeatedly.
          recoveryLayoutResizeDeadline = performance.now() + RECOVERY_LAYOUT_SETTLE_WINDOW_MS
          publishTerminalDimensions()
          if (validTerminalDimensions(terminal.cols, terminal.rows)) {
            resizeCoalescer.offer(terminal.cols, terminal.rows)
          }
          scheduleCheckpoint(0)
          onReplayComplete(`replayed-through:${message.throughSequence}`)
          awaitRenderedTerminalFrame()
          scheduleE2eRows()
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
      matches: TerminalHistoryLine[]
      gapCount: number
      hasMore: boolean
    } | undefined
    let archiveQueryGeneration = 0
    const openHistoryContext = (result: NonNullable<typeof archivedResult>) => {
      publishArchivedSearch(result, setArchivedSearch, onSearchResultsRef.current)
      if (historyDismissedSearchSequenceRef.current === searchRequestRef.current?.sequence) return
      const match = result.matches[result.index]
      if (!match) return
      const generation = ++historyRequestGenerationRef.current
      historyModeRef.current = true
      historyOpenedSearchSequenceRef.current = searchRequestRef.current?.sequence
      setHistoryContext({
        state: 'loading', match,
        resultIndex: result.index,
        resultCount: result.matches.length,
        lines: [], gapCount: result.gapCount,
        hasMoreBefore: false, hasMoreAfter: false
      })
      void client.historyAroundTerminalCursor(sessionId, match.cursor, 250).then((page) => {
        if (generation !== historyRequestGenerationRef.current) return
        if (page.anchorIndex === undefined || !page.lines[page.anchorIndex] ||
          !sameHistoryCursor(page.lines[page.anchorIndex]!.cursor, match.cursor)) {
          historyModeRef.current = false
          historyOpenedSearchSequenceRef.current = undefined
          setHistoryContext(undefined)
          return
        }
        setHistoryContext({
          state: 'ready', match,
          resultIndex: result.index,
          resultCount: result.matches.length,
          lines: page.lines,
          anchorIndex: page.anchorIndex,
          gapCount: page.gaps.length,
          hasMoreBefore: page.hasMoreBefore === true,
          hasMoreAfter: page.hasMoreAfter === true
        })
      }).catch(() => {
        if (generation !== historyRequestGenerationRef.current) return
        historyModeRef.current = false
        historyOpenedSearchSequenceRef.current = undefined
        setHistoryContext(undefined)
      })
    }
    const searchResults = search.onDidChangeResults((result) => {
      const request = searchRequestRef.current
      if (historyModeRef.current && historyOpenedSearchSequenceRef.current === request?.sequence) return
      onSearchResultsRef.current(result)
      if (result.resultCount > 0 || !request?.query) {
        archivedResult = undefined
        historyRequestGenerationRef.current += 1
        historyModeRef.current = false
        historyOpenedSearchSequenceRef.current = undefined
        setHistoryContext(undefined)
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
        openHistoryContext(archivedResult)
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
        openHistoryContext(archivedResult)
      }).catch(() => {
        if (generation !== archiveQueryGeneration) return
        historyRequestGenerationRef.current += 1
        historyModeRef.current = false
        historyOpenedSearchSequenceRef.current = undefined
        setHistoryContext(undefined)
        setArchivedSearch(undefined)
      })
    })
    let resizeSettleTimer: ReturnType<typeof setTimeout> | undefined
    const settleTerminalResize = () => {
      resizeSettleTimer = undefined
      // Hover previews animate between compact and expanded widths. Keeping
      // that animation visual avoids making shells redraw their prompt into
      // durable scrollback after the card has already lost input focus. The
      // final focused grid is published when that card becomes active again.
      const settlingRecoveredLayout = performance.now() <= recoveryLayoutResizeDeadline
      if (!visibleRef.current || (!activeRef.current && !settlingRecoveredLayout)) return
      recoveryLayoutResizeDeadline = 0
      fit.fit()
      publishTerminalDimensions()
      if (validTerminalDimensions(terminal.cols, terminal.rows)) {
        resizeCoalescer.offer(terminal.cols, terminal.rows)
      }
      if (activationReadyPendingRef.current && !viewportMovingRef.current) {
        reportActivatedVisualReady()
      }
    }
    const observer = new ResizeObserver(() => {
      if (!visibleRef.current) return
      if (activationReadyPendingRef.current && activeRef.current) {
        activationResizeObservedRef.current = true
      }
      if (resizeSettleTimer !== undefined) clearTimeout(resizeSettleTimer)
      resizeSettleTimer = setTimeout(settleTerminalResize, TERMINAL_RESIZE_SETTLE_MS)
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
      historyRequestGenerationRef.current += 1
      historyModeRef.current = false
      historyOpenedSearchSequenceRef.current = undefined
      surfaceDisposed = true
      if (activationReadyFrameRef.current !== undefined) {
        cancelAnimationFrame(activationReadyFrameRef.current)
        activationReadyFrameRef.current = undefined
      }
      clearCheckpointTimer()
      if (e2eRowsTimer !== undefined) clearTimeout(e2eRowsTimer)
      output.dispose()
      cancelWebgl()
      activateWebglRef.current = NOOP
      scheduleWebglRef.current = NOOP
      cancelWebglRef.current = NOOP
      flushOutputRef.current = NOOP
      resumeVisualRef.current = NOOP
      storeCheckpoint()
      checkpointNowRef.current = NOOP
      container.removeEventListener('wheel', wheel)
      observer.disconnect()
      if (resizeSettleTimer !== undefined) clearTimeout(resizeSettleTimer)
      resizeCoalescer.flush()
      resizeCoalescer.dispose()
      input.dispose()
      rendered.dispose()
      window.removeEventListener('matou:forward-terminal-tab', forwardTab)
      searchResults.dispose()
      for (const handler of oscHandlers) handler.dispose()
      detach()
      fitRef.current = null
      searchRef.current = null
      terminalRef.current = null
      sendInputRef.current = NOOP
      const retained = foregroundTerminalModels.release(sessionId)
      if (retained) terminal.element?.remove()
    }
  }, [client, diagnosticsProbe, executionContextId, onReplayComplete, onRuntimeError, onSmokeMarker, onStatusChange, readOnly, sessionId, spawnRevision])

  useEffect(() => {
    if (!active || !visible || focusRequest <= 0) return
    const container = containerRef.current
    if (!container) return
    const frame = requestAnimationFrame(() => {
      if (!hostNavigationTerminalFocusSuppressed() && !terminalFocusBlockedByDialog()) {
        terminalRef.current?.focus()
      }
    })
    return () => cancelAnimationFrame(frame)
  }, [active, focusRequest, visible])

  useEffect(() => {
    const search = searchRef.current
    if (!search || !searchRequest) return
    if (!searchRequest.query) {
      search.clearDecorations()
      historyRequestGenerationRef.current += 1
      historyModeRef.current = false
      historyOpenedSearchSequenceRef.current = undefined
      setHistoryContext(undefined)
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
    <div className="terminal-surface__viewport" ref={containerRef}
      aria-hidden={historyContext !== undefined} />
    <div className="e2e-terminal-observer" ref={e2eRowsRef} aria-hidden="true"><div /></div>
    {historyContext && <TerminalHistoryContextView view={historyContext}
      anchorRef={historyAnchorRef} onClose={exitHistoryView} />}
    {archivedSearch && !historyContext && <div className="terminal-history-result" role="status"
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

function TerminalHistoryContextView(props: {
  view: HistoryContextView
  anchorRef: React.RefObject<HTMLDivElement | null>
  onClose: () => void
}) {
  const { view, anchorRef, onClose } = props
  return <section className="terminal-history-context" role="region" aria-label="终端历史记录">
    <header className="terminal-history-context__header">
      <div className="terminal-history-context__title">
        <strong>历史记录</strong>
        <span>只读</span>
        <span>{view.resultIndex + 1}/{view.resultCount}</span>
      </div>
      <button type="button" className="terminal-history-context__return" onClick={onClose}>
        返回实时终端
      </button>
    </header>
    {view.state === 'loading'
      ? <div className="terminal-history-context__loading" role="status">正在读取历史上下文…</div>
      : <div className="terminal-history-context__body">
        {view.hasMoreBefore && <div className="terminal-history-context__boundary">上方还有更早记录</div>}
        {view.lines.map((line, index) => {
          const current = index === view.anchorIndex
          return <div key={`${line.cursor.sequence}:${line.cursor.lineIndex}`}
            ref={current ? anchorRef : undefined}
            className={`terminal-history-context__line${current ? ' is-current' : ''}`}
            data-current-match={current ? 'true' : undefined}>
            <span className="terminal-history-context__line-number">{index + 1}</span>
            <span className="terminal-history-context__line-text">{line.text || ' '}</span>
          </div>
        })}
        {view.hasMoreAfter && <div className="terminal-history-context__boundary">下方还有更新记录</div>}
      </div>}
    <footer className="terminal-history-context__footer">
      <span>命中行前后各最多 250 行</span>
      {view.gapCount > 0 && <span>{view.gapCount} 处历史缺口</span>}
      <span>Esc 返回</span>
    </footer>
  </section>
}

function sameHistoryCursor(left: TerminalHistoryCursor, right: TerminalHistoryCursor): boolean {
  return left.sequence === right.sequence && left.lineIndex === right.lineIndex
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

function terminalViewportHasContent(terminal: Terminal): boolean {
  const buffer = terminal.buffer.active
  const firstLine = Math.max(0, buffer.viewportY)
  const lastLine = Math.min(buffer.length, firstLine + Math.max(1, terminal.rows))
  for (let index = firstLine; index < lastLine; index += 1) {
    if (/\S/u.test(buffer.getLine(index)?.translateToString(true) ?? '')) return true
  }
  return false
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
  if (hostNavigationTerminalFocusSuppressed() || terminalFocusBlockedByDialog()) return false
  const focused = document.activeElement as HTMLElement | null
  return !focused || focused === document.body || container.contains(focused)
}

function hostNavigationTerminalFocusSuppressed(): boolean {
  return document.documentElement.dataset.hostNavigationTerminalFocus === 'suppressed'
}

function terminalFocusBlockedByDialog(): boolean {
  return Array.from(document.querySelectorAll<HTMLElement>(
    '[role="dialog"], [role="alertdialog"]'
  )).some((dialog) => !dialog.hidden && !dialog.closest('[hidden], [inert], [aria-hidden="true"]'))
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
