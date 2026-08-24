import { randomUUID } from 'node:crypto'

import type { DomainCommandMetadata } from '@matou/domain'

import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

interface DetachedRow {
  scene_window_id: string
  mount_id: string
  session_id: string
  scene_id: string
  task_id: string
  workspace_id: string
}

export interface DetachedSessionResult {
  sceneWindowId: string
  sessionId: string
  mountId: string
  sceneId: string
  state: 'attached' | 'detached'
}

export class DetachedSessionService {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  detach(command: DomainCommandMetadata, input: {
    mainWindowId: string
    sceneWindowId: string
    sceneId: string
    mountId: string
    sessionId: string
    nativeWindowKey: string
    now: number
  }): DetachedSessionResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const row = tx.get<{
        session_id: string; scene_id: string; scene_window_id: string | null
        task_id: string; workspace_id: string
      }>(
        `SELECT sm.session_id, sm.scene_id, sm.scene_window_id,
                s.task_id, t.workspace_id
         FROM session_mounts sm
         JOIN sessions s ON s.id = sm.session_id
         JOIN tasks t ON t.id = s.task_id
         WHERE sm.id = ? AND sm.scene_id = ? AND sm.session_id = ?
           AND s.archived_at IS NULL`,
        input.mountId, input.sceneId, input.sessionId
      )
      if (!row) throw new Error('Session mount does not exist')
      if (row.scene_window_id !== null) throw new Error('Session is already detached')
      if (!tx.get('SELECT id FROM app_windows WHERE id = ? AND state <> ?', input.mainWindowId, 'closed')) {
        throw new Error('Main window does not exist')
      }
      tx.run(
        `INSERT INTO app_windows (id, kind, state, created_at, updated_at)
         VALUES (?, 'detached-terminal', 'visible', ?, ?)`,
        input.sceneWindowId, input.now, input.now
      )
      tx.run(
        `INSERT INTO scene_windows (
           id, scene_id, native_window_key, state, created_at, updated_at
         ) VALUES (?, ?, ?, 'detached', ?, ?)`,
        input.sceneWindowId, input.sceneId, input.nativeWindowKey, input.now, input.now
      )
      tx.run(
        'UPDATE session_mounts SET scene_window_id = ? WHERE id = ?',
        input.sceneWindowId, input.mountId
      )
      emit({
        eventId: `${command.commandId}:session-detached`,
        eventType: 'scene.session-detached', aggregateType: 'scene',
        aggregateId: input.sceneId, workspaceId: row.workspace_id,
        taskId: row.task_id, sessionId: input.sessionId,
        payload: {
          sceneWindowId: input.sceneWindowId, mountId: input.mountId,
          nativeWindowKey: input.nativeWindowKey
        },
        occurredAt: input.now
      })
      return {
        sceneWindowId: input.sceneWindowId, sessionId: input.sessionId,
        mountId: input.mountId, sceneId: input.sceneId, state: 'detached' as const
      }
    }).result
  }

  returnSession(command: DomainCommandMetadata, input: {
    sceneWindowId: string
    mainWindowId: string
    now: number
  }): DetachedSessionResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const detached = requireDetached(tx.get<DetachedRow>(
        `SELECT sw.id AS scene_window_id, sm.id AS mount_id,
                sm.session_id, sm.scene_id, s.task_id, t.workspace_id
         FROM scene_windows sw
         JOIN session_mounts sm ON sm.scene_window_id = sw.id
         JOIN sessions s ON s.id = sm.session_id
         JOIN tasks t ON t.id = s.task_id
         WHERE sw.id = ? AND sw.state = 'detached'`,
        input.sceneWindowId
      ))
      const target = activeScene(tx, detached.scene_id)
        ?? focusedScene(tx, input.mainWindowId)
      if (!target) throw new Error('Return target Scene does not exist')

      if (target.scene_id !== detached.scene_id) {
        const nodeId = randomUUID()
        const ordinal = tx.get<{ count: number }>(
          'SELECT COUNT(*) AS count FROM session_mounts WHERE scene_id = ?', target.scene_id
        )?.count ?? 0
        tx.run(
          `INSERT INTO scene_nodes (
             id, scene_id, parent_node_id, kind, ordinal, created_at
           ) VALUES (?, ?, ?, 'mount', ?, ?)`,
          nodeId, target.scene_id, target.root_node_id, ordinal, input.now
        )
        tx.run(
          `UPDATE session_mounts
           SET scene_id = ?, scene_node_id = ?, scene_window_id = NULL
           WHERE id = ?`,
          target.scene_id, nodeId, detached.mount_id
        )
        if (target.task_id !== detached.task_id) {
          tx.run(
            'UPDATE sessions SET task_id = ?, updated_at = ?, version = version + 1 WHERE id = ?',
            target.task_id, input.now, detached.session_id
          )
        }
      } else {
        tx.run('UPDATE session_mounts SET scene_window_id = NULL WHERE id = ?', detached.mount_id)
      }
      tx.run("UPDATE scene_windows SET state = 'closed', updated_at = ? WHERE id = ?", input.now, input.sceneWindowId)
      tx.run("UPDATE app_windows SET state = 'closed', updated_at = ? WHERE id = ?", input.now, input.sceneWindowId)
      tx.run(
        `INSERT INTO window_scene_focus (window_id, scene_id, active_session_id, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(window_id, scene_id) DO UPDATE SET
           active_session_id = excluded.active_session_id,
           updated_at = excluded.updated_at`,
        input.mainWindowId, target.scene_id, detached.session_id, input.now
      )
      emit({
        eventId: `${command.commandId}:session-returned`,
        eventType: 'scene.session-returned', aggregateType: 'scene',
        aggregateId: target.scene_id, workspaceId: target.workspace_id,
        taskId: target.task_id, sessionId: detached.session_id,
        payload: {
          sceneWindowId: input.sceneWindowId, mountId: detached.mount_id,
          originalSceneId: detached.scene_id
        },
        occurredAt: input.now
      })
      return {
        sceneWindowId: input.sceneWindowId, sessionId: detached.session_id,
        mountId: detached.mount_id, sceneId: target.scene_id, state: 'attached' as const
      }
    }).result
  }

  normalizeOnStartup(now: number): string[] {
    const rows = this.#database.all<{ id: string; session_id: string }>(
      `SELECT sw.id, sm.session_id
       FROM scene_windows sw JOIN session_mounts sm ON sm.scene_window_id = sw.id
       WHERE sw.state = 'detached'`
    )
    this.#database.transaction((tx) => {
      for (const row of rows) {
        tx.run('UPDATE session_mounts SET scene_window_id = NULL WHERE scene_window_id = ?', row.id)
        tx.run("UPDATE scene_windows SET state = 'closed', updated_at = ? WHERE id = ?", now, row.id)
        tx.run("UPDATE app_windows SET state = 'closed', updated_at = ? WHERE id = ?", now, row.id)
      }
    })
    return rows.map(({ session_id }) => session_id)
  }
}

interface TargetScene {
  scene_id: string
  root_node_id: string
  task_id: string
  workspace_id: string
}

function activeScene(tx: DatabaseTransaction, sceneId: string): TargetScene | undefined {
  return tx.get<TargetScene>(
    `SELECT sc.id AS scene_id, sc.root_node_id, t.id AS task_id, t.workspace_id
     FROM scenes sc JOIN tasks t ON t.id = sc.task_id
     JOIN workspaces w ON w.id = t.workspace_id
     WHERE sc.id = ? AND sc.archived_at IS NULL
       AND t.archived_at IS NULL AND w.archived_at IS NULL`,
    sceneId
  )
}

function focusedScene(tx: DatabaseTransaction, windowId: string): TargetScene | undefined {
  return tx.get<TargetScene>(
    `SELECT sc.id AS scene_id, sc.root_node_id, t.id AS task_id, t.workspace_id
     FROM window_navigation wn
     JOIN window_workspace_focus wwf
       ON wwf.window_id = wn.window_id AND wwf.workspace_id = wn.active_workspace_id
     JOIN window_task_focus wtf
       ON wtf.window_id = wn.window_id AND wtf.task_id = wwf.active_task_id
     JOIN scenes sc ON sc.id = wtf.active_scene_id
     JOIN tasks t ON t.id = sc.task_id
     WHERE wn.window_id = ? AND sc.archived_at IS NULL AND t.archived_at IS NULL`,
    windowId
  )
}

function requireDetached(value: DetachedRow | undefined): DetachedRow {
  if (!value) throw new Error('Detached Session does not exist')
  return value
}
