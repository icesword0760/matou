import { describe, expect, it } from 'vitest'

import { RPC_METHODS, type RpcMethod } from '@matou/contracts'

import { StorageReadOnlyError } from './database'
import { RuntimeAccessPolicy } from './runtime-access-policy'

const READ_ONLY_RPC_METHODS = [
  'projection.snapshot',
  'provider-config.snapshot',
  'hierarchy.get-scene-session-graph',
  'claude-sessions.list',
  'claude-sessions.detail',
  'claude-sessions.search',
  'session.environment-open',
  'session.instructions-read',
  'geometry.list',
  'terminal.history-page',
  'terminal.history-search',
  'events.replay',
  'events.ack'
] satisfies RpcMethod[]

describe('RuntimeAccessPolicy', () => {
  it('keeps the normal Runtime capability surface unrestricted', () => {
    const policy = new RuntimeAccessPolicy('normal')

    for (const method of RPC_METHODS) expect(() => policy.assertRpcAllowed(method)).not.toThrow()
    for (const type of [
      'terminal.spawn', 'terminal.input', 'terminal.resize', 'terminal.dispose',
      'terminal.user-interaction', 'terminal.retry-last-input', 'terminal.ack',
      'terminal.replay-request', 'terminal.checkpoint', 'terminal.hud-refresh'
    ] as const) {
      expect(() => policy.assertTerminalAllowed(type)).not.toThrow()
    }
    expect(policy.startBackgroundServices).toBe(true)
  })

  it('allows only explicit read/query RPC methods in read-only mode', () => {
    const policy = new RuntimeAccessPolicy('read-only')

    for (const method of RPC_METHODS) {
      if (READ_ONLY_RPC_METHODS.includes(method as typeof READ_ONLY_RPC_METHODS[number])) {
        expect(() => policy.assertRpcAllowed(method)).not.toThrow()
      } else {
        expect(() => policy.assertRpcAllowed(method)).toThrow(StorageReadOnlyError)
      }
    }
  })

  it('allows replay flow but rejects every terminal lifecycle mutation', () => {
    const policy = new RuntimeAccessPolicy('read-only')

    for (const type of ['terminal.replay-request', 'terminal.ack'] as const) {
      expect(() => policy.assertTerminalAllowed(type)).not.toThrow()
    }
    for (const type of [
      'terminal.spawn', 'terminal.input', 'terminal.resize', 'terminal.dispose',
      'terminal.user-interaction', 'terminal.retry-last-input', 'terminal.checkpoint',
      'terminal.hud-refresh'
    ] as const) {
      expect(() => policy.assertTerminalAllowed(type)).toThrow(StorageReadOnlyError)
    }
    expect(policy.startBackgroundServices).toBe(false)
    expect(policy.capabilities).toEqual([
      'semantic-events-v1', 'replay-v1', 'projection-v1'
    ])
  })
})
