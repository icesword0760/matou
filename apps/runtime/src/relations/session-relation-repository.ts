import type {
  DomainCommandMetadata,
  DomainCommit,
  RelationKind,
  SessionRelation
} from '@matou/domain'

import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

interface CurrentRow {
  relation_id: string
  task_id: string
  from_session_id: string
  to_session_id: string
  relation_kind: RelationKind
  metadata_json: string
  created_at: number
  updated_at: number
  source_event_sequence: number
}

interface HistoryRow {
  sequence: number
  event_id: string
  relation_id: string
  operation: RelationOperation
  task_id: string
  from_session_id: string
  to_session_id: string
  relation_kind: RelationKind
  metadata_json: string
  command_id: string
  occurred_at: number
}

export type RelationOperation = 'created' | 'revoked' | 'restored' | 'metadata-updated'

export interface RelationHistoryEntry {
  sequence: number
  eventId: string
  relationId: string
  operation: RelationOperation
  taskId: string
  fromSessionId: string
  toSessionId: string
  kind: RelationKind
  metadata: unknown
  commandId: string
  occurredAt: number
}

export class SessionRelationRepository {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  create(
    command: DomainCommandMetadata,
    input: {
      id: string
      taskId: string
      fromSessionId: string
      toSessionId: string
      kind: RelationKind
      metadata: unknown
      now: number
    }
  ): DomainCommit<SessionRelation> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      this.#validateNewEdge(tx, input)
      const insertion = tx.run(
        `INSERT INTO session_relation_events (
           event_id, relation_id, operation, task_id, from_session_id,
           to_session_id, relation_kind, metadata_json, command_id, occurred_at
         ) VALUES (?, ?, 'created', ?, ?, ?, ?, ?, ?, ?)`,
        `${command.commandId}:relation-created`,
        input.id,
        input.taskId,
        input.fromSessionId,
        input.toSessionId,
        input.kind,
        JSON.stringify(input.metadata),
        command.commandId,
        input.now
      )
      tx.run(
        `INSERT INTO session_relations_current (
           relation_id, task_id, from_session_id, to_session_id, relation_kind,
           metadata_json, created_at, updated_at, source_event_sequence
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        input.id,
        input.taskId,
        input.fromSessionId,
        input.toSessionId,
        input.kind,
        JSON.stringify(input.metadata),
        input.now,
        input.now,
        Number(insertion.lastInsertRowid)
      )
      const relation = mapCurrent(requireCurrent(tx.get<CurrentRow>(
        'SELECT * FROM session_relations_current WHERE relation_id = ?', input.id
      )))
      emit({
        eventId: `${command.commandId}:domain-relation-created`,
        eventType: 'session-relation.created',
        aggregateType: 'session-relation',
        aggregateId: input.id,
        taskId: input.taskId,
        sessionId: input.fromSessionId,
        payload: relation,
        occurredAt: input.now
      })
      return relation
    })
  }

  revoke(
    command: DomainCommandMetadata,
    relationId: string,
    now: number
  ): DomainCommit<SessionRelation> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const current = requireCurrent(tx.get<CurrentRow>(
        'SELECT * FROM session_relations_current WHERE relation_id = ?', relationId
      ))
      tx.run(
        `INSERT INTO session_relation_events (
           event_id, relation_id, operation, task_id, from_session_id,
           to_session_id, relation_kind, metadata_json, command_id, occurred_at
         ) VALUES (?, ?, 'revoked', ?, ?, ?, ?, ?, ?, ?)`,
        `${command.commandId}:relation-revoked`,
        relationId,
        current.task_id,
        current.from_session_id,
        current.to_session_id,
        current.relation_kind,
        current.metadata_json,
        command.commandId,
        now
      )
      tx.run('DELETE FROM session_relations_current WHERE relation_id = ?', relationId)
      const relation = mapCurrent(current)
      emit({
        eventId: `${command.commandId}:domain-relation-revoked`,
        eventType: 'session-relation.revoked',
        aggregateType: 'session-relation',
        aggregateId: relationId,
        taskId: current.task_id,
        sessionId: current.from_session_id,
        payload: relation,
        occurredAt: now
      })
      return relation
    })
  }

  restore(
    command: DomainCommandMetadata,
    relationId: string,
    now: number
  ): DomainCommit<SessionRelation> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      if (tx.get('SELECT relation_id FROM session_relations_current WHERE relation_id = ?', relationId)) {
        throw new Error(`Relation ${relationId} is already active`)
      }
      const original = tx.get<HistoryRow>(
        `SELECT * FROM session_relation_events
         WHERE relation_id = ? AND operation IN ('created', 'restored')
         ORDER BY sequence DESC LIMIT 1`,
        relationId
      )
      if (!original) throw new Error(`Relation ${relationId} has no restorable history`)
      this.#validateNewEdge(tx, {
        id: relationId,
        taskId: original.task_id,
        fromSessionId: original.from_session_id,
        toSessionId: original.to_session_id,
        kind: original.relation_kind
      })
      const insertion = tx.run(
        `INSERT INTO session_relation_events (
           event_id, relation_id, operation, task_id, from_session_id,
           to_session_id, relation_kind, metadata_json, command_id, occurred_at
         ) VALUES (?, ?, 'restored', ?, ?, ?, ?, ?, ?, ?)`,
        `${command.commandId}:relation-restored`,
        relationId,
        original.task_id,
        original.from_session_id,
        original.to_session_id,
        original.relation_kind,
        original.metadata_json,
        command.commandId,
        now
      )
      const createdAt =
        tx.get<{ occurred_at: number }>(
          `SELECT occurred_at FROM session_relation_events
           WHERE relation_id = ? AND operation = 'created' ORDER BY sequence LIMIT 1`,
          relationId
        )?.occurred_at ?? now
      tx.run(
        `INSERT INTO session_relations_current (
           relation_id, task_id, from_session_id, to_session_id, relation_kind,
           metadata_json, created_at, updated_at, source_event_sequence
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        relationId,
        original.task_id,
        original.from_session_id,
        original.to_session_id,
        original.relation_kind,
        original.metadata_json,
        createdAt,
        now,
        Number(insertion.lastInsertRowid)
      )
      const relation = mapCurrent(requireCurrent(tx.get<CurrentRow>(
        'SELECT * FROM session_relations_current WHERE relation_id = ?', relationId
      )))
      emit({
        eventId: `${command.commandId}:domain-relation-restored`,
        eventType: 'session-relation.restored',
        aggregateType: 'session-relation',
        aggregateId: relationId,
        taskId: original.task_id,
        sessionId: original.from_session_id,
        payload: relation,
        occurredAt: now
      })
      return relation
    })
  }

  getCurrent(relationId: string): SessionRelation | undefined {
    const row = this.#database.get<CurrentRow>(
      'SELECT * FROM session_relations_current WHERE relation_id = ?', relationId
    )
    return row ? mapCurrent(row) : undefined
  }

  history(relationId: string): RelationHistoryEntry[] {
    return this.#database
      .all<HistoryRow>(
        'SELECT * FROM session_relation_events WHERE relation_id = ? ORDER BY sequence',
        relationId
      )
      .map((row) => ({
        sequence: row.sequence,
        eventId: row.event_id,
        relationId: row.relation_id,
        operation: row.operation,
        taskId: row.task_id,
        fromSessionId: row.from_session_id,
        toSessionId: row.to_session_id,
        kind: row.relation_kind,
        metadata: JSON.parse(row.metadata_json) as unknown,
        commandId: row.command_id,
        occurredAt: row.occurred_at
      }))
  }

  deriveSiblings(sessionId: string): Array<{ id: string; title: string }> {
    const parent = this.#database.get<{ to_session_id: string }>(
      `SELECT to_session_id FROM session_relations_current
       WHERE from_session_id = ? AND relation_kind = 'forked-from'`,
      sessionId
    )
    if (!parent) return []
    return this.#database.all<{ id: string; title: string }>(
      `SELECT sessions.id, sessions.title
       FROM session_relations_current
       JOIN sessions ON sessions.id = session_relations_current.from_session_id
       WHERE session_relations_current.to_session_id = ?
         AND session_relations_current.relation_kind = 'forked-from'
         AND session_relations_current.from_session_id <> ?
       ORDER BY session_relations_current.created_at, sessions.id`,
      parent.to_session_id,
      sessionId
    )
  }

  #validateNewEdge(
    tx: DatabaseTransaction,
    input: {
      id: string
      taskId: string
      fromSessionId: string
      toSessionId: string
      kind: RelationKind
    }
  ): void {
    if (input.fromSessionId === input.toSessionId) throw new Error('relation endpoints must differ')
    const endpoints = tx.all<{ id: string; task_id: string }>(
      'SELECT id, task_id FROM sessions WHERE id IN (?, ?)',
      input.fromSessionId,
      input.toSessionId
    )
    if (
      endpoints.length !== 2 ||
      endpoints.some(({ task_id }) => task_id !== input.taskId)
    ) {
      throw new Error('relation endpoints must exist in the same Task')
    }
    if (input.kind === 'forked-from') {
      const parent = tx.get<{ relation_id: string }>(
        `SELECT relation_id FROM session_relations_current
         WHERE from_session_id = ? AND relation_kind = 'forked-from'`,
        input.fromSessionId
      )
      if (parent) throw new Error(`Session ${input.fromSessionId} already has an active fork parent`)
    }
    if (
      (input.kind === 'forked-from' || input.kind === 'depends-on') &&
      createsCycle(tx, input.fromSessionId, input.toSessionId, input.kind)
    ) {
      throw new Error(`creating ${input.kind} would introduce a cycle`)
    }
  }
}

function createsCycle(
  tx: DatabaseTransaction,
  fromSessionId: string,
  toSessionId: string,
  kind: 'forked-from' | 'depends-on'
): boolean {
  return Boolean(
    tx.get(
      `WITH RECURSIVE reachable(id) AS (
         SELECT to_session_id FROM session_relations_current
         WHERE from_session_id = ? AND relation_kind = ?
         UNION
         SELECT relation.to_session_id
         FROM session_relations_current AS relation
         JOIN reachable ON relation.from_session_id = reachable.id
         WHERE relation.relation_kind = ?
       )
       SELECT 1 AS found FROM reachable WHERE id = ? LIMIT 1`,
      toSessionId,
      kind,
      kind,
      fromSessionId
    )
  )
}

function mapCurrent(row: CurrentRow): SessionRelation {
  return {
    id: row.relation_id,
    taskId: row.task_id,
    fromSessionId: row.from_session_id,
    toSessionId: row.to_session_id,
    kind: row.relation_kind,
    metadata: JSON.parse(row.metadata_json) as unknown,
    createdAt: row.created_at,
    updatedAt: row.updated_at
  }
}

function requireCurrent(row: CurrentRow | undefined): CurrentRow {
  if (!row) throw new Error('active SessionRelation does not exist')
  return row
}
