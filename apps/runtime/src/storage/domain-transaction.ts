import type {
  DomainCommandMetadata,
  DomainCommit,
  DomainEventInput
} from '@matou/domain'

import type { DatabaseTransaction, RuntimeDatabase } from './database'

export interface DomainMutationContext {
  tx: DatabaseTransaction
  emit(event: DomainEventInput): void
}

interface StoredCommand {
  request_hash: string
  response_json: string
  first_event_seq: number | null
  last_event_seq: number | null
}

export class DomainTransactionManager {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  execute<T>(
    command: DomainCommandMetadata,
    mutate: (context: DomainMutationContext) => T
  ): DomainCommit<T> {
    const stored = this.#database.get<StoredCommand>(
      `SELECT request_hash, response_json, first_event_seq, last_event_seq
       FROM command_deduplication WHERE command_id = ?`,
      command.commandId
    )
    if (stored) {
      if (stored.request_hash !== command.requestHash) {
        throw new Error(
          `command id ${command.commandId} was already used for a different request`
        )
      }
      return compactCommit(
        JSON.parse(stored.response_json) as T,
        stored.first_event_seq ?? undefined,
        stored.last_event_seq ?? undefined,
        true
      )
    }

    return this.#database.transaction((tx) => {
      const events: DomainEventInput[] = []
      const result = mutate({ tx, emit: (event) => events.push(event) })
      if (isPromiseLike(result)) {
        throw new Error('domain transaction mutations must be synchronous')
      }

      let firstEventSequence: number | undefined
      let lastEventSequence: number | undefined
      for (const event of events) {
        const insertion = tx.run(
          `INSERT INTO domain_events (
             event_id, event_type, aggregate_type, aggregate_id,
             workspace_id, task_id, session_id, payload_json, schema_version,
             required_terminal_sequence, command_id, causation_id, correlation_id, occurred_at
           ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          event.eventId,
          event.eventType,
          event.aggregateType,
          event.aggregateId,
          event.workspaceId ?? null,
          event.taskId ?? null,
          event.sessionId ?? null,
          JSON.stringify(event.payload),
          event.schemaVersion ?? 1,
          event.requiredTerminalSequence ?? null,
          command.commandId,
          command.causationId ?? null,
          command.correlationId ?? null,
          event.occurredAt
        )
        const sequence = Number(insertion.lastInsertRowid)
        firstEventSequence ??= sequence
        lastEventSequence = sequence
      }

      const responseJson = JSON.stringify(result)
      if (responseJson === undefined) {
        throw new Error('domain transaction result must be JSON serializable')
      }
      tx.run(
        `INSERT INTO command_deduplication (
           command_id, command_type, request_hash, response_json,
           first_event_seq, last_event_seq, committed_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?)`,
        command.commandId,
        command.commandType,
        command.requestHash,
        responseJson,
        firstEventSequence ?? null,
        lastEventSequence ?? null,
        Date.now()
      )

      return compactCommit(result, firstEventSequence, lastEventSequence, false)
    })
  }
}

function compactCommit<T>(
  result: T,
  firstEventSequence: number | undefined,
  lastEventSequence: number | undefined,
  replayed: boolean
): DomainCommit<T> {
  return {
    result,
    ...(firstEventSequence === undefined ? {} : { firstEventSequence }),
    ...(lastEventSequence === undefined ? {} : { lastEventSequence }),
    replayed
  }
}

function isPromiseLike(value: unknown): value is PromiseLike<unknown> {
  return (
    (typeof value === 'object' || typeof value === 'function') &&
    value !== null &&
    'then' in value &&
    typeof (value as { then?: unknown }).then === 'function'
  )
}
