import { describe, expect, it } from 'vitest'

import { PROTOCOL_VERSION, parseRendererMessage } from '@matou/contracts'

import { RuntimeClient, type RuntimeClientPort } from './RuntimeClient'

describe('RuntimeClient', () => {
  it('gives every terminal resize a session-local identity', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port)

    client.resizeTerminal('session-a', 80, 24)
    client.resizeTerminal('session-a', 100, 30)
    client.resizeTerminal('session-b', 120, 40)

    expect(port.sent.filter((message) => message.type === 'terminal.resize')).toEqual([
      expect.objectContaining({ sessionId: 'session-a', resizeId: 1, cols: 80, rows: 24 }),
      expect.objectContaining({ sessionId: 'session-a', resizeId: 2, cols: 100, rows: 30 }),
      expect.objectContaining({ sessionId: 'session-b', resizeId: 1, cols: 120, rows: 40 })
    ])
  })
  it('correlates RPC and reattaches terminal consumers after a new port', async () => {
    const first = new FakePort()
    const client = new RuntimeClient(first, { clientId: 'renderer-1' })
    first.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1', capabilities: ['terminal-v1']
    })
    const detach = client.attachTerminal({
      sessionId: 'session-1', executionContextId: 'context-1',
      profile: 'shell', cols: 80, rows: 24
    }, () => {})
    expect(first.sent.map((message) => message.type)).toContain('terminal.spawn')

    const request = client.request('hierarchy.bootstrap-window', { input: true })
    await Promise.resolve()
    const rpc = first.sent.find((message) => message.type === 'rpc.request')!
    first.deliver({
      type: 'rpc.response', protocolVersion: PROTOCOL_VERSION,
      requestId: rpc.requestId, runtimeGeneration: 'generation-1',
      result: { windowId: 'window-1' }
    })
    await expect(request).resolves.toEqual({ windowId: 'window-1' })

    const second = new FakePort()
    client.replacePort(second)
    second.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-2', capabilities: ['terminal-v1']
    })
    expect(second.sent.map((message) => message.type)).toContain('terminal.spawn')
    detach()
    expect(second.sent.map((message) => message.type)).not.toContain('terminal.dispose')
  })

  it('waits for readiness and resumes the projection stream after reconnect', async () => {
    const first = new FakePort()
    const client = new RuntimeClient(first, { clientId: 'renderer-1' })
    client.startProjection(12)
    const pending = client.request('projection.snapshot', { windowId: 'window-1' })
    expect(first.sent.some(({ type }) => type === 'rpc.request')).toBe(false)

    first.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1', capabilities: ['projection-v1']
    })
    expect(first.sent).toContainEqual(expect.objectContaining({
      type: 'events.subscribe', afterSequence: 12,
      consumerId: 'renderer-1-projection'
    }))
    await Promise.resolve()
    const rpc = first.sent.find(({ type }) => type === 'rpc.request')!
    first.deliver({
      type: 'rpc.response', protocolVersion: PROTOCOL_VERSION,
      requestId: rpc.requestId, runtimeGeneration: 'generation-1', result: { ok: true }
    })
    await expect(pending).resolves.toEqual({ ok: true })

    const second = new FakePort()
    client.replacePort(second)
    second.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-2', capabilities: ['projection-v1']
    })
    expect(second.sent).toContainEqual(expect.objectContaining({
      type: 'events.subscribe', afterSequence: 12
    }))
  })

  it('posts the interaction marker before its related terminal bytes', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })
    port.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1', capabilities: ['terminal-v1']
    })

    client.recordTerminalInteraction('session-1', 'submit', true)
    client.sendTerminalInput('session-1', '\r')

    expect(port.sent.slice(-2)).toEqual([
      expect.objectContaining({
        type: 'terminal.user-interaction', sessionId: 'session-1', interactionKind: 'submit',
        deferOrdering: true
      }),
      expect.objectContaining({ type: 'terminal.input', sessionId: 'session-1', data: '\r' })
    ])
  })

  it('transparently sends multi-megabyte UTF-8 input as ordered protocol-safe chunks', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })
    const value = `${'a'.repeat(1024 * 1024)}🧭${'中文e\u0301'.repeat(32 * 1024)}`

    client.sendTerminalInput('session-1', value)

    const messages = port.sent.filter(({ type }) => type === 'terminal.input')
    expect(messages.length).toBeGreaterThan(4)
    expect(messages.map(({ data }) => data).join('')).toBe(value)
    expect(messages.every((message) => {
      expect(() => parseRendererMessage(message)).not.toThrow()
      return new TextEncoder().encode(message.data).byteLength <= 256 * 1024
    })).toBe(true)
  })

  it('updates a promoted terminal profile without spawning again until channel recovery', () => {
    const first = new FakePort()
    const client = new RuntimeClient(first, { clientId: 'renderer-1' })
    client.attachTerminal({
      sessionId: 'session-1', executionContextId: 'context-1',
      profile: 'shell', cols: 80, rows: 24
    }, () => undefined)
    first.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1', capabilities: ['terminal-v1']
    })
    const spawnCount = first.sent.filter(({ type }) => type === 'terminal.spawn').length

    client.updateTerminalProfile('session-1', 'claude-code')

    expect(first.sent.filter(({ type }) => type === 'terminal.spawn')).toHaveLength(spawnCount)
    const second = new FakePort()
    client.replacePort(second)
    second.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-2', capabilities: ['terminal-v1']
    })
    expect(second.sent).toContainEqual(expect.objectContaining({
      type: 'terminal.spawn', sessionId: 'session-1', profile: 'claude-code'
    }))
  })

  it('asks Runtime to retry the last submitted input in the same Session', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })
    port.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1', capabilities: ['terminal-v1']
    })

    client.retryLastTerminalInput('session-1')

    expect(port.sent.at(-1)).toMatchObject({
      type: 'terminal.retry-last-input', sessionId: 'session-1'
    })
  })

  it('sends a serialized terminal checkpoint with its applied Journal watermark', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })

    client.storeTerminalCheckpoint('session-1', 42, 3, '\u001b[2Jscreen')

    expect(port.sent.at(-1)).toMatchObject({
      type: 'terminal.checkpoint', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1', throughSequence: 42, screenEpoch: 3,
      snapshot: '\u001b[2Jscreen'
    })
    expect(() => parseRendererMessage(port.sent.at(-1))).not.toThrow()
  })

  it('keeps one checkpoint in flight per Session and retains only the newest pending screen', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })

    client.storeTerminalCheckpoint('session-1', 1, 0, 'first')
    client.storeTerminalCheckpoint('session-1', 2, 0, 'second')
    client.storeTerminalCheckpoint('session-1', 3, 0, 'newest')
    client.storeTerminalCheckpoint('session-2', 4, 1, 'independent')

    expect(port.sent.filter(({ type }) => type === 'terminal.checkpoint')).toEqual([
      expect.objectContaining({ sessionId: 'session-1', throughSequence: 1, snapshot: 'first' }),
      expect.objectContaining({ sessionId: 'session-2', throughSequence: 4, snapshot: 'independent' })
    ])
    port.deliver({
      type: 'terminal.checkpoint-stored', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1', throughSequence: 1
    })
    expect(port.sent.filter(({ type }) => type === 'terminal.checkpoint').at(-1)).toMatchObject({
      sessionId: 'session-1', throughSequence: 3, snapshot: 'newest'
    })
  })

  it('continues checkpointing after rejection and after replacing a port with a lost acknowledgement', () => {
    const first = new FakePort()
    const client = new RuntimeClient(first, { clientId: 'renderer-1' })
    client.storeTerminalCheckpoint('session-1', 1, 0, 'first')
    client.storeTerminalCheckpoint('session-1', 2, 0, 'pending')

    first.deliver({
      type: 'terminal.checkpoint-rejected', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1', throughSequence: 1, reason: 'stale'
    })
    expect(first.sent.filter(({ type }) => type === 'terminal.checkpoint').at(-1)).toMatchObject({
      sessionId: 'session-1', throughSequence: 2, snapshot: 'pending'
    })

    const second = new FakePort()
    client.replacePort(second)
    client.storeTerminalCheckpoint('session-1', 3, 0, 'after reconnect')
    expect(second.sent.at(-1)).toMatchObject({
      type: 'terminal.checkpoint', sessionId: 'session-1', throughSequence: 3,
      snapshot: 'after reconnect'
    })
  })

  it('drains the final pending foreground checkpoint after its terminal view detaches', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })
    const detach = client.attachTerminal({
      sessionId: 'session-1', executionContextId: 'context-1',
      profile: 'shell', cols: 80, rows: 24
    }, () => undefined)
    client.storeTerminalCheckpoint('session-1', 1, 0, 'quiet')
    client.storeTerminalCheckpoint('session-1', 2, 0, 'leaving foreground')

    detach()
    port.deliver({
      type: 'terminal.checkpoint-stored', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1', throughSequence: 1
    })

    expect(port.sent.filter(({ type }) => type === 'terminal.checkpoint').at(-1)).toMatchObject({
      sessionId: 'session-1', throughSequence: 2, snapshot: 'leaving foreground'
    })
  })

  it('attaches a read-only terminal through replay without spawning a process', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })
    port.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1', capabilities: ['replay-v1']
    })

    client.attachTerminal({
      sessionId: 'session-read-only', executionContextId: 'context-1',
      profile: 'shell', cols: 80, rows: 24, readOnly: true
    }, () => undefined)

    expect(port.sent).toContainEqual(expect.objectContaining({
      type: 'terminal.replay-request', sessionId: 'session-read-only', fromSequence: 0
    }))
    expect(port.sent.some(({ type }) => type === 'terminal.spawn')).toBe(false)
  })

  it('downgrades existing terminal consumers before a read-only Runtime port becomes ready', () => {
    const first = new FakePort()
    const client = new RuntimeClient(first, { clientId: 'renderer-1' })
    first.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1', capabilities: ['terminal-v1']
    })
    client.attachTerminal({
      sessionId: 'session-1', executionContextId: 'context-1',
      profile: 'shell', cols: 80, rows: 24
    }, () => undefined)

    client.setRuntimeMode('read-only')
    const second = new FakePort()
    client.replacePort(second)
    second.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-2', capabilities: ['replay-v1']
    })

    expect(second.sent).toContainEqual(expect.objectContaining({
      type: 'terminal.replay-request', sessionId: 'session-1'
    }))
    expect(second.sent.some(({ type }) => type === 'terminal.spawn')).toBe(false)
  })

  it('drops terminal mutations immediately after Runtime enters read-only recovery', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })
    port.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1', capabilities: ['terminal-v1']
    })
    client.setRuntimeMode('read-only')
    const before = port.sent.length

    client.sendTerminalInput('session-1', 'blocked')
    client.resizeTerminal('session-1', 100, 30)
    client.retryLastTerminalInput('session-1')
    client.recordTerminalInteraction('session-1', 'submit')
    client.storeTerminalCheckpoint('session-1', 1, 0, 'blocked')
    client.disposeDeletedTerminal('session-1')

    expect(port.sent).toHaveLength(before)
  })

  it('replays a terminal attached after Runtime has already entered read-only recovery', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })
    port.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1', capabilities: ['replay-v1']
    })
    client.setRuntimeMode('read-only')

    client.attachTerminal({
      sessionId: 'session-1', executionContextId: 'context-1',
      profile: 'shell', cols: 80, rows: 24
    }, () => undefined)

    expect(port.sent).toContainEqual(expect.objectContaining({
      type: 'terminal.replay-request', sessionId: 'session-1'
    }))
    expect(port.sent.some(({ type }) => type === 'terminal.spawn')).toBe(false)
  })

  it('routes a Session-scoped startup error only to that terminal card', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })
    port.deliver({
      type: 'protocol.ready', protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1', capabilities: ['terminal-v1']
    })
    const first: any[] = []
    const second: any[] = []
    client.attachTerminal({
      sessionId: 'session-1', executionContextId: 'context-1', profile: 'shell', cols: 80, rows: 24
    }, (message) => first.push(message))
    client.attachTerminal({
      sessionId: 'session-2', executionContextId: 'context-1', profile: 'shell', cols: 80, rows: 24
    }, (message) => second.push(message))

    port.deliver({
      type: 'protocol.error', protocolVersion: PROTOCOL_VERSION,
      code: 'WORKSPACE_PATH_INVALID', message: '工作空间目录不存在', sessionId: 'session-2'
    })

    expect(first).toEqual([])
    expect(second).toEqual([expect.objectContaining({
      type: 'protocol.error', code: 'WORKSPACE_PATH_INVALID', sessionId: 'session-2'
    })])
  })
})

class FakePort implements RuntimeClientPort {
  readonly sent: any[] = []
  onmessage: ((event: MessageEvent) => void) | null = null
  postMessage(message: any): void { this.sent.push(message) }
  start(): void {}
  close(): void {}
  deliver(data: any): void { this.onmessage?.({ data } as MessageEvent) }
}
