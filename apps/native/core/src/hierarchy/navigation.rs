use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use super::{
    ExecutionContext, HierarchyResult, Mount, Scene, Session, Task, WindowNavigation, Workspace,
};

pub(super) fn register_window(connection: &Connection, window_id: &str, now: i64) -> Result<()> {
    connection.execute(
        "INSERT INTO app_windows (id, kind, state, created_at, updated_at)
         VALUES (?, 'main', 'visible', ?, ?)
         ON CONFLICT(id) DO UPDATE SET state = 'visible', updated_at = excluded.updated_at",
        params![window_id, now, now],
    )?;
    connection.execute(
        "INSERT INTO window_navigation (window_id, active_workspace_id, updated_at)
         VALUES (?, NULL, ?)
         ON CONFLICT(window_id) DO NOTHING",
        params![window_id, now],
    )?;
    Ok(())
}

pub(super) fn activate_workspace(
    connection: &Connection,
    window_id: &str,
    workspace_id: &str,
    now: i64,
) -> Result<()> {
    connection.execute(
        "INSERT INTO window_navigation (window_id, active_workspace_id, updated_at)
         VALUES (?, ?, ?)
         ON CONFLICT(window_id) DO UPDATE SET
           active_workspace_id = excluded.active_workspace_id,
           updated_at = excluded.updated_at",
        params![window_id, workspace_id, now],
    )?;

    let task_id: Option<String> = connection
        .query_row(
            "SELECT tasks.id FROM tasks
         LEFT JOIN window_workspace_focus focus
           ON focus.window_id = ? AND focus.workspace_id = tasks.workspace_id
         WHERE tasks.workspace_id = ? AND tasks.archived_at IS NULL
         ORDER BY (tasks.id = focus.active_task_id) DESC, tasks.sort_key, tasks.created_at
         LIMIT 1",
            [window_id, workspace_id],
            |row| row.get(0),
        )
        .optional()?;
    connection.execute(
        "INSERT INTO window_workspace_focus (window_id, workspace_id, active_task_id, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(window_id, workspace_id) DO UPDATE SET
           active_task_id = excluded.active_task_id,
           updated_at = excluded.updated_at",
        params![window_id, workspace_id, task_id, now],
    )?;
    let Some(task_id) = task_id else {
        return Ok(());
    };

    let scene_id: Option<String> = connection
        .query_row(
            "SELECT scenes.id FROM scenes
         LEFT JOIN window_task_focus focus
           ON focus.window_id = ? AND focus.task_id = scenes.task_id
         WHERE scenes.task_id = ? AND scenes.archived_at IS NULL
         ORDER BY (scenes.id = focus.active_scene_id) DESC, scenes.sort_key, scenes.created_at
         LIMIT 1",
            [window_id, &task_id],
            |row| row.get(0),
        )
        .optional()?;
    connection.execute(
        "INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(window_id, task_id) DO UPDATE SET
           active_scene_id = excluded.active_scene_id,
           updated_at = excluded.updated_at",
        params![window_id, task_id, scene_id, now],
    )?;
    let Some(scene_id) = scene_id else {
        return Ok(());
    };

    let session_id: Option<String> = connection
        .query_row(
            "SELECT sessions.id FROM session_mounts
         JOIN sessions ON sessions.id = session_mounts.session_id
         LEFT JOIN window_scene_focus focus
           ON focus.window_id = ? AND focus.scene_id = session_mounts.scene_id
         WHERE session_mounts.scene_id = ? AND sessions.archived_at IS NULL
         ORDER BY (sessions.id = focus.active_session_id) DESC, session_mounts.created_at
         LIMIT 1",
            [window_id, &scene_id],
            |row| row.get(0),
        )
        .optional()?;
    connection.execute(
        "INSERT INTO window_scene_focus (window_id, scene_id, active_session_id, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(window_id, scene_id) DO UPDATE SET
           active_session_id = excluded.active_session_id,
           updated_at = excluded.updated_at",
        params![window_id, scene_id, session_id, now],
    )?;
    Ok(())
}

fn read_navigation(connection: &Connection, window_id: &str) -> Result<WindowNavigation> {
    let active_workspace_id = connection
        .query_row(
            "SELECT active_workspace_id FROM window_navigation WHERE window_id = ?",
            [window_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    Ok(WindowNavigation {
        window_id: window_id.to_owned(),
        active_workspace_id,
        task_by_workspace: read_focus(
            connection,
            window_id,
            "SELECT workspace_id, active_task_id FROM window_workspace_focus
             WHERE window_id = ? AND active_task_id IS NOT NULL",
        )?,
        scene_by_task: read_focus(
            connection,
            window_id,
            "SELECT task_id, active_scene_id FROM window_task_focus
             WHERE window_id = ? AND active_scene_id IS NOT NULL",
        )?,
        session_by_scene: read_focus(
            connection,
            window_id,
            "SELECT scene_id, active_session_id FROM window_scene_focus
             WHERE window_id = ? AND active_session_id IS NOT NULL",
        )?,
    })
}

fn read_focus(
    connection: &Connection,
    window_id: &str,
    sql: &str,
) -> Result<BTreeMap<String, String>> {
    let mut statement = connection.prepare(sql)?;
    Ok(statement
        .query_map([window_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Read the window's persisted navigation and the entities along its active path.
pub fn read_hierarchy(connection: &Connection, window_id: &str) -> Result<HierarchyResult> {
    let navigation = read_navigation(connection, window_id)?;
    let workspace = match &navigation.active_workspace_id {
        Some(workspace_id) => connection
            .query_row(
                "SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL",
                [workspace_id],
                Workspace::from_row,
            )
            .optional()?,
        None => None,
    };
    let active_task_id = workspace
        .as_ref()
        .and_then(|workspace| navigation.task_by_workspace.get(&workspace.id));
    let task = match active_task_id {
        Some(task_id) => connection
            .query_row(
                "SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL",
                [task_id],
                Task::from_row,
            )
            .optional()?,
        None => None,
    };
    let execution_context = match &task {
        Some(task) => connection
            .query_row(
                "SELECT id, workspace_id, cwd, created_at, archived_at FROM execution_contexts
             WHERE id = ? AND kind = 'plain-directory'",
                [&task.execution_context_id],
                ExecutionContext::plain_directory_from_row,
            )
            .optional()?,
        None => None,
    };
    let active_scene_id = task
        .as_ref()
        .and_then(|task| navigation.scene_by_task.get(&task.id));
    let scene = match active_scene_id {
        Some(scene_id) => connection
            .query_row(
                "SELECT * FROM scenes WHERE id = ? AND archived_at IS NULL",
                [scene_id],
                Scene::from_row,
            )
            .optional()?,
        None => None,
    };
    let active_session_id = scene
        .as_ref()
        .and_then(|scene| navigation.session_by_scene.get(&scene.id));
    let session = match active_session_id {
        Some(session_id) => connection
            .query_row(
                "SELECT * FROM sessions WHERE id = ? AND archived_at IS NULL",
                [session_id],
                Session::from_row,
            )
            .optional()?,
        None => None,
    };
    let mount = match (&scene, &session) {
        (Some(scene), Some(session)) => connection.query_row(
            "SELECT * FROM session_mounts WHERE scene_id = ? AND session_id = ? ORDER BY created_at LIMIT 1",
            params![scene.id, session.id],
            Mount::from_row,
        ).optional()?,
        _ => None,
    };
    Ok(HierarchyResult {
        workspace,
        execution_context,
        task,
        scene,
        session,
        mount,
        navigation,
    })
}
