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
  inputDisabled?: boolean
  onStatusChange?: (status: RuntimeStatus) => void
  onSmokeMarker?: (marker: string) => void
  onReplayComplete?: (marker: string) => void
}

export function TerminalSurface(props: TerminalSurfaceProps) {
  const {
    sessionId = 'foundation-shell', executionContextId = 'local-default',
    profile = 'shell', visible = true, inputDisabled = false,
    onStatusChange = NOOP, onSmokeMarker = NOOP, onReplayComplete = NOOP
  } = props
  const client = useRuntimeClient()
  const [pid, setPid] = useState<number | undefined>()
  const containerRef = useRef<HTMLDivElement>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const visibleRef = useRef(visible)
  const inputDisabledRef = useRef(inputDisabled)

  useEffect(() => {
    visibleRef.current = visible
    if (visible) requestAnimationFrame(() => fitRef.current?.fit())
  }, [visible])

  useEffect(() => { inputDisabledRef.current = inputDisabled }, [inputDisabled])

  useEffect(() => {
    const container = containerRef.current
    if (!container || !client) {
      onStatusChange('waiting-for-port')
      return
    }
    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: 'JetBrains Mono, SFMono-Regular, Menlo, monospace',
      fontSize: 13,
      lineHeight: 1.2,
      scrollback: 10_000,
      theme: {
        background: '#0b0e14', foreground: '#d6deeb', cursor: '#82aaff',
        selectionBackground: '#283457'
      }
    })
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminal.open(container)
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
      client.resizeTerminal(sessionId, terminal.cols, terminal.rows)
    })
    observer.observe(container)
    return () => {
      observer.disconnect()
      input.dispose()
      detach()
      fitRef.current = null
      terminal.dispose()
    }
  }, [client, executionContextId, onReplayComplete, onSmokeMarker, onStatusChange, profile, sessionId])

  return <div className="terminal-surface" ref={containerRef} aria-hidden={!visible}
    data-session-id={sessionId} {...(pid === undefined ? {} : { 'data-pid': pid })} />
}
