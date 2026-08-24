import type { TaskPlacement, WindowNavigation } from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'

export class NavigationRepository {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  get(windowId: string): WindowNavigation {
    const navigation = this.#database.get<{ active_workspace_id: string | null }>(
      'SELECT active_workspace_id FROM window_navigation WHERE window_id = ?',
      windowId
    )
    return {
      windowId,
      ...(navigation?.active_workspace_id == null
        ? {}
        : { activeWorkspaceId: navigation.active_workspace_id }),
      taskByWorkspace: Object.fromEntries(
        this.#database.all<{ workspace_id: string; active_task_id: string }>(
          `SELECT workspace_id, active_task_id FROM window_workspace_focus
           WHERE window_id = ? AND active_task_id IS NOT NULL`,
          windowId
        ).map((row) => [row.workspace_id, row.active_task_id])
      ),
      sceneByTask: Object.fromEntries(
        this.#database.all<{ task_id: string; active_scene_id: string }>(
          `SELECT task_id, active_scene_id FROM window_task_focus
           WHERE window_id = ? AND active_scene_id IS NOT NULL`,
          windowId
        ).map((row) => [row.task_id, row.active_scene_id])
      ),
      sessionByScene: Object.fromEntries(
        this.#database.all<{ scene_id: string; active_session_id: string }>(
          `SELECT scene_id, active_session_id FROM window_scene_focus
           WHERE window_id = ? AND active_session_id IS NOT NULL`,
          windowId
        ).map((row) => [row.scene_id, row.active_session_id])
      )
    }
  }

  listTaskPlacements(): TaskPlacement[] {
    return this.#database.all<{
      window_id: string
      task_id: string
      ordinal: number
      updated_at: number
    }>(
      'SELECT * FROM window_task_placements ORDER BY window_id, ordinal, task_id'
    ).map((row) => ({
      windowId: row.window_id,
      taskId: row.task_id,
      ordinal: row.ordinal,
      updatedAt: row.updated_at
    }))
  }
}
