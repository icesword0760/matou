import { describe, expect, it } from 'vitest'

import { PROTOCOL_VERSION, parseRendererMessage } from './protocol'

describe('parseRendererMessage', () => {
  it('accepts an exact-version hello message', () => {
    const message = parseRendererMessage({
      type: 'protocol.hello',
      protocolVersion: PROTOCOL_VERSION,
      clientId: 'renderer-1'
    })

    expect(message).toEqual({
      type: 'protocol.hello',
      protocolVersion: 1,
      clientId: 'renderer-1'
    })
  })

  it('rejects a hello message from another protocol version', () => {
    expect(() =>
      parseRendererMessage({
        type: 'protocol.hello',
        protocolVersion: 2,
        clientId: 'renderer-1'
      })
    ).toThrow(/protocolVersion/)
  })

  it('accepts a bounded terminal spawn request', () => {
    const message = parseRendererMessage({
      type: 'terminal.spawn',
      protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1',
      executionContextId: 'context-1',
      profile: 'shell',
      cols: 120,
      rows: 40
    })

    expect(message.type).toBe('terminal.spawn')
  })

  it('rejects terminal dimensions outside the contract', () => {
    expect(() =>
      parseRendererMessage({
        type: 'terminal.spawn',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: 'session-1',
        executionContextId: 'context-1',
        profile: 'shell',
        cols: 1,
        rows: 501
      })
    ).toThrow()
  })

  it('rejects session identifiers that could escape journal directories', () => {
    expect(() =>
      parseRendererMessage({
        type: 'terminal.spawn',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: '../outside',
        executionContextId: 'context-1',
        profile: 'shell',
        cols: 80,
        rows: 24
      })
    ).toThrow(/sessionId/)
  })

  it('accepts cumulative acknowledgements', () => {
    const message = parseRendererMessage({
      type: 'terminal.ack',
      protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1',
      throughSequence: 17
    })

    expect(message).toMatchObject({
      type: 'terminal.ack',
      throughSequence: 17
    })
  })

  it('rejects unknown message types', () => {
    expect(() =>
      parseRendererMessage({
        type: 'terminal.execute-arbitrary',
        protocolVersion: PROTOCOL_VERSION
      })
    ).toThrow()
  })

  it('accepts bounded versioned domain RPC requests', () => {
    expect(parseRendererMessage({
      type: 'rpc.request',
      protocolVersion: PROTOCOL_VERSION,
      requestId: 'request-1',
      method: 'projection.snapshot',
      capability: 'renderer',
      deadlineAt: Date.now() + 1000,
      payload: {}
    })).toMatchObject({ type: 'rpc.request', method: 'projection.snapshot' })
  })

  it('rejects RPC methods outside the explicit allowlist', () => {
    expect(() => parseRendererMessage({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'request-1',
      method: 'database.execute', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: { sql: 'DROP TABLE workspaces' }
    })).toThrow()
  })

  it('bounds semantic event subscription batches', () => {
    expect(() => parseRendererMessage({
      type: 'events.subscribe', protocolVersion: PROTOCOL_VERSION,
      consumerId: 'renderer-1', afterSequence: 0, batchSize: 1001
    })).toThrow()
  })
})
