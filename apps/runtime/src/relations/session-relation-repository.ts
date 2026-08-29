import type {
  DomainCommandMetadata,
  DomainCommit,
  RelationKind,
  Session,
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

interface SessionRow {
  id: string
  task_id: string
  execution_context_id: string
  kind: Session['kind']
  status: Session['status']
  title: string
  cwd: string
  created_at: number
  updated_at: number
  last_activity_at: number
  archived_at: number | null
  version: number
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

  appendStructuralRelation(
    command: DomainCommandMetadata,
    input: {
      id: string
      taskId: string
      childSessionId: string
      parentSessionId: string
      kind: 'derived-from' | 'forked-from'
      metadata: unknown
      now: number
    }
  ): DomainCommit<SessionRelation> {
    return this.create(command, {
      id: input.id,
      taskId: input.taskId,
      fromSessionId: input.childSessionId,
      toSessionId: input.parentSessionId,
      kind: input.kind,
      metadata: input.metadata,
      now: input.now
    })
  }

  getStructuralParent(sessionId: string): SessionRelation | undefined {
    const row = this.#database.get<CurrentRow>(
      `SELECT * FROM session_relations_current
       WHERE from_session_id = ?
         AND relation_kind IN ('derived-from', 'forked-from')`,
      sessionId
    )
    return row ? mapCurrent(row) : undefined
  }

  listStructuralChildren(
    parentSessionId: string,
    options: { includeArchived?: boolean } = {}
  ): SessionRelation[] {
    return this.#database.all<CurrentRow>(
      `SELECT relation.*
       FROM session_relations_current AS relation
       JOIN sessions ON sessions.id = relation.from_session_id
       WHERE relation.to_session_id = ?
         AND relation.relation_kind IN ('derived-from', 'forked-from')
         AND (? = 1 OR sessions.archived_at IS NULL)
       ORDER BY relation.created_at, relation.relation_id`,
      parentSessionId,
      options.includeArchived ? 1 : 0
    ).map(mapCurrent)
  }

  listSiblings(
    sessionId: string,
    options: { includeArchived?: boolean } = {}
  ): Session[] {
    const parent = this.getStructuralParent(sessionId)
    const parameters: Array<string | number> = []
    let structuralScope: string
    if (parent) {
      structuralScope = `EXISTS (
        SELECT 1 FROM session_relations_current AS sibling_relation
        WHERE sibling_relation.from_session_id = sessions.id
          AND sibling_relation.to_session_id = ?
          AND sibling_relation.relation_kind IN ('derived-from', 'forked-from')
      )`
      parameters.push(parent.toSessionId)
    } else {
      const sceneId = this.#resolveSceneId(this.#database, sessionId)
      if (!sceneId) return []
      structuralScope = `membership.scene_id = ? AND NOT EXISTS (
        SELECT 1 FROM session_relations_current AS sibling_relation
        WHERE sibling_relation.from_session_id = sessions.id
          AND sibling_relation.relation_kind IN ('derived-from', 'forked-from')
      )`
      parameters.push(sceneId)
    }
    parameters.push(sessionId, options.includeArchived ? 1 : 0)
    return this.#database.all<SessionRow>(
      `SELECT sessions.*
       FROM sessions
       JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
       WHERE ${structuralScope}
         AND sessions.id <> ?
         AND (? = 1 OR sessions.archived_at IS NULL)
       ORDER BY membership.last_user_interaction_seq DESC,
                membership.sibling_created_seq ASC,
                sessions.id ASC`,
      ...parameters
    ).map(mapSession)
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
    return this.listSiblings(sessionId).map(({ id, title }) => ({ id, title }))
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
    if (isStructuralKind(input.kind)) {
      const parent = tx.get<{ relation_id: string }>(
        `SELECT relation_id FROM session_relations_current
         WHERE from_session_id = ?
           AND relation_kind IN ('derived-from', 'forked-from')`,
        input.fromSessionId
      )
      if (parent) throw new Error(`Session ${input.fromSessionId} already has an active structural parent`)
      const fromSceneId = this.#resolveSceneId(tx, input.fromSessionId)
      const toSceneId = this.#resolveSceneId(tx, input.toSessionId)
      if (!fromSceneId || fromSceneId !== toSceneId) {
        throw new Error('structural relation endpoints must belong to the same Scene')
      }
      if (createsStructuralCycle(tx, input.fromSessionId, input.toSessionId)) {
        throw new Error('creating structural relation would introduce a cycle')
      }
    }
    if (
      input.kind === 'depends-on' &&
      createsCycle(tx, input.fromSessionId, input.toSessionId, input.kind)
    ) {
      throw new Error(`creating ${input.kind} would introduce a cycle`)
    }
  }

  #resolveSceneId(
    database: Pick<RuntimeDatabase, 'get'> | Pick<DatabaseTransaction, 'get'>,
    sessionId: string
  ): string | undefined {
    return database.get<{ scene_id: string }>(
      `SELECT scene_id FROM session_canvas_memberships WHERE session_id = ?
       UNION ALL
       SELECT scene_id FROM session_mounts WHERE session_id = ? ORDER BY scene_id LIMIT 1`,
      sessionId,
      sessionId
    )?.scene_id
  }
}

function isStructuralKind(kind: RelationKind): kind is 'derived-from' | 'forked-from' {
  return kind === 'derived-from' || kind === 'forked-from'
}

function createsStructuralCycle(
  tx: DatabaseTransaction,
  fromSessionId: string,
  toSessionId: string
): boolean {
  return Boolean(
    tx.get(
      `WITH RECURSIVE reachable(id) AS (
         SELECT to_session_id FROM session_relations_current
         WHERE from_session_id = ?
           AND relation_kind IN ('derived-from', 'forked-from')
         UNION
         SELECT relation.to_session_id
         FROM session_relations_current AS relation
         JOIN reachable ON relation.from_session_id = reachable.id
         WHERE relation.relation_kind IN ('derived-from', 'forked-from')
       )
       SELECT 1 AS found FROM reachable WHERE id = ? LIMIT 1`,
      toSessionId,
      fromSessionId
    )
  )
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

function mapSession(row: SessionRow): Session {
  return {
    id: row.id,
    taskId: row.task_id,
    executionContextId: row.execution_context_id,
    kind: row.kind,
    status: row.status,
    title: row.title,
    cwd: row.cwd,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    lastActivityAt: row.last_activity_at,
    ...(row.archived_at === null ? {} : { archivedAt: row.archived_at }),
    version: row.version
  }
}

function requireCurrent(row: CurrentRow | undefined): CurrentRow {
  if (!row) throw new Error('active SessionRelation does not exist')
  return row
}
