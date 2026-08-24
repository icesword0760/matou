import type {
  Anchor,
  Annotation,
  Artifact,
  DomainCommandMetadata,
  DomainCommit,
  ValidationRun
} from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

interface AnnotationRow {
  id: string; task_id: string; session_id: string; kind: string; text_snapshot: string
  anchor_json: string; status: Annotation['status']; created_at: number; updated_at: number
}
interface ArtifactRow {
  id: string; task_id: string; producer_session_id: string | null; path_identity: string
  media_type: string | null; state: Artifact['state']; metadata_json: string
  created_at: number; updated_at: number
}
interface ValidationRow {
  id: string; task_id: string; session_id: string | null; check_id: string
  status: ValidationRun['status']; summary_json: string; started_at: number | null
  ended_at: number | null; created_at: number
}

export class ProductFoundationRepository {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  createAnnotation(
    command: DomainCommandMetadata,
    input: {
      id: string; taskId: string; sessionId: string; kind: string
      textSnapshot: string; anchor: Anchor; now: number
    }
  ): DomainCommit<Annotation> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const session = tx.get<{ task_id: string }>('SELECT task_id FROM sessions WHERE id = ?', input.sessionId)
      if (!session || session.task_id !== input.taskId || input.anchor.sessionId !== input.sessionId) {
        throw new Error('Annotation Session, Task, and anchor must refer to the same Session')
      }
      tx.run(
        `INSERT INTO annotations (
           id, task_id, session_id, kind, text_snapshot, anchor_json,
           status, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?)`,
        input.id, input.taskId, input.sessionId, input.kind, input.textSnapshot,
        JSON.stringify(input.anchor), input.now, input.now
      )
      const annotation = mapAnnotation(requireRow(tx.get<AnnotationRow>('SELECT * FROM annotations WHERE id = ?', input.id), 'Annotation'))
      emitFoundation(emit, command.commandId, 'annotation.created', 'annotation', input.id, input.taskId, input.sessionId, annotation, input.now)
      return annotation
    })
  }

  updateAnnotationStatus(
    command: DomainCommandMetadata,
    id: string,
    status: Annotation['status'],
    now: number
  ): DomainCommit<Annotation> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<AnnotationRow>('SELECT * FROM annotations WHERE id = ?', id), 'Annotation')
      tx.run('UPDATE annotations SET status = ?, updated_at = ? WHERE id = ?', status, now, id)
      const annotation = mapAnnotation({ ...before, status, updated_at: now })
      emitFoundation(emit, command.commandId, 'annotation.status-changed', 'annotation', id, before.task_id, before.session_id, annotation, now)
      return annotation
    })
  }

  observeArtifact(
    command: DomainCommandMetadata,
    input: {
      id: string; taskId: string; producerSessionId?: string; pathIdentity: string
      mediaType?: string; state: Artifact['state']; metadata: unknown; now: number
    }
  ): DomainCommit<Artifact> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const existing = tx.get<ArtifactRow>(
        'SELECT * FROM artifacts WHERE task_id = ? AND path_identity = ?',
        input.taskId, input.pathIdentity
      )
      const id = existing?.id ?? input.id
      if (existing) {
        tx.run(
          `UPDATE artifacts SET producer_session_id = ?, media_type = ?, state = ?,
           metadata_json = ?, updated_at = ? WHERE id = ?`,
          input.producerSessionId ?? existing.producer_session_id,
          input.mediaType ?? existing.media_type,
          input.state,
          JSON.stringify(input.metadata),
          input.now,
          id
        )
      } else {
        tx.run(
          `INSERT INTO artifacts (
             id, task_id, producer_session_id, path_identity, media_type,
             state, metadata_json, created_at, updated_at
           ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          id, input.taskId, input.producerSessionId ?? null, input.pathIdentity,
          input.mediaType ?? null, input.state, JSON.stringify(input.metadata), input.now, input.now
        )
      }
      const artifact = mapArtifact(requireRow(tx.get<ArtifactRow>('SELECT * FROM artifacts WHERE id = ?', id), 'Artifact'))
      emitFoundation(
        emit, command.commandId, existing ? 'artifact.updated' : 'artifact.observed',
        'artifact', id, input.taskId, input.producerSessionId, artifact, input.now
      )
      return artifact
    })
  }

  createValidation(
    command: DomainCommandMetadata,
    input: {
      id: string; taskId: string; sessionId?: string; checkId: string
      status: ValidationRun['status']; now: number
    }
  ): DomainCommit<ValidationRun> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      tx.run(
        `INSERT INTO validation_runs (
           id, task_id, session_id, check_id, status, summary_json,
           started_at, created_at
         ) VALUES (?, ?, ?, ?, ?, '{}', ?, ?)`,
        input.id, input.taskId, input.sessionId ?? null, input.checkId, input.status,
        input.status === 'running' ? input.now : null, input.now
      )
      const validation = mapValidation(requireRow(tx.get<ValidationRow>('SELECT * FROM validation_runs WHERE id = ?', input.id), 'ValidationRun'))
      emitFoundation(emit, command.commandId, 'validation.created', 'validation', input.id, input.taskId, input.sessionId, validation, input.now)
      return validation
    })
  }

  updateValidation(
    command: DomainCommandMetadata,
    id: string,
    status: ValidationRun['status'],
    summary: unknown,
    now: number
  ): DomainCommit<ValidationRun> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<ValidationRow>('SELECT * FROM validation_runs WHERE id = ?', id), 'ValidationRun')
      const terminal = ['passed', 'failed', 'cancelled', 'error'].includes(status)
      tx.run(
        `UPDATE validation_runs SET status = ?, summary_json = ?,
         started_at = COALESCE(started_at, ?), ended_at = ? WHERE id = ?`,
        status, JSON.stringify(summary), status === 'running' ? now : null,
        terminal ? now : null, id
      )
      const validation = mapValidation(requireRow(tx.get<ValidationRow>('SELECT * FROM validation_runs WHERE id = ?', id), 'ValidationRun'))
      emitFoundation(emit, command.commandId, 'validation.status-changed', 'validation', id, before.task_id, before.session_id ?? undefined, validation, now)
      return validation
    })
  }
}

export type TaskTelemetryEvent =
  | { kind: 'status'; taskId: string; key: string; value: string | null }
  | { kind: 'progress'; taskId: string; progress: number; label?: string }
  | { kind: 'log'; taskId: string; id: number; level: TaskLogLevel; source: string; message: string }

type TaskLogLevel = 'debug' | 'info' | 'warn' | 'error'

export class TaskTelemetryRepository {
  readonly #database: RuntimeDatabase
  readonly #generation: string
  readonly #maxLogsPerTask: number
  readonly #listeners = new Set<(event: TaskTelemetryEvent) => void>()

  constructor(database: RuntimeDatabase, runtimeGeneration: string, options: { maxLogsPerTask?: number } = {}) {
    this.#database = database
    this.#generation = runtimeGeneration
    this.#maxLogsPerTask = options.maxLogsPerTask ?? 10_000
  }

  subscribe(listener: (event: TaskTelemetryEvent) => void): () => void {
    this.#listeners.add(listener)
    return () => this.#listeners.delete(listener)
  }

  setStatus(taskId: string, key: string, value: string | null, now = Date.now()): void {
    if (value === null) {
      this.#database.run('DELETE FROM task_status_entries WHERE task_id = ? AND key = ?', taskId, key)
    } else {
      this.#database.run(
        `INSERT INTO task_status_entries (task_id, key, value, runtime_generation, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(task_id, key) DO UPDATE SET value = excluded.value,
           runtime_generation = excluded.runtime_generation, updated_at = excluded.updated_at`,
        taskId, key, value, this.#generation, now
      )
    }
    this.#emit({ kind: 'status', taskId, key, value })
  }

  setProgress(taskId: string, progress: number, label?: string, now = Date.now()): void {
    if (!Number.isFinite(progress) || progress < 0 || progress > 100) {
      throw new Error('Task progress must be between 0 and 100')
    }
    this.#database.run(
      `INSERT INTO task_progress (task_id, progress, label, runtime_generation, updated_at)
       VALUES (?, ?, ?, ?, ?)
       ON CONFLICT(task_id) DO UPDATE SET progress = excluded.progress, label = excluded.label,
         runtime_generation = excluded.runtime_generation, updated_at = excluded.updated_at`,
      taskId, progress, label ?? null, this.#generation, now
    )
    this.#emit({ kind: 'progress', taskId, progress, ...(label === undefined ? {} : { label }) })
  }

  appendLog(taskId: string, level: TaskLogLevel, source: string, message: string, now = Date.now()): number {
    if (Buffer.byteLength(message) > 16 * 1024) throw new Error('Task log message exceeds 16 KiB')
    const result = this.#database.run(
      `INSERT INTO task_logs (task_id, level, source, message, runtime_generation, created_at)
       VALUES (?, ?, ?, ?, ?, ?)`,
      taskId, level, source, message, this.#generation, now
    )
    const id = Number(result.lastInsertRowid)
    this.#database.run(
      `DELETE FROM task_logs WHERE task_id = ? AND id NOT IN (
         SELECT id FROM task_logs WHERE task_id = ? ORDER BY id DESC LIMIT ?
       )`,
      taskId, taskId, this.#maxLogsPerTask
    )
    this.#emit({ kind: 'log', taskId, id, level, source, message })
    return id
  }

  snapshot(taskId: string): {
    status: Record<string, string>
    progress: { progress: number; label?: string } | undefined
    logs: Array<{ id: number; level: TaskLogLevel; source: string; message: string; createdAt: number }>
  } {
    const status = Object.fromEntries(
      this.#database.all<{ key: string; value: string }>(
        'SELECT key, value FROM task_status_entries WHERE task_id = ? AND runtime_generation = ?',
        taskId, this.#generation
      ).map(({ key, value }) => [key, value])
    )
    const progressRow = this.#database.get<{ progress: number; label: string | null }>(
      'SELECT progress, label FROM task_progress WHERE task_id = ? AND runtime_generation = ?',
      taskId, this.#generation
    )
    const logs = this.#database.all<{
      id: number; level: TaskLogLevel; source: string; message: string; created_at: number
    }>(
      'SELECT id, level, source, message, created_at FROM task_logs WHERE task_id = ? AND runtime_generation = ? ORDER BY id',
      taskId, this.#generation
    ).map((row) => ({ id: row.id, level: row.level, source: row.source, message: row.message, createdAt: row.created_at }))
    return {
      status,
      progress: progressRow
        ? { progress: progressRow.progress, ...(progressRow.label === null ? {} : { label: progressRow.label }) }
        : undefined,
      logs
    }
  }

  purgeStaleGenerations(): number {
    let removed = 0
    this.#database.transaction((tx) => {
      for (const table of ['task_status_entries', 'task_progress', 'task_logs']) {
        removed += Number(tx.run(
          `DELETE FROM ${table} WHERE runtime_generation <> ?`, this.#generation
        ).changes)
      }
    })
    return removed
  }

  #emit(event: TaskTelemetryEvent): void {
    for (const listener of this.#listeners) listener(event)
  }
}

function emitFoundation(
  emit: Parameters<Parameters<DomainTransactionManager['execute']>[1]>[0]['emit'],
  eventId: string,
  eventType: string,
  aggregateType: string,
  aggregateId: string,
  taskId: string,
  sessionId: string | undefined,
  payload: unknown,
  occurredAt: number
): void {
  emit({
    eventId, eventType, aggregateType, aggregateId, taskId,
    ...(sessionId === undefined ? {} : { sessionId }), payload, occurredAt
  })
}

function mapAnnotation(row: AnnotationRow): Annotation {
  return {
    id: row.id, taskId: row.task_id, sessionId: row.session_id, kind: row.kind,
    textSnapshot: row.text_snapshot, anchor: JSON.parse(row.anchor_json) as Anchor,
    status: row.status, createdAt: row.created_at, updatedAt: row.updated_at
  }
}
function mapArtifact(row: ArtifactRow): Artifact {
  return {
    id: row.id, taskId: row.task_id,
    ...(row.producer_session_id === null ? {} : { producerSessionId: row.producer_session_id }),
    pathIdentity: row.path_identity,
    ...(row.media_type === null ? {} : { mediaType: row.media_type }),
    state: row.state, metadata: JSON.parse(row.metadata_json) as unknown,
    createdAt: row.created_at, updatedAt: row.updated_at
  }
}
function mapValidation(row: ValidationRow): ValidationRun {
  return {
    id: row.id, taskId: row.task_id,
    ...(row.session_id === null ? {} : { sessionId: row.session_id }),
    checkId: row.check_id, status: row.status,
    summary: JSON.parse(row.summary_json) as unknown,
    ...(row.started_at === null ? {} : { startedAt: row.started_at }),
    ...(row.ended_at === null ? {} : { endedAt: row.ended_at }),
    createdAt: row.created_at
  }
}
function requireRow<T>(row: T | undefined, label: string): T {
  if (!row) throw new Error(`${label} does not exist`)
  return row
}
