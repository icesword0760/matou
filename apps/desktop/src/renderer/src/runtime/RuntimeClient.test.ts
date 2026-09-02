import { describe, expect, it } from 'vitest'

import { PROTOCOL_VERSION } from '@matou/contracts'

import { RuntimeClient, type RuntimeClientPort } from './RuntimeClient'

describe('RuntimeClient', () => {
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

  it('asks Runtime to refresh the visible Session HUD', () => {
    const port = new FakePort()
    const client = new RuntimeClient(port, { clientId: 'renderer-1' })

    client.refreshTerminalHud('session-1')

    expect(port.sent.at(-1)).toMatchObject({
      type: 'terminal.hud-refresh', sessionId: 'session-1'
    })
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
