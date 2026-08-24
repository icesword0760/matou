import type { DomainEventEnvelope } from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'

interface StoredDomainEvent {
  seq: number
  event_id: string
  event_type: string
  aggregate_type: string
  aggregate_id: string
  workspace_id: string | null
  task_id: string | null
  session_id: string | null
  payload_json: string
  schema_version: number
  required_terminal_sequence: number | null
  command_id: string
  causation_id: string | null
  correlation_id: string | null
  occurred_at: number
}

export class DomainEventStore {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  readAfter(sequence: number, limit: number): DomainEventEnvelope[] {
    if (!Number.isSafeInteger(sequence) || sequence < 0) {
      throw new Error('event sequence must be a non-negative safe integer')
    }
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 1000) {
      throw new Error('event replay limit must be between 1 and 1000')
    }
    return this.#database
      .all<StoredDomainEvent>(
        'SELECT * FROM domain_events WHERE seq > ? ORDER BY seq LIMIT ?',
        sequence,
        limit
      )
      .map(decodeEvent)
  }

  readForConsumer(consumerId: string, limit: number): DomainEventEnvelope[] {
    return this.readAfter(this.cursor(consumerId), limit)
  }

  cursor(consumerId: string): number {
    return (
      this.#database.get<{ last_event_seq: number }>(
        'SELECT last_event_seq FROM consumer_cursors WHERE consumer_id = ?',
        consumerId
      )?.last_event_seq ?? 0
    )
  }

  acknowledge(consumerId: string, sequence: number, updatedAt = Date.now()): void {
    if (!Number.isSafeInteger(sequence) || sequence < 0) {
      throw new Error('event sequence must be a non-negative safe integer')
    }
    this.#database.run(
      `INSERT INTO consumer_cursors (consumer_id, last_event_seq, updated_at)
       VALUES (?, ?, ?)
       ON CONFLICT(consumer_id) DO UPDATE SET
         last_event_seq = MAX(consumer_cursors.last_event_seq, excluded.last_event_seq),
         updated_at = CASE
           WHEN excluded.last_event_seq >= consumer_cursors.last_event_seq THEN excluded.updated_at
           ELSE consumer_cursors.updated_at
         END`,
      consumerId,
      sequence,
      updatedAt
    )
  }

  lag(consumerId: string): number {
    const maximum = this.#database.get<{ maximum: number }>(
      'SELECT COALESCE(MAX(seq), 0) AS maximum FROM domain_events'
    )?.maximum ?? 0
    return Math.max(0, maximum - this.cursor(consumerId))
  }
}

function decodeEvent(row: StoredDomainEvent): DomainEventEnvelope {
  return {
    sequence: row.seq,
    eventId: row.event_id,
    eventType: row.event_type,
    aggregateType: row.aggregate_type,
    aggregateId: row.aggregate_id,
    ...(row.workspace_id === null ? {} : { workspaceId: row.workspace_id }),
    ...(row.task_id === null ? {} : { taskId: row.task_id }),
    ...(row.session_id === null ? {} : { sessionId: row.session_id }),
    payload: JSON.parse(row.payload_json) as unknown,
    schemaVersion: row.schema_version,
    ...(row.required_terminal_sequence === null
      ? {}
      : { requiredTerminalSequence: row.required_terminal_sequence }),
    commandId: row.command_id,
    ...(row.causation_id === null ? {} : { causationId: row.causation_id }),
    ...(row.correlation_id === null ? {} : { correlationId: row.correlation_id }),
    occurredAt: row.occurred_at
  }
}
