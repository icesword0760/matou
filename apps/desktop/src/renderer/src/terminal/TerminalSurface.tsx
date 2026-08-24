import { useEffect, useRef, useState } from 'react'

import type { RuntimeMessage } from '@matou/contracts'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'

import { useRuntimeClient } from '../runtime/RuntimeProvider'

const SMOKE_MARKER = '__MATOU_CHANNEL_READY__'
const NOOP = () => {}

export type RuntimeStatus =
  | 'waiting-for-port' | 'handshaking' | 'starting-session'
  | 'streaming' | 'error' | 'exited'

interface TerminalSurfaceProps {
  sessionId?: string
  executionContextId?: string
  profile?: 'shell' | 'claude-code' | 'codex'
  visible?: boolean
  active?: boolean
  inputDisabled?: boolean
  onStatusChange?: (status: RuntimeStatus) => void
  onSmokeMarker?: (marker: string) => void
  onReplayComplete?: (marker: string) => void
}

export function TerminalSurface(props: TerminalSurfaceProps) {
  const {
    sessionId = 'foundation-shell', executionContextId = 'local-default',
    profile = 'shell', visible = true, active = true, inputDisabled = false,
    onStatusChange = NOOP, onSmokeMarker = NOOP, onReplayComplete = NOOP
  } = props
  const client = useRuntimeClient()
  const [pid, setPid] = useState<number | undefined>()
  const containerRef = useRef<HTMLDivElement>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const visibleRef = useRef(visible)
  const inputDisabledRef = useRef(inputDisabled)

  useEffect(() => {
    visibleRef.current = visible
    if (visible) requestAnimationFrame(() => fitRef.current?.fit())
  }, [visible])

  useEffect(() => { inputDisabledRef.current = inputDisabled }, [inputDisabled])

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
    const terminal = new Terminal({
      cursorBlink: true,
      cursorStyle: 'bar',
      fontFamily: "'SF Mono', 'Monaco', 'Menlo', 'Consolas', monospace",
      fontSize: 14,
      scrollback: 10_000,
      theme: {
        background: '#1B1B1B', foreground: '#FAFAFA', cursor: '#FF7809', cursorAccent: '#0d1117',
        selectionBackground: '#264f78', black: '#484f58', red: '#ff7b72', green: '#3fb950',
        yellow: '#d29922', blue: '#58a6ff', magenta: '#bc8cff', cyan: '#39d353', white: '#b1bac4',
        brightBlack: '#6e7681', brightRed: '#ffa198', brightGreen: '#56d364', brightYellow: '#e3b341',
        brightBlue: '#79c0ff', brightMagenta: '#d2a8ff', brightCyan: '#56d364', brightWhite: '#f0f6fc'
      }
    })
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminal.open(container)
    terminalRef.current = terminal
    fit.fit()
    fitRef.current = fit
    const decoder = new TextDecoder()
    let observed = ''
    let replayRequested = false
    onStatusChange('starting-session')

    const onMessage = (message: RuntimeMessage) => {
      if (message.type === 'terminal.spawned') {
        setPid(message.pid)
        onStatusChange('streaming')
        if (message.reattached && !replayRequested) {
          replayRequested = true
          client.requestTerminalReplay(sessionId)
        }
        if (new URLSearchParams(window.location.search).get('e2e') === '1') {
          client.sendTerminalInput(sessionId, `printf '${SMOKE_MARKER}\\n'\r`)
        }
      } else if (message.type === 'terminal.data') {
        const bytes = message.data instanceof Uint8Array
          ? message.data
          : new Uint8Array(message.data)
        observed = (observed + decoder.decode(bytes, { stream: true })).slice(-8192)
        if (observed.includes(SMOKE_MARKER)) {
          onSmokeMarker(SMOKE_MARKER)
          if (!replayRequested && new URLSearchParams(window.location.search).get('e2e') === '1') {
            replayRequested = true
            client.requestTerminalReplay(sessionId)
          }
        }
        terminal.write(bytes, () => client.acknowledgeTerminal(sessionId, message.sequence))
      } else if (message.type === 'terminal.exited') {
        onStatusChange('exited')
      } else if (message.type === 'protocol.error') {
        onStatusChange('error')
      } else if (message.type === 'terminal.replay-start') {
        terminal.reset()
      } else if (message.type === 'terminal.replay-complete') {
        onReplayComplete(`replayed-through:${message.throughSequence}`)
      }
    }
    const detach = client.attachTerminal({
      sessionId,
      executionContextId,
      profile,
      cols: terminal.cols,
      rows: terminal.rows
    }, onMessage)
    const input = terminal.onData((data) => {
      if (!inputDisabledRef.current) client.sendTerminalInput(sessionId, data)
    })
    const observer = new ResizeObserver(() => {
      if (!visibleRef.current) return
      fit.fit()
      if (terminal.cols >= 2 && terminal.cols <= 1000 && terminal.rows >= 1 && terminal.rows <= 500) {
        client.resizeTerminal(sessionId, terminal.cols, terminal.rows)
      }
    })
    observer.observe(container)
    return () => {
      observer.disconnect()
      input.dispose()
      detach()
      fitRef.current = null
      terminalRef.current = null
      terminal.dispose()
    }
  }, [client, executionContextId, onReplayComplete, onSmokeMarker, onStatusChange, profile, sessionId])

  return <div className="terminal-surface" ref={containerRef} aria-hidden={!visible}
    data-session-id={sessionId} {...(pid === undefined ? {} : { 'data-pid': pid })} />
}
