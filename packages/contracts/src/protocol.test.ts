import { describe, expect, it } from 'vitest'

import {
  MAX_CHECKPOINT_SNAPSHOT_BYTES,
  PROTOCOL_VERSION,
  RPC_METHODS,
  parseRendererMessage,
  type RuntimeMessage
} from './protocol'

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

  it('accepts an optional main-window identity during the protocol transition', () => {
    expect(parseRendererMessage({
      type: 'protocol.hello',
      protocolVersion: PROTOCOL_VERSION,
      clientId: 'renderer-main-2',
      windowId: 'main-window-2',
      windowKind: 'main'
    })).toEqual({
      type: 'protocol.hello',
      protocolVersion: PROTOCOL_VERSION,
      clientId: 'renderer-main-2',
      windowId: 'main-window-2',
      windowKind: 'main'
    })

    expect(parseRendererMessage({
      type: 'protocol.hello',
      protocolVersion: PROTOCOL_VERSION,
      clientId: 'renderer-detached-1',
      windowId: 'detached-window-1',
      windowKind: 'detached-terminal'
    })).toMatchObject({ windowKind: 'detached-terminal' })
  })

  it('accepts bounded navigation acknowledgements with optional result details', () => {
    expect(parseRendererMessage({
      type: 'host.navigation-result',
      protocolVersion: PROTOCOL_VERSION,
      requestId: 'nav-1',
      windowId: 'main-window-2',
      ok: true,
      finalPath: {
        windowId: 'main-window-2',
        workspaceId: 'workspace-2',
        taskId: 'task-2',
        sceneId: 'scene-2',
        sessionId: 'session-2'
      }
    })).toMatchObject({ type: 'host.navigation-result', requestId: 'nav-1', ok: true })

    expect(parseRendererMessage({
      type: 'host.navigation-result',
      protocolVersion: PROTOCOL_VERSION,
      requestId: 'nav-2',
      windowId: 'main-window-2',
      ok: false,
      error: 'target card is no longer mounted'
    })).toMatchObject({ type: 'host.navigation-result', requestId: 'nav-2', ok: false })

    expect(parseRendererMessage({
      type: 'host.navigation-result',
      protocolVersion: PROTOCOL_VERSION,
      requestId: 'nav-missing-path',
      windowId: 'main-window-2',
      ok: true
    })).toMatchObject({ requestId: 'nav-missing-path', ok: true })
  })

  it('exposes a complete absolute-deadline navigation request to the Renderer', () => {
    const message = {
      type: 'host.navigation-request',
      protocolVersion: PROTOCOL_VERSION,
      requestId: 'nav-1',
      windowId: 'main-window-2',
      workspaceId: 'workspace-2',
      taskId: 'task-2',
      sceneId: 'scene-2',
      sessionId: 'session-2',
      focusTerminal: true,
      deadlineAt: 5_000
    } satisfies RuntimeMessage

    expect(message).toMatchObject({
      type: 'host.navigation-request',
      requestId: 'nav-1',
      deadlineAt: 5_000
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

  it('requires a resize identity so Runtime application can be observed exactly', () => {
    expect(parseRendererMessage({
      type: 'terminal.resize', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1', resizeId: 7, cols: 120, rows: 40
    })).toMatchObject({ type: 'terminal.resize', resizeId: 7, cols: 120, rows: 40 })
    expect(() => parseRendererMessage({
      type: 'terminal.resize', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1', cols: 120, rows: 40
    })).toThrow(/resizeId/)
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

  it('accepts a bounded serialized terminal checkpoint', () => {
    expect(parseRendererMessage({
      type: 'terminal.checkpoint', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1', throughSequence: 17, screenEpoch: 2,
      snapshot: '\u001b[2Jrestored screen'
    })).toMatchObject({ type: 'terminal.checkpoint', throughSequence: 17 })
  })

  it('rejects unsafe checkpoint identities and oversized UTF-8 snapshots', () => {
    expect(() => parseRendererMessage({
      type: 'terminal.checkpoint', protocolVersion: PROTOCOL_VERSION,
      sessionId: '../outside', throughSequence: 17, screenEpoch: 2, snapshot: 'screen'
    })).toThrow(/sessionId/)
    expect(() => parseRendererMessage({
      type: 'terminal.checkpoint', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1', throughSequence: 17, screenEpoch: 2,
      snapshot: '😀'.repeat(Math.floor(MAX_CHECKPOINT_SNAPSHOT_BYTES / 4) + 1)
    })).toThrow(/transport limit/)
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

  it('accepts foreground recovery priority, retry, and view detach messages', () => {
    expect(parseRendererMessage({
      type: 'session.recovery-prioritize', protocolVersion: PROTOCOL_VERSION,
      sceneId: 'scene-1', activeSessionId: 'session-1',
      foregroundSessionIds: ['session-1', 'session-offscreen']
    })).toMatchObject({
      type: 'session.recovery-prioritize', sceneId: 'scene-1',
      foregroundSessionIds: ['session-1', 'session-offscreen']
    })
    expect(parseRendererMessage({
      type: 'session.recovery-retry', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1'
    })).toMatchObject({ type: 'session.recovery-retry', sessionId: 'session-1' })
    expect(parseRendererMessage({
      type: 'terminal.view-detach', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1'
    })).toMatchObject({ type: 'terminal.view-detach', sessionId: 'session-1' })
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
      'hierarchy.reorder-task',
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
      'hierarchy.get-scene-snapshot',
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

  it('allowlists terminal history paging and search RPCs', () => {
    expect(RPC_METHODS).toEqual(expect.arrayContaining([
      'terminal.history-page', 'terminal.history-search'
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

describe('runtime storage fault messages', () => {
  it('accepts scoped retry and end commands from the affected terminal card', () => {
    expect(parseRendererMessage({
      type: 'terminal.storage-retry', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1'
    })).toMatchObject({ type: 'terminal.storage-retry', sessionId: 'session-1' })
    expect(parseRendererMessage({
      type: 'terminal.storage-end', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1'
    })).toMatchObject({ type: 'terminal.storage-end', sessionId: 'session-1' })
  })

  it('exposes scoped storage fault and recovery messages', () => {
    const fault = {
      type: 'terminal.storage-fault',
      protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1',
      sequence: 42,
      code: 'STORAGE_WRITE_FAILED',
      message: 'journal is not writable',
      retainedBytes: 1024
    } satisfies import('./protocol').RuntimeMessage
    const recoveryRequired = {
      type: 'protocol.error',
      protocolVersion: PROTOCOL_VERSION,
      code: 'DATABASE_RECOVERY_REQUIRED',
      message: 'database recovery is required'
    } satisfies import('./protocol').RuntimeMessage
    const recovered = {
      type: 'terminal.storage-recovered',
      protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-1',
      sequence: 42
    } satisfies import('./protocol').RuntimeMessage

    expect({ fault, recoveryRequired, recovered }).toMatchObject({
      fault: { retainedBytes: 1024, sequence: 42 },
      recovered: { sessionId: 'session-1' },
      recoveryRequired: { code: 'DATABASE_RECOVERY_REQUIRED' }
    })
  })
})
