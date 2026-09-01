import { z } from 'zod'

import type { RuntimeErrorCode, StorageFaultCode } from './runtime-lifecycle'

export const PROTOCOL_VERSION = 1 as const
export const MAX_CHECKPOINT_SNAPSHOT_BYTES = 16 * 1024 * 1024

const protocolVersion = z.literal(PROTOCOL_VERSION)
const identifier = z
  .string()
  .min(1)
  .max(160)
  .regex(/^[A-Za-z0-9][A-Za-z0-9._-]*$/, 'identifier contains unsupported characters')
const sessionId = identifier

const helloSchema = z.object({
  type: z.literal('protocol.hello'),
  protocolVersion,
  clientId: identifier
})

const spawnSchema = z.object({
  type: z.literal('terminal.spawn'),
  protocolVersion,
  sessionId,
  executionContextId: identifier,
  profile: z.enum(['shell', 'claude-code', 'codex']),
  cols: z.number().int().min(2).max(1000),
  rows: z.number().int().min(1).max(500),
  spawnRevision: z.number().int().nonnegative().optional()
})

const inputSchema = z.object({
  type: z.literal('terminal.input'),
  protocolVersion,
  sessionId,
  data: z.string().max(1024 * 1024)
})

const retryLastInputSchema = z.object({
  type: z.literal('terminal.retry-last-input'),
  protocolVersion,
  sessionId
})

const userInteractionSchema = z.object({
  type: z.literal('terminal.user-interaction'),
  protocolVersion,
  sessionId,
  interactionKind: z.enum(['submit', 'control', 'provider-action']),
  deferOrdering: z.boolean().optional()
})

const resizeSchema = z.object({
  type: z.literal('terminal.resize'),
  protocolVersion,
  sessionId,
  resizeId: z.number().int().nonnegative(),
  cols: z.number().int().min(2).max(1000),
  rows: z.number().int().min(1).max(500)
})

const disposeSchema = z.object({
  type: z.literal('terminal.dispose'),
  protocolVersion,
  sessionId
})

const ackSchema = z.object({
  type: z.literal('terminal.ack'),
  protocolVersion,
  sessionId,
  throughSequence: z.number().int().nonnegative()
})

const replayRequestSchema = z.object({
  type: z.literal('terminal.replay-request'),
  protocolVersion,
  sessionId,
  fromSequence: z.number().int().nonnegative()
})

const checkpointSchema = z.object({
  type: z.literal('terminal.checkpoint'),
  protocolVersion,
  sessionId,
  throughSequence: z.number().int().nonnegative(),
  screenEpoch: z.number().int().nonnegative(),
  snapshot: z.string().refine(
    (value) => new TextEncoder().encode(value).byteLength <= MAX_CHECKPOINT_SNAPSHOT_BYTES,
    'checkpoint snapshot exceeds the transport limit'
  )
})

export const RPC_METHODS = [
  'projection.snapshot',
  'hierarchy.bootstrap-window',
  'hierarchy.create-workspace',
  'hierarchy.rename-workspace',
  'hierarchy.relink-workspace',
  'hierarchy.remove-workspace',
  'hierarchy.activate-workspace',
  'hierarchy.set-workspace-pinned',
  'hierarchy.reorder-pinned-workspace',
  'hierarchy.validate-workspace-path',
  'hierarchy.create-task',
  'hierarchy.rename-task',
  'hierarchy.reorder-task',
  'hierarchy.delete-task',
  'hierarchy.activate-task',
  'hierarchy.set-task-pinned',
  'hierarchy.reorder-pinned-task',
  'hierarchy.create-scene',
  'hierarchy.rename-scene',
  'hierarchy.reorder-scene',
  'hierarchy.close-scene',
  'hierarchy.activate-scene',
  'hierarchy.split-session',
  'hierarchy.fork-session',
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
  'hierarchy.set-focused-session',
  'hierarchy.activate-session',
  'hierarchy.delete-session',
  'hierarchy.replace-layout',
  'hierarchy.detach-session',
  'hierarchy.return-session',
  'hierarchy.move-task-to-window',
  'claude-sessions.list',
  'claude-sessions.detail',
  'claude-sessions.load',
  'workspace.create',
  'workspace.update',
  'workspace.archive',
  'execution-context.create-plain',
  'task.create',
  'task.update',
  'task.archive',
  'session.create',
  'session.update',
  'session.set-permission-mode',
  'session.set-model',
  'session.archive',
  'relation.create',
  'relation.revoke',
  'relation.restore',
  'scene.create',
  'scene.set-mode',
  'scene.add-node',
  'scene.remove-node',
  'scene.attach-window',
  'scene.detach-window',
  'scene.mount-session',
  'scene.unmount-session',
  'scene.archive',
  'geometry.put',
  'geometry.list',
  'git.status',
  'git.checkout',
  'git.create-branch',
  'git.commit',
  'git.push',
  'git.worktree-create',
  'git.worktree-open',
  'git.worktree-remove',
  'events.replay',
  'events.ack'
] as const

export type RpcMethod = (typeof RPC_METHODS)[number]

const rpcRequestSchema = z.object({
  type: z.literal('rpc.request'),
  protocolVersion,
  requestId: identifier,
  method: z.enum(RPC_METHODS),
  capability: z.literal('renderer'),
  deadlineAt: z.number().int().positive(),
  payload: z.unknown()
})

const rpcCancelSchema = z.object({
  type: z.literal('rpc.cancel'),
  protocolVersion,
  requestId: identifier
})

const eventsSubscribeSchema = z.object({
  type: z.literal('events.subscribe'),
  protocolVersion,
  consumerId: identifier,
  afterSequence: z.number().int().nonnegative(),
  batchSize: z.number().int().min(1).max(1000)
})

const rendererMessageSchema = z.discriminatedUnion('type', [
  helloSchema,
  spawnSchema,
  inputSchema,
  retryLastInputSchema,
  userInteractionSchema,
  resizeSchema,
  disposeSchema,
  ackSchema,
  replayRequestSchema,
  checkpointSchema,
  rpcRequestSchema,
  rpcCancelSchema,
  eventsSubscribeSchema
])

export type RendererMessage = z.infer<typeof rendererMessageSchema>

export function parseRendererMessage(value: unknown): RendererMessage {
  return rendererMessageSchema.parse(value)
}

export type RuntimeCapability =
  | 'terminal-v1'
  | 'semantic-events-v1'
  | 'replay-v1'
  | 'domain-rpc-v1'
  | 'projection-v1'
  | 'hud-v1'

type ProtocolErrorCode =
  | 'VERSION_MISMATCH'
  | 'INVALID_MESSAGE'
  | 'SESSION_FORBIDDEN'
  | 'WORKSPACE_PATH_INVALID'
  | 'INTERNAL_ERROR'
  | RuntimeErrorCode

type RpcErrorCode =
  | 'INVALID_REQUEST'
  | 'NOT_FOUND'
  | 'CONFLICT'
  | 'TIMEOUT'
  | 'CANCELLED'
  | 'CAPABILITY_DENIED'
  | 'INTERNAL_ERROR'
  | RuntimeErrorCode

export type RuntimeMessage =
  | {
      type: 'protocol.ready'
      protocolVersion: typeof PROTOCOL_VERSION
      runtimeId: string
      capabilities: RuntimeCapability[]
    }
  | {
      type: 'protocol.error'
      protocolVersion: typeof PROTOCOL_VERSION
      code: ProtocolErrorCode
      message: string
      sessionId?: string
    }
  | {
      type: 'terminal.storage-fault'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      sequence: number
      code: StorageFaultCode
      message: string
      retainedBytes: number
    }
  | {
      type: 'terminal.storage-recovered'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      sequence: number
    }
  | {
      type: 'terminal.hud'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      hud: import('./hud').SessionHudWire | null
    }
  | {
      type: 'terminal.spawned'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      pid: number
      reattached?: boolean
      replayFromSequence?: number
    }
  | {
      type: 'terminal.data'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      sequence: number
      data: Uint8Array
    }
  | {
      type: 'terminal.restored-history'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      blockCount: number
      data: Uint8Array
    }
  | {
      type: 'terminal.exited'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      sequence: number
      exitCode: number
      signal?: number
    }
  | {
      type: 'terminal.resized'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      resizeId: number
      cols: number
      rows: number
    }
  | {
      type: 'terminal.replay-start'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      checkpointSequence?: number
      checkpoint?: {
        terminalSequence: number
        domainEventSequence: number
        screenEpoch: number
        snapshot: Uint8Array
      }
      availableFromSequence: number
      liveSequence: number
      source: 'checkpoint' | 'tail'
      fromSequence: number
      throughSequence: number
      instantLineLimit: 10_000
    }
  | {
      type: 'terminal.replay-resize'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      sequence: number
      cols: number
      rows: number
    }
  | {
      type: 'terminal.replay-reset'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      sequence: number
      screenEpoch: number
    }
  | {
      type: 'terminal.replay-complete'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      throughSequence: number
    }
  | {
      type: 'terminal.checkpoint-stored'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      throughSequence: number
    }
  | {
      type: 'terminal.checkpoint-rejected'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      throughSequence: number
      reason: string
    }
  | {
      type: 'terminal.gap'
      protocolVersion: typeof PROTOCOL_VERSION
      sessionId: string
      requestedFromSequence: number
      availableFromSequence: number
      reason: 'retention' | 'corruption'
    }
  | {
      type: 'rpc.response'
      protocolVersion: typeof PROTOCOL_VERSION
      requestId: string
      runtimeGeneration: string
      result: unknown
    }
  | {
      type: 'rpc.error'
      protocolVersion: typeof PROTOCOL_VERSION
      requestId: string
      runtimeGeneration: string
      code: RpcErrorCode
      message: string
      retryable: boolean
    }
  | {
      type: 'events.batch'
      protocolVersion: typeof PROTOCOL_VERSION
      consumerId: string
      runtimeGeneration: string
      events: import('./domain-events').DomainEventWireEnvelope[]
      throughSequence: number
    }

export interface RuntimeConnectRequest {
  type: 'runtime.connect'
  protocolVersion: typeof PROTOCOL_VERSION
}
