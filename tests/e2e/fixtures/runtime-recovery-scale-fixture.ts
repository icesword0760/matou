import { join } from 'node:path'

import { RuntimeDatabase } from '../../../apps/runtime/src/storage/database'
import { seedScaleDatabase } from '../scale/scale-database'

export const RECOVERY_SCALE_FOREGROUND_IDS = sessionIds('scale-sibling', 16)
export const RECOVERY_SCALE_BACKGROUND_IDS = sessionIds(
  'scale-catalog-00002-00001', 4
)
export const RECOVERY_SCALE_SESSION_IDS = [
  ...RECOVERY_SCALE_FOREGROUND_IDS,
  ...RECOVERY_SCALE_BACKGROUND_IDS
]
export const RECOVERY_SCALE_ACTIVE_SESSION_ID = RECOVERY_SCALE_FOREGROUND_IDS[0]!
export const RECOVERY_SCALE_BACKGROUND_WORKSPACE_ID = 'scale-workspace-00002'
export const RECOVERY_SCALE_BACKGROUND_TASK_ID = 'scale-task-00002-00001'

const DATABASE_NAME = 'matou.sqlite'
const IDLE_SCENE_ID = 'scale-idle-scene'
const BACKGROUND_IDLE_SCENE_ID = 'scale-background-idle-scene'
const FIXED_TIME = 1_700_000_100_000

export interface RecoveryScaleDatabaseCounts {
  sessions: number
  recoverySessions: number
  recoveryWorkspaces: number
  recoveryTasks: number
  recoveryScenes: number
  workspaces: number
  tasks: number
  scenes: number
}

export async function seedRuntimeRecoveryScale(dataDirectory: string): Promise<void> {
  await seedScaleDatabase(dataDirectory, {
    siblingSessions: 20,
    workspaceCount: 3,
    tasksPerWorkspace: 5,
    sessionsPerTask: 8
  })
  const database = RuntimeDatabase.open(join(dataDirectory, DATABASE_NAME))
  try {
    database.exec('BEGIN IMMEDIATE;')
    try {
      database.run(
        `INSERT INTO scenes (
           id, task_id, name, mode, root_node_id, created_at, updated_at,
           archived_at, title_pinned, sort_key, layout_revision
         ) VALUES (?, 'scale-task', 'Scale Idle Sessions', 'card', NULL, ?, ?, NULL, 0, '000002', 1)`,
        IDLE_SCENE_ID, FIXED_TIME, FIXED_TIME
      )
      database.run(
        `INSERT INTO scenes (
           id, task_id, name, mode, root_node_id, created_at, updated_at,
           archived_at, title_pinned, sort_key, layout_revision
         ) VALUES (?, ?, 'Scale Background Idle Sessions', 'card', NULL, ?, ?, NULL, 0, '000002', 1)`,
        BACKGROUND_IDLE_SCENE_ID, RECOVERY_SCALE_BACKGROUND_TASK_ID, FIXED_TIME + 1, FIXED_TIME + 1
      )
      for (const sessionId of sessionIds('scale-sibling', 20).slice(16)) {
        database.run(
          'UPDATE session_canvas_memberships SET scene_id = ?, updated_at = ? WHERE session_id = ?',
          IDLE_SCENE_ID, FIXED_TIME, sessionId
        )
        database.run(
          'UPDATE session_mounts SET scene_id = ? WHERE session_id = ?',
          IDLE_SCENE_ID, sessionId
        )
      }
      for (const sessionId of sessionIds('scale-catalog-00002-00001', 8).slice(4)) {
        database.run(
          'UPDATE session_canvas_memberships SET scene_id = ?, updated_at = ? WHERE session_id = ?',
          BACKGROUND_IDLE_SCENE_ID, FIXED_TIME, sessionId
        )
        database.run(
          'UPDATE session_mounts SET scene_id = ? WHERE session_id = ?',
          BACKGROUND_IDLE_SCENE_ID, sessionId
        )
      }
      const placeholders = RECOVERY_SCALE_SESSION_IDS.map(() => '?').join(', ')
      database.run(
        `UPDATE sessions
         SET work_status = 'interrupted', last_activity_at = ?, updated_at = ?
         WHERE id IN (${placeholders})`,
        FIXED_TIME, FIXED_TIME, ...RECOVERY_SCALE_SESSION_IDS
      )
      database.exec('COMMIT;')
    } catch (error) {
      database.exec('ROLLBACK;')
      throw error
    }
  } finally {
    database.close()
  }
}

export function historyMarker(sessionId: string): string {
  const index = RECOVERY_SCALE_SESSION_IDS.indexOf(sessionId)
  if (index < 0) throw new Error(`Unknown recovery scale Session: ${sessionId}`)
  return `MATOU_HISTORY_${String(index + 1).padStart(2, '0')}`
}

export async function readRecoveryScaleCounts(
  dataDirectory: string
): Promise<RecoveryScaleDatabaseCounts> {
  const database = RuntimeDatabase.openReadOnly(join(dataDirectory, DATABASE_NAME))
  try {
    const count = (sql: string): number => Number(database.get<{ count: number | bigint }>(sql)?.count ?? 0)
    return {
      sessions: count('SELECT COUNT(*) AS count FROM sessions WHERE archived_at IS NULL'),
      recoverySessions: count(
        "SELECT COUNT(*) AS count FROM sessions WHERE archived_at IS NULL AND work_status = 'interrupted'"
      ),
      recoveryWorkspaces: count(
        `SELECT COUNT(DISTINCT tasks.workspace_id) AS count
           FROM sessions JOIN tasks ON tasks.id = sessions.task_id
          WHERE sessions.archived_at IS NULL AND sessions.work_status = 'interrupted'`
      ),
      recoveryTasks: count(
        "SELECT COUNT(DISTINCT task_id) AS count FROM sessions WHERE archived_at IS NULL AND work_status = 'interrupted'"
      ),
      recoveryScenes: count(
        `SELECT COUNT(DISTINCT session_mounts.scene_id) AS count
           FROM sessions JOIN session_mounts ON session_mounts.session_id = sessions.id
          WHERE sessions.archived_at IS NULL AND sessions.work_status = 'interrupted'`
      ),
      workspaces: count('SELECT COUNT(*) AS count FROM workspaces WHERE archived_at IS NULL'),
      tasks: count('SELECT COUNT(*) AS count FROM tasks WHERE archived_at IS NULL'),
      scenes: count('SELECT COUNT(*) AS count FROM scenes WHERE archived_at IS NULL')
    }
  } finally {
    database.close()
  }
}

function sessionIds(prefix: string, count: number): string[] {
  return Array.from({ length: count }, (_, index) =>
    `${prefix}-${String(index + 1).padStart(5, '0')}`)
}
