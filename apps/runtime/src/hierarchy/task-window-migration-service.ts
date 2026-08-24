import type { DomainCommandMetadata } from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

export interface TaskWindowMigration {
  id: string
  taskId: string
  sourceWindowId: string
  targetWindowId: string
  state: 'preparing' | 'committed' | 'failed'
  failureReason?: string
  createdAt: number
  updatedAt: number
}

interface MigrationRow {
  id: string
  task_id: string
  source_window_id: string
  target_window_id: string
  state: TaskWindowMigration['state']
  failure_reason: string | null
  created_at: number
  updated_at: number
}

export class TaskWindowMigrationService {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  prepare(command: DomainCommandMetadata, input: {
    migrationId: string
    taskId: string
    sourceWindowId: string
    targetWindowId: string
    now: number
  }): TaskWindowMigration {
    if (input.sourceWindowId === input.targetWindowId) {
      throw new Error('Task migration target must differ from source')
    }
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const placement = tx.get<{ window_id: string; workspace_id: string }>(
        `SELECT p.window_id, t.workspace_id
         FROM window_task_placements p JOIN tasks t ON t.id = p.task_id
         WHERE p.task_id = ? AND t.archived_at IS NULL`, input.taskId
      )
      if (!placement || placement.window_id !== input.sourceWindowId) {
        throw new Error('Task source placement is stale')
      }
      const target = tx.get<{ id: string }>(
        `SELECT id FROM app_windows
         WHERE id = ? AND kind = 'main' AND state <> 'closed'`, input.targetWindowId
      )
      if (!target) throw new Error('Target main window does not exist')
      if (tx.get(
        "SELECT id FROM task_window_migrations WHERE task_id = ? AND state = 'preparing'",
        input.taskId
      )) throw new Error('Task migration already overlaps an active migration')
      tx.run(
        `INSERT INTO task_window_migrations (
           id, task_id, source_window_id, target_window_id, state, created_at, updated_at
         ) VALUES (?, ?, ?, ?, 'preparing', ?, ?)`,
        input.migrationId, input.taskId, input.sourceWindowId,
        input.targetWindowId, input.now, input.now
      )
      emit({
        eventId: `${command.commandId}:task-window-migration-prepared`,
        eventType: 'task.window-migration-prepared', aggregateType: 'task',
        aggregateId: input.taskId, workspaceId: placement.workspace_id,
        taskId: input.taskId, payload: {
          migrationId: input.migrationId, sourceWindowId: input.sourceWindowId,
          targetWindowId: input.targetWindowId
        }, occurredAt: input.now
      })
      return mapMigration(tx.get<MigrationRow>(
        'SELECT * FROM task_window_migrations WHERE id = ?', input.migrationId
      )!)
    }).result
  }

  acknowledgeTarget(command: DomainCommandMetadata, input: {
    migrationId: string
    now: number
  }): TaskWindowMigration {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requirePreparing(tx.get<MigrationRow>(
        'SELECT * FROM task_window_migrations WHERE id = ?', input.migrationId
      ))
      const task = tx.get<{ workspace_id: string }>(
        'SELECT workspace_id FROM tasks WHERE id = ? AND archived_at IS NULL', before.task_id
      )
      if (!task) throw new Error('Migrating Task does not exist')
      if (!tx.get(
        `SELECT id FROM app_windows WHERE id = ? AND kind = 'main' AND state <> 'closed'`,
        before.target_window_id
      )) throw new Error('Target main window does not exist')
      tx.run(
        'UPDATE window_task_placements SET window_id = ?, updated_at = ? WHERE task_id = ?',
        before.target_window_id, input.now, before.task_id
      )
      const scene = tx.get<{ id: string }>(
        `SELECT id FROM scenes WHERE task_id = ? AND archived_at IS NULL
         ORDER BY sort_key, created_at, id LIMIT 1`, before.task_id
      )
      const session = scene ? tx.get<{ id: string }>(
        `SELECT s.id FROM session_mounts sm JOIN sessions s ON s.id = sm.session_id
         WHERE sm.scene_id = ? AND s.archived_at IS NULL
         ORDER BY sm.created_at, sm.id LIMIT 1`, scene.id
      ) : undefined
      tx.run(
        `INSERT INTO window_workspace_focus (window_id, workspace_id, active_task_id, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(window_id, workspace_id) DO UPDATE SET
           active_task_id = excluded.active_task_id, updated_at = excluded.updated_at`,
        before.target_window_id, task.workspace_id, before.task_id, input.now
      )
      if (scene) {
        tx.run(
          `INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at)
           VALUES (?, ?, ?, ?)
           ON CONFLICT(window_id, task_id) DO UPDATE SET
             active_scene_id = excluded.active_scene_id, updated_at = excluded.updated_at`,
          before.target_window_id, before.task_id, scene.id, input.now
        )
      }
      if (scene && session) {
        tx.run(
          `INSERT INTO window_scene_focus (window_id, scene_id, active_session_id, updated_at)
           VALUES (?, ?, ?, ?)
           ON CONFLICT(window_id, scene_id) DO UPDATE SET
             active_session_id = excluded.active_session_id, updated_at = excluded.updated_at`,
          before.target_window_id, scene.id, session.id, input.now
        )
      }
      tx.run(
        "UPDATE task_window_migrations SET state = 'committed', updated_at = ? WHERE id = ?",
        input.now, input.migrationId
      )
      emit({
        eventId: `${command.commandId}:task-window-migration-committed`,
        eventType: 'task.window-migration-committed', aggregateType: 'task',
        aggregateId: before.task_id, workspaceId: task.workspace_id,
        taskId: before.task_id, payload: {
          migrationId: before.id, sourceWindowId: before.source_window_id,
          targetWindowId: before.target_window_id
        }, occurredAt: input.now
      })
      return mapMigration({ ...before, state: 'committed', updated_at: input.now })
    }).result
  }

  fail(command: DomainCommandMetadata, input: {
    migrationId: string
    reason: string
    now: number
  }): TaskWindowMigration {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = tx.get<MigrationRow>(
        'SELECT * FROM task_window_migrations WHERE id = ?', input.migrationId
      )
      if (!before) throw new Error('Task migration does not exist')
      if (before.state === 'committed') {
        tx.run(
          'UPDATE window_task_placements SET window_id = ?, updated_at = ? WHERE task_id = ?',
          before.source_window_id, input.now, before.task_id
        )
      }
      tx.run(
        `UPDATE task_window_migrations
         SET state = 'failed', failure_reason = ?, updated_at = ? WHERE id = ?`,
        input.reason, input.now, input.migrationId
      )
      const workspace = tx.get<{ workspace_id: string }>(
        'SELECT workspace_id FROM tasks WHERE id = ?', before.task_id
      )
      if (!workspace) throw new Error('Migrating Task does not exist')
      emit({
        eventId: `${command.commandId}:task-window-migration-failed`,
        eventType: 'task.window-migration-failed', aggregateType: 'task',
        aggregateId: before.task_id, workspaceId: workspace.workspace_id,
        taskId: before.task_id, payload: {
          migrationId: before.id, reason: input.reason,
          restoredWindowId: before.source_window_id
        }, occurredAt: input.now
      })
      return mapMigration({
        ...before, state: 'failed', failure_reason: input.reason, updated_at: input.now
      })
    }).result
  }

  get(migrationId: string): TaskWindowMigration | undefined {
    const row = this.#database.get<MigrationRow>(
      'SELECT * FROM task_window_migrations WHERE id = ?', migrationId
    )
    return row ? mapMigration(row) : undefined
  }
}

function requirePreparing(row: MigrationRow | undefined): MigrationRow {
  if (!row) throw new Error('Task migration does not exist')
  if (row.state !== 'preparing') throw new Error('Task migration is not awaiting target acknowledgement')
  return row
}
function mapMigration(row: MigrationRow): TaskWindowMigration {
  return {
    id: row.id, taskId: row.task_id,
    sourceWindowId: row.source_window_id, targetWindowId: row.target_window_id,
    state: row.state,
    ...(row.failure_reason === null ? {} : { failureReason: row.failure_reason }),
    createdAt: row.created_at, updatedAt: row.updated_at
  }
}
