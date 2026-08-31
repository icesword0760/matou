import type { CommandId, EventId, SessionId, TaskId, WorkspaceId } from './model'

export interface DomainEventInput {
  eventId: EventId
  eventType: string
  aggregateType: string
  aggregateId: string
  workspaceId?: WorkspaceId
  taskId?: TaskId
  sessionId?: SessionId
  payload: unknown
  schemaVersion?: number
  requiredTerminalSequence?: number
  occurredAt: number
}

export interface DomainEventEnvelope extends Omit<DomainEventInput, 'schemaVersion'> {
  sequence: number
  schemaVersion: number
  commandId: CommandId
  causationId?: string
  correlationId?: string
}

export interface DomainCommandMetadata {
  commandId: CommandId
  commandType: string
  requestHash: string
  causationId?: string
  correlationId?: string
}

export interface DomainCommit<T> {
  result: T
  firstEventSequence?: number
  lastEventSequence?: number
  replayed: boolean
}

export type SessionGraphEventType =
  | 'scene.canvas-created'
  | 'session.canvas-membership-created'
  | 'session.structural-relation-created'
  | 'session.user-interacted'
  | 'session.mode-changed'
  | 'session.restore-state-changed'
  | 'session.graph-summary-changed'
  | 'session.stopped-state-changed'

export type AgentSemanticKind =
  | 'agent.message'
  | 'agent.todo'
  | 'agent.tool-started'
  | 'agent.tool-finished'
  | 'agent.permission-requested'
  | 'file.changed'
  | 'artifact.observed'
  | 'validation.status-changed'

export interface AgentSemanticEvent {
  eventId: EventId
  sessionId: SessionId
  kind: AgentSemanticKind
  provider: 'claude-code' | 'codex' | 'generic'
  sourceRef: {
    providerEventId: string
    source: 'structured' | 'transcript' | 'terminal-marker' | 'terminal-parse'
  }
  confidence: 'high' | 'medium' | 'low'
  payload: unknown
  occurredAt: number
}
