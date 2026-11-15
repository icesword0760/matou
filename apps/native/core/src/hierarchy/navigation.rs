use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

use super::{
    ExecutionContext, HierarchyResult, Mount, Scene, Session, Task, WindowNavigation, Workspace,
};
use crate::session_graph::project_scene_graph;

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
    set_active_workspace(connection, window_id, workspace_id, now)?;
    let task_id = preferred_task(connection, window_id, workspace_id)?;
    focus_task(connection, window_id, workspace_id, task_id.as_deref(), now)
}

fn set_active_workspace(
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
    Ok(())
}

pub(super) fn preferred_task(
    connection: &Connection,
    window_id: &str,
    workspace_id: &str,
) -> Result<Option<String>> {
    Ok(connection
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
        .optional()?)
}

pub(super) fn activate_task_in_transaction(
    connection: &Connection,
    window_id: &str,
    task_id: &str,
    now: i64,
) -> Result<()> {
    let workspace_id: String = connection
        .query_row(
            "SELECT workspace_id FROM tasks WHERE id = ? AND archived_at IS NULL",
            [task_id],
            |row| row.get(0),
        )
        .optional()?
        .with_context(|| format!("Task {task_id} does not exist"))?;
    set_active_workspace(connection, window_id, &workspace_id, now)?;
    focus_task(connection, window_id, &workspace_id, Some(task_id), now)
}

fn focus_task(
    connection: &Connection,
    window_id: &str,
    workspace_id: &str,
    task_id: Option<&str>,
    now: i64,
) -> Result<()> {
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
    focus_preferred_session(connection, window_id, &scene_id, now)
}

fn focus_preferred_session(
    connection: &Connection,
    window_id: &str,
    scene_id: &str,
    now: i64,
) -> Result<()> {
    let session_id: Option<String> = connection
        .query_row(
            "SELECT sessions.id FROM session_mounts
         JOIN sessions ON sessions.id = session_mounts.session_id
         LEFT JOIN window_scene_focus focus
           ON focus.window_id = ? AND focus.scene_id = session_mounts.scene_id
         WHERE session_mounts.scene_id = ? AND sessions.archived_at IS NULL
         ORDER BY (sessions.id = focus.active_session_id) DESC, session_mounts.created_at
         LIMIT 1",
            [window_id, scene_id],
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

pub(super) fn activate_scene_in_transaction(
    connection: &Connection,
    window_id: &str,
    scene_id: &str,
    now: i64,
) -> Result<()> {
    let (task_id, workspace_id): (String, String) = connection
        .query_row(
            "SELECT scenes.task_id, tasks.workspace_id FROM scenes
             JOIN tasks ON tasks.id = scenes.task_id
             WHERE scenes.id = ? AND scenes.archived_at IS NULL AND tasks.archived_at IS NULL",
            [scene_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .with_context(|| format!("Scene {scene_id} does not exist"))?;
    connection.execute(
        "INSERT INTO window_navigation (window_id, active_workspace_id, updated_at)
         VALUES (?, ?, ?)
         ON CONFLICT(window_id) DO UPDATE SET
           active_workspace_id = excluded.active_workspace_id,
           updated_at = excluded.updated_at",
        params![window_id, workspace_id, now],
    )?;
    connection.execute(
        "INSERT INTO window_workspace_focus (window_id, workspace_id, active_task_id, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(window_id, workspace_id) DO UPDATE SET
           active_task_id = excluded.active_task_id,
           updated_at = excluded.updated_at",
        params![window_id, workspace_id, task_id, now],
    )?;
    connection.execute(
        "INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(window_id, task_id) DO UPDATE SET
           active_scene_id = excluded.active_scene_id,
           updated_at = excluded.updated_at",
        params![window_id, task_id, scene_id, now],
    )?;
    focus_preferred_session(connection, window_id, scene_id, now)
}

pub(super) fn activate_session_in_transaction(
    connection: &Connection,
    window_id: &str,
    session_id: &str,
    now: i64,
) -> Result<()> {
    let scene_id: String = connection
        .query_row(
            "SELECT session_mounts.scene_id FROM session_mounts
             JOIN sessions ON sessions.id = session_mounts.session_id
             JOIN scenes ON scenes.id = session_mounts.scene_id
             WHERE sessions.id = ? AND sessions.archived_at IS NULL AND scenes.archived_at IS NULL
             ORDER BY session_mounts.created_at LIMIT 1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?
        .with_context(|| format!("Session {session_id} does not exist in an active Scene"))?;
    let previous_session: Option<String> = connection
        .query_row(
            "SELECT active_session_id FROM window_scene_focus WHERE window_id = ? AND scene_id = ?",
            params![window_id, scene_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    activate_scene_in_transaction(connection, window_id, &scene_id, now)?;
    connection.execute(
        "UPDATE window_scene_focus SET active_session_id = ?, updated_at = ?
         WHERE window_id = ? AND scene_id = ?",
        params![session_id, now, window_id, scene_id],
    )?;

    if let Some(previous_session) = previous_session
        && !previous_session.is_empty()
        && previous_session != session_id
    {
        let remains_active: bool = connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM window_scene_focus WHERE active_session_id = ?)",
            [&previous_session],
            |row| row.get(0),
        )?;
        if !remains_active {
            connection.execute(
                "UPDATE session_canvas_memberships
                 SET last_user_interaction_seq = pending_user_interaction_seq,
                     pending_user_interaction_seq = 0,
                     updated_at = ?
                 WHERE session_id = ? AND pending_user_interaction_seq > 0",
                params![now, previous_session],
            )?;
        }
    }
    Ok(())
}

pub(super) fn restore_navigation_in_transaction(
    connection: &Connection,
    navigation: &WindowNavigation,
    now: i64,
) -> Result<()> {
    let window_id = &navigation.window_id;
    connection.execute(
        "UPDATE window_navigation SET active_workspace_id = ?, updated_at = ? WHERE window_id = ?",
        params![navigation.active_workspace_id, now, window_id],
    )?;
    connection.execute(
        "DELETE FROM window_workspace_focus WHERE window_id = ?",
        [window_id],
    )?;
    connection.execute(
        "DELETE FROM window_task_focus WHERE window_id = ?",
        [window_id],
    )?;
    connection.execute(
        "DELETE FROM window_scene_focus WHERE window_id = ?",
        [window_id],
    )?;
    for (workspace_id, task_id) in &navigation.task_by_workspace {
        connection.execute(
            "INSERT INTO window_workspace_focus (window_id, workspace_id, active_task_id, updated_at)
             VALUES (?, ?, ?, ?)",
            params![window_id, workspace_id, task_id, now],
        )?;
    }
    for (task_id, scene_id) in &navigation.scene_by_task {
        connection.execute(
            "INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at)
             VALUES (?, ?, ?, ?)",
            params![window_id, task_id, scene_id, now],
        )?;
    }
    for (scene_id, session_id) in &navigation.session_by_scene {
        connection.execute(
            "INSERT INTO window_scene_focus (window_id, scene_id, active_session_id, updated_at)
             VALUES (?, ?, ?, ?)",
            params![window_id, scene_id, session_id, now],
        )?;
    }
    Ok(())
}

pub fn read_created_path(connection: &Connection, session_id: &str) -> Result<Value> {
    let session = connection
        .query_row(
            "SELECT * FROM sessions WHERE id = ? AND archived_at IS NULL",
            [session_id],
            Session::from_row,
        )
        .optional()?
        .context("Session does not exist")?;
    let mount = connection
        .query_row(
            "SELECT * FROM session_mounts WHERE session_id = ? ORDER BY created_at, id LIMIT 1",
            [session_id],
            Mount::from_row,
        )
        .optional()?
        .context("SessionMount does not exist")?;
    let scene = connection
        .query_row(
            "SELECT * FROM scenes WHERE id = ? AND archived_at IS NULL",
            [&mount.scene_id],
            Scene::from_row,
        )
        .optional()?
        .context("Scene does not exist")?;
    let task = connection
        .query_row(
            "SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL",
            [&session.task_id],
            Task::from_row,
        )
        .optional()?
        .context("Task does not exist")?;
    let workspace = connection
        .query_row(
            "SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL",
            [&task.workspace_id],
            Workspace::from_row,
        )
        .optional()?
        .context("Workspace does not exist")?;
    Ok(
        json!({ "workspace": workspace, "task": task, "scene": scene, "session": session, "mount": mount }),
    )
}

pub fn select_scene(
    connection: &mut Connection,
    window_id: &str,
    scene_id: &str,
    now: i64,
) -> Result<HierarchyResult> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    register_window(&transaction, window_id, now)?;
    activate_scene_in_transaction(&transaction, window_id, scene_id, now)?;
    let result = read_hierarchy(&transaction, window_id)?;
    transaction.commit()?;
    Ok(result)
}

pub fn focus_session(
    connection: &mut Connection,
    window_id: &str,
    scene_id: &str,
    session_id: &str,
    now: i64,
    not_in_canvas_message: &str,
) -> Result<Value> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    register_window(&transaction, window_id, now)?;
    let belongs_to_scene: bool = transaction.query_row(
        "SELECT EXISTS (SELECT 1 FROM session_canvas_memberships WHERE session_id = ? AND scene_id = ?)",
        params![session_id, scene_id],
        |row| row.get(0),
    )?;
    ensure!(belongs_to_scene, "{not_in_canvas_message}");
    activate_session_in_transaction(&transaction, window_id, session_id, now)?;
    let graph = project_scene_graph(&transaction, scene_id, Some(window_id))?;
    transaction.commit()?;
    Ok(graph)
}

pub fn list_scenes(connection: &Connection, task_id: &str) -> Result<Vec<Scene>> {
    let mut statement = connection.prepare(
        "SELECT * FROM scenes WHERE task_id = ? AND archived_at IS NULL
         ORDER BY sort_key, created_at, id",
    )?;
    Ok(statement
        .query_map([task_id], Scene::from_row)?
        .collect::<rusqlite::Result<_>>()?)
}

pub(super) fn read_navigation(
    connection: &Connection,
    window_id: &str,
) -> Result<WindowNavigation> {
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
