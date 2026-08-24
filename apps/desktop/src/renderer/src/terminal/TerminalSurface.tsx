import { useEffect, useRef } from 'react'

import { PROTOCOL_VERSION, type RuntimeMessage } from '@matou/contracts'
import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'

const PORT_CHANNEL = 'matou:terminal-port'
const RENDERER_READY = 'matou:renderer-ready'
const SESSION_ID = 'foundation-shell'
const SMOKE_MARKER = '__MATOU_CHANNEL_READY__'

export type RuntimeStatus =
  | 'waiting-for-port'
  | 'handshaking'
  | 'starting-session'
  | 'streaming'
  | 'error'
  | 'exited'

interface TerminalSurfaceProps {
  onStatusChange: (status: RuntimeStatus) => void
  onSmokeMarker: (marker: string) => void
  onReplayComplete: (marker: string) => void
}

export function TerminalSurface({ onStatusChange, onSmokeMarker, onReplayComplete }: TerminalSurfaceProps) {
  const containerRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const container = containerRef.current
    if (!container) {
      return
    }

    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: 'JetBrains Mono, SFMono-Regular, Menlo, monospace',
      fontSize: 13,
      lineHeight: 1.2,
      scrollback: 10_000,
      theme: {
        background: '#0b0e14',
        foreground: '#d6deeb',
        cursor: '#82aaff',
        selectionBackground: '#283457'
      }
    })
    const fitAddon = new FitAddon()
    terminal.loadAddon(fitAddon)
    terminal.open(container)
    fitAddon.fit()

    let port: MessagePort | undefined
    let spawned = false
    let observedOutput = ''
    let replayRequested = false
    const decoder = new TextDecoder()

    const post = (message: unknown) => port?.postMessage(message)

    const handleRuntimeMessage = (event: MessageEvent<RuntimeMessage>) => {
      const message = event.data
      switch (message.type) {
        case 'protocol.ready':
          onStatusChange('starting-session')
          post({
            type: 'terminal.spawn',
            protocolVersion: PROTOCOL_VERSION,
            sessionId: SESSION_ID,
            executionContextId: 'local-default',
            profile: 'shell',
            cols: terminal.cols,
            rows: terminal.rows
          })
          break
        case 'protocol.error':
          console.error(`[Runtime ${message.code}] ${message.message}`)
          terminal.writeln(`\r\n[Runtime ${message.code}] ${message.message}`)
          onStatusChange('error')
          break
        case 'terminal.spawned':
          spawned = true
          onStatusChange('streaming')
          if (message.reattached && !replayRequested) {
            replayRequested = true
            post({
              type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
              sessionId: SESSION_ID, fromSequence: 0
            })
          }
          if (new URLSearchParams(window.location.search).get('e2e') === '1') {
            post({
              type: 'terminal.input',
              protocolVersion: PROTOCOL_VERSION,
              sessionId: SESSION_ID,
              data: `printf '${SMOKE_MARKER}\\n'\r`
            })
          }
          break
        case 'terminal.data': {
          const bytes = message.data instanceof Uint8Array ? message.data : new Uint8Array(message.data)
          observedOutput = (observedOutput + decoder.decode(bytes, { stream: true })).slice(-8192)
          if (observedOutput.includes(SMOKE_MARKER)) {
            onSmokeMarker(SMOKE_MARKER)
            if (!replayRequested && new URLSearchParams(window.location.search).get('e2e') === '1') {
              replayRequested = true
              post({
                type: 'terminal.replay-request',
                protocolVersion: PROTOCOL_VERSION,
                sessionId: SESSION_ID,
                fromSequence: 0
              })
            }
          }
          terminal.write(bytes, () => {
            post({
              type: 'terminal.ack',
              protocolVersion: PROTOCOL_VERSION,
              sessionId: SESSION_ID,
              throughSequence: message.sequence
            })
          })
          break
        }
        case 'terminal.exited':
          onStatusChange('exited')
          break
        case 'terminal.replay-start':
          terminal.reset()
          if (message.checkpoint) {
            const snapshot = message.checkpoint.snapshot instanceof Uint8Array
              ? message.checkpoint.snapshot
              : new Uint8Array(message.checkpoint.snapshot)
            terminal.write(snapshot)
          }
          break
        case 'terminal.gap':
          break
        case 'terminal.replay-complete':
          onReplayComplete(`replayed-through:${message.throughSequence}`)
          break
      }
    }

    const handlePort = (event: MessageEvent) => {
      if (event.source !== window || event.data?.type !== PORT_CHANNEL || event.ports.length !== 1) {
        return
      }
      const [receivedPort] = event.ports
      if (!receivedPort) {
        return
      }
      port = receivedPort
      receivedPort.onmessage = handleRuntimeMessage
      receivedPort.start()
      onStatusChange('handshaking')
      post({
        type: 'protocol.hello',
        protocolVersion: PROTOCOL_VERSION,
        clientId: crypto.randomUUID()
      })
    }

    const inputSubscription = terminal.onData((data) => {
      if (!spawned) {
        return
      }
      post({
        type: 'terminal.input',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: SESSION_ID,
        data
      })
    })

    let resizeFrame = 0
    const resizeObserver = new ResizeObserver(() => {
      cancelAnimationFrame(resizeFrame)
      resizeFrame = requestAnimationFrame(() => {
        fitAddon.fit()
        if (spawned) {
          post({
            type: 'terminal.resize',
            protocolVersion: PROTOCOL_VERSION,
            sessionId: SESSION_ID,
            cols: terminal.cols,
            rows: terminal.rows
          })
        }
      })
    })
    resizeObserver.observe(container)

    window.addEventListener('message', handlePort)
    window.postMessage({ type: RENDERER_READY }, '*')

    return () => {
      window.removeEventListener('message', handlePort)
      resizeObserver.disconnect()
      cancelAnimationFrame(resizeFrame)
      inputSubscription.dispose()
      if (spawned) {
        post({
          type: 'terminal.dispose',
          protocolVersion: PROTOCOL_VERSION,
          sessionId: SESSION_ID
        })
      }
      port?.close()
      terminal.dispose()
    }
  }, [onReplayComplete, onSmokeMarker, onStatusChange])

  return <div className="terminal-surface" ref={containerRef} />
}
