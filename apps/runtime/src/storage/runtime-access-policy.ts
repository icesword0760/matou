import type { RpcMethod, RuntimeCapability } from '@matou/contracts'

import { StorageReadOnlyError } from './database'

export type RuntimeAccessMode = 'normal' | 'read-only'

export type TerminalMessageType =
  | 'terminal.spawn'
  | 'terminal.input'
  | 'terminal.retry-last-input'
  | 'terminal.user-interaction'
  | 'terminal.resize'
  | 'terminal.dispose'
  | 'terminal.ack'
  | 'terminal.replay-request'
  | 'terminal.checkpoint'

const NORMAL_CAPABILITIES: RuntimeCapability[] = [
  'terminal-v1',
  'semantic-events-v1',
  'replay-v1',
  'domain-rpc-v1',
  'projection-v1',
  'hud-v1'
]

const READ_ONLY_CAPABILITIES: RuntimeCapability[] = [
  'semantic-events-v1',
  'replay-v1',
  'projection-v1'
]

const READ_ONLY_RPC_METHODS = new Set<RpcMethod>([
  'projection.snapshot',
  'hierarchy.get-scene-session-graph',
  'claude-sessions.list',
  'claude-sessions.detail',
  'geometry.list',
  'events.replay',
  'events.ack'
])

const READ_ONLY_TERMINAL_MESSAGES = new Set<TerminalMessageType>([
  'terminal.replay-request',
  'terminal.ack'
])

export class RuntimeAccessPolicy {
  readonly mode: RuntimeAccessMode

  constructor(mode: RuntimeAccessMode) {
    this.mode = mode
  }

  get startBackgroundServices(): boolean {
    return this.mode === 'normal'
  }

  get capabilities(): RuntimeCapability[] {
    return [...(this.mode === 'normal' ? NORMAL_CAPABILITIES : READ_ONLY_CAPABILITIES)]
  }

  get readOnly(): boolean {
    return this.mode === 'read-only'
  }

  assertRpcAllowed(method: RpcMethod): void {
    if (this.mode === 'read-only' && !READ_ONLY_RPC_METHODS.has(method)) {
      throw new StorageReadOnlyError()
    }
  }

  assertTerminalAllowed(type: TerminalMessageType): void {
    if (this.mode === 'read-only' && !READ_ONLY_TERMINAL_MESSAGES.has(type)) {
      throw new StorageReadOnlyError()
    }
  }
}
