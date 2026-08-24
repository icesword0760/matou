import { useEffect, useRef } from 'react'

import type { RuntimeMessage } from '@matou/contracts'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'

import { useRuntimeClient } from '../runtime/RuntimeProvider'

const SESSION_ID = 'foundation-shell'
const SMOKE_MARKER = '__MATOU_CHANNEL_READY__'

export type RuntimeStatus =
  | 'waiting-for-port' | 'handshaking' | 'starting-session'
  | 'streaming' | 'error' | 'exited'

interface TerminalSurfaceProps {
  onStatusChange: (status: RuntimeStatus) => void
  onSmokeMarker: (marker: string) => void
  onReplayComplete: (marker: string) => void
}

export function TerminalSurface(props: TerminalSurfaceProps) {
  const { onStatusChange, onSmokeMarker, onReplayComplete } = props
  const client = useRuntimeClient()
  const containerRef = useRef<HTMLDivElement>(null)

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
    const decoder = new TextDecoder()
    let observed = ''
    let replayRequested = false
    onStatusChange('starting-session')

    const onMessage = (message: RuntimeMessage) => {
      if (message.type === 'terminal.spawned') {
        onStatusChange('streaming')
        if (message.reattached && !replayRequested) {
          replayRequested = true
          client.requestTerminalReplay(SESSION_ID)
        }
        if (new URLSearchParams(window.location.search).get('e2e') === '1') {
          client.sendTerminalInput(SESSION_ID, `printf '${SMOKE_MARKER}\\n'\r`)
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
            client.requestTerminalReplay(SESSION_ID)
          }
        }
        terminal.write(bytes, () => client.acknowledgeTerminal(SESSION_ID, message.sequence))
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
      sessionId: SESSION_ID,
      executionContextId: 'local-default',
      profile: 'shell',
      cols: terminal.cols,
      rows: terminal.rows
    }, onMessage)
    const input = terminal.onData((data) => client.sendTerminalInput(SESSION_ID, data))
    const observer = new ResizeObserver(() => {
      fit.fit()
      client.resizeTerminal(SESSION_ID, terminal.cols, terminal.rows)
    })
    observer.observe(container)
    return () => {
      observer.disconnect()
      input.dispose()
      detach()
      terminal.dispose()
    }
  }, [client, onReplayComplete, onSmokeMarker, onStatusChange])

  return <div className="terminal-surface" ref={containerRef} />
}
