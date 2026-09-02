import { describe, expect, it } from 'vitest'

import { PROTOCOL_VERSION, RPC_METHODS, parseRendererMessage } from './protocol'

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

  it('accepts an explicit HUD refresh for a visible Session', () => {
    expect(parseRendererMessage({
      type: 'terminal.hud-refresh', protocolVersion: PROTOCOL_VERSION, sessionId: 'session-1'
    })).toEqual({
      type: 'terminal.hud-refresh', protocolVersion: PROTOCOL_VERSION, sessionId: 'session-1'
    })
  })

  it.each(['submit', 'control', 'provider-action'])(
    'accepts a completed %s interaction marker',
    (interactionKind) => {
      expect(parseRendererMessage({
        type: 'terminal.user-interaction',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: 'session-1',
        interactionKind,
        deferOrdering: true
      })).toMatchObject({ type: 'terminal.user-interaction', interactionKind, deferOrdering: true })
    }
  )

  it.each(['click', 'output', 'draft'])(
    'rejects a non-ordering %s interaction marker',
    (interactionKind) => {
      expect(() => parseRendererMessage({
        type: 'terminal.user-interaction',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: 'session-1',
        interactionKind
      })).toThrow()
    }
  )

  it('accepts a retry request scoped to the last submitted terminal input', () => {
    expect(parseRendererMessage({
      type: 'terminal.retry-last-input',
      protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1'
    })).toMatchObject({ type: 'terminal.retry-last-input', sessionId: 'session-1' })
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

  it('allowlists every PRD 05 hierarchy workflow', () => {
    expect(RPC_METHODS).toEqual(expect.arrayContaining([
      'hierarchy.bootstrap-window',
      'hierarchy.create-workspace',
      'hierarchy.rename-workspace',
      'hierarchy.remove-workspace',
      'hierarchy.activate-workspace',
      'hierarchy.validate-workspace-path',
      'hierarchy.create-task',
      'hierarchy.rename-task',
      'hierarchy.rename-session',
      'hierarchy.restore-session-auto-title',
      'hierarchy.reorder-task',
      'hierarchy.move-task-on-board',
      'hierarchy.delete-task',
      'hierarchy.activate-task',
      'hierarchy.create-scene',
      'hierarchy.rename-scene',
      'hierarchy.reorder-scene',
      'hierarchy.close-scene',
      'hierarchy.activate-scene',
      'hierarchy.split-session',
      'hierarchy.activate-session',
      'hierarchy.delete-session',
      'hierarchy.replace-layout',
      'hierarchy.detach-session',
      'hierarchy.return-session',
      'hierarchy.move-task-to-window'
    ]))
  })

  it('allowlists the PRD 06 one-shot Session fork workflow', () => {
    expect(RPC_METHODS).toContain('hierarchy.fork-session')
    expect(parseRendererMessage({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'fork-1',
      method: 'hierarchy.fork-session', capability: 'renderer',
      deadlineAt: Date.now() + 1000, payload: {}
    })).toMatchObject({ type: 'rpc.request', method: 'hierarchy.fork-session' })
  })

  it('allowlists every session canvas and DAG workflow', () => {
    expect(RPC_METHODS).toEqual(expect.arrayContaining([
      'hierarchy.create-canvas',
      'hierarchy.create-shell-sibling',
      'hierarchy.create-fork-child',
      'hierarchy.create-fork-sibling',
      'hierarchy.retry-fork',
      'hierarchy.remove-failed-fork',
      'hierarchy.record-session-interaction',
      'hierarchy.retry-provider-restore',
      'hierarchy.restart-stopped-session',
      'hierarchy.remove-session-branch',
      'hierarchy.reopen-scene',
      'hierarchy.get-scene-session-graph',
      'hierarchy.set-focused-session'
    ]))
    expect(parseRendererMessage({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'graph-1',
      method: 'hierarchy.get-scene-session-graph', capability: 'renderer',
      deadlineAt: Date.now() + 1000, payload: { sceneId: 'scene-1' }
    })).toMatchObject({ type: 'rpc.request', method: 'hierarchy.get-scene-session-graph' })
  })

  it('allowlists the PRD 04 permission-mode persistence workflow', () => {
    expect(RPC_METHODS).toContain('session.set-permission-mode')
    expect(parseRendererMessage({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'permission-1',
      method: 'session.set-permission-mode', capability: 'renderer',
      deadlineAt: Date.now() + 1000, payload: {}
    })).toMatchObject({ type: 'rpc.request', method: 'session.set-permission-mode' })
  })

  it('allowlists global provider configuration and switching', () => {
    expect(RPC_METHODS).toEqual(expect.arrayContaining([
      'provider-config.snapshot',
      'provider-config.upsert',
      'provider-config.delete',
      'provider-config.activate'
    ]))
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
