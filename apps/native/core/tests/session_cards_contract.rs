use anyhow::Result;
use matou_native_core::{
    hierarchy::{
        BootstrapWindowInput, HierarchyIds, bootstrap_window,
        canvas::{
            CanvasIds, CanvasMessages, CreateCanvasInput, CreateShellSiblingInput, create_canvas,
            create_shell_sibling,
        },
        cards::{
            CloseSessionCardInput, RemoveScope, RemoveSessionBranchInput, close_card,
            close_shortcut_is_last, remove_branch,
        },
        focus_session,
        tasks::{CreateTaskInput, create_task},
    },
    sessions::{StartRunInput, finish_run, get_run, get_session, start_run},
    storage::{CommandMetadata, configure_connection, migrate},
};
use rusqlite::{Connection, params, types::Value as SqlValue};
use serde_json::{Value, json};

const WINDOW: &str = "main";
const SCENE: &str = "canvas";
const ROOT: &str = "root";
const AT: i64 = 100;
const MESSAGES: CanvasMessages<'_> = CanvasMessages {
    new_canvas: "新画布",
    duplicate_scene_name: "同名页签",
    workspace_path_invalid: "工作区不可用",
    detached_session_must_return: "独立窗口中的会话需要先回到原会话列表",
};

struct Fixture {
    db: Connection,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let mut db = Connection::open(directory.path().join("matou.sqlite"))?;
        configure_connection(&db)?;
        migrate(&mut db, 1)?;
        bootstrap_window(
            &mut db,
            &command("bootstrap"),
            &BootstrapWindowInput {
                window_id: WINDOW.into(),
                default_root_directory: directory.path().display().to_string(),
                default_name: "Workspace".into(),
                now: 2,
            },
            &ids("workspace", "task", SCENE, ROOT),
            "默认",
            2,
        )?;
        Ok(Self {
            db,
            _directory: directory,
        })
    }

    fn sibling(&mut self, id: &str, parent: Option<&str>, now: i64) -> Result<()> {
        create_shell_sibling(
            &mut self.db,
            &command(id),
            &CreateShellSiblingInput {
                window_id: WINDOW.into(),
                scene_id: SCENE.into(),
                source_session_id: parent.unwrap_or(ROOT).into(),
                parent_session_id: parent.map(str::to_owned),
                execution_context_id: None,
                title: None,
                navigation: None,
                now,
            },
            &canvas_ids(SCENE, id),
            &MESSAGES,
            now,
        )?;
        Ok(())
    }

    fn extra_canvas(&mut self) -> Result<()> {
        create_canvas(
            &mut self.db,
            &command("other-canvas"),
            &CreateCanvasInput {
                window_id: WINDOW.into(),
                task_id: "task".into(),
                title: None,
                navigation: None,
                now: 20,
            },
            &canvas_ids("other-canvas", "other-session"),
            &MESSAGES,
            20,
        )?;
        Ok(())
    }

    fn focus(&mut self, id: &str) -> Result<()> {
        focus_session(&mut self.db, WINDOW, SCENE, id, 50, "wrong canvas")?;
        Ok(())
    }

    fn remove(&mut self, request: &str, id: &str, scope: RemoveScope) -> Result<Value> {
        remove_branch(
            &mut self.db,
            &command(request),
            &RemoveSessionBranchInput {
                window_id: WINDOW.into(),
                scene_id: SCENE.into(),
                session_id: id.into(),
                scope,
                now: AT,
            },
            AT,
        )
    }

    fn close(
        &mut self,
        request: &str,
        id: &str,
        intent: Option<&str>,
        preserve: bool,
    ) -> Result<Value> {
        close_card(
            &mut self.db,
            &command(request),
            &close_input(id, intent, preserve),
            &ids(
                "workspace",
                "replacement-task",
                "replacement-scene",
                "replacement-session",
            ),
            "默认",
            None,
            AT,
        )
    }

    fn snapshot(&self) -> Result<Vec<Vec<Vec<SqlValue>>>> {
        [
            "app_windows",
            "scene_windows",
            "window_navigation",
            "window_workspace_focus",
            "window_task_focus",
            "window_scene_focus",
            "window_task_placements",
            "tasks",
            "workspaces",
            "scenes",
            "sessions",
            "session_runs",
            "session_mounts",
            "session_canvas_memberships",
            "session_relation_events",
            "session_relations_current",
            "domain_events",
            "command_deduplication",
        ]
        .iter()
        .map(|table| {
            let mut query = self
                .db
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
            let columns = query.column_count();
            Ok(query
                .query_map([], |row| {
                    (0..columns).map(|column| row.get(column)).collect()
                })?
                .collect::<rusqlite::Result<_>>()?)
        })
        .collect()
    }
}

#[test]
fn remove_leaf_preserves_history_and_focus_and_is_replayable() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.sibling("leaf", Some(ROOT), 3)?;
    fixture.focus(ROOT)?;
    let before = get_session(&fixture.db, "leaf")?.unwrap();
    let result = fixture.remove("remove-leaf", "leaf", RemoveScope::NodeOnly)?;
    assert_eq!(result["removedSessionIds"], json!(["leaf"]));
    assert_eq!(result["disposedSessionIds"], json!(["leaf"]));
    assert_eq!(result["graph"]["focusedSessionId"], ROOT);
    assert_eq!(result["graph"]["edges"], json!([]));
    let after = get_session(&fixture.db, "leaf")?.unwrap();
    assert_eq!(after.status, "archived");
    assert_eq!(after.archived_at, Some(AT));
    assert_eq!(after.version, before.version + 1);
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_mounts WHERE session_id = 'leaf'"
        )?,
        0
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_canvas_memberships WHERE session_id = 'leaf'"
        )?,
        0
    );
    let snapshot = fixture.snapshot()?;
    assert_eq!(
        fixture.remove("remove-leaf", "leaf", RemoveScope::NodeOnly)?,
        result
    );
    assert_eq!(
        fixture
            .remove("remove-again", "leaf", RemoveScope::NodeOnly)
            .unwrap_err()
            .to_string(),
        "Session does not exist in this Scene"
    );
    assert_eq!(fixture.snapshot()?, snapshot);
    Ok(())
}

#[test]
fn remove_middle_reconnects_direct_children_in_membership_order() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.sibling("middle", Some(ROOT), 3)?;
    fixture.sibling("z-first", Some("middle"), 4)?;
    fixture.sibling("a-second", Some("middle"), 5)?;
    fixture.sibling("grandchild", Some("z-first"), 6)?;
    fixture.db.execute(
        "UPDATE session_relations_current SET relation_kind = 'forked-from', metadata_json = '{\"source\":\"keep\"}'
         WHERE from_session_id = 'z-first'", [],
    )?;
    fixture.focus("middle")?;
    let result = fixture.remove("remove-middle", "middle", RemoveScope::NodeOnly)?;
    assert_eq!(result["removedSessionIds"], json!(["middle"]));
    assert_eq!(result["graph"]["focusedSessionId"], ROOT);
    let mut query = fixture.db.prepare(
        "SELECT from_session_id, to_session_id, relation_kind, metadata_json
         FROM session_relation_events WHERE command_id = 'remove-middle' AND operation = 'created'
         ORDER BY sequence",
    )?;
    let reconnected = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert_eq!(
        reconnected,
        [
            (
                "z-first".into(),
                ROOT.into(),
                "forked-from".into(),
                "{\"source\":\"keep\"}".into()
            ),
            (
                "a-second".into(),
                ROOT.into(),
                "derived-from".into(),
                "{}".into()
            ),
        ]
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_relations_current
        WHERE to_session_id = 'z-first' AND from_session_id = 'grandchild'"
        )?,
        1
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_relation_events
        WHERE command_id = 'remove-middle' AND operation = 'revoked'"
        )?,
        3
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_relations_current AS current
        JOIN session_relation_events AS event ON event.sequence = current.source_event_sequence
        WHERE event.command_id = 'remove-middle' AND current.to_session_id = 'root'"
        )?,
        2
    );
    Ok(())
}

#[test]
fn remove_root_promotes_children_and_focuses_first_live_child() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.sibling("old-child", Some(ROOT), 3)?;
    fixture.sibling("live-child", Some(ROOT), 4)?;
    fixture.sibling("peer", None, 5)?;
    fixture.db.execute(
        "UPDATE sessions SET status = 'archived', archived_at = 7 WHERE id = 'old-child'",
        [],
    )?;
    fixture.focus(ROOT)?;
    let result = fixture.remove("remove-root", ROOT, RemoveScope::NodeOnly)?;
    assert_eq!(result["graph"]["focusedSessionId"], "live-child");
    assert_eq!(result["graph"]["edges"], json!([]));
    assert_eq!(result["graph"]["nodes"].as_array().unwrap().len(), 3);
    Ok(())
}

#[test]
fn remove_focused_root_clears_focus_if_only_stopped_members_remain() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.sibling("stopped", None, 3)?;
    fixture.db.execute(
        "UPDATE sessions SET status = 'archived', archived_at = 7 WHERE id = 'stopped'",
        [],
    )?;
    fixture.focus(ROOT)?;
    let result = fixture.remove("remove-live", ROOT, RemoveScope::NodeAndDescendants)?;
    assert!(result["graph"]["focusedSessionId"].is_null());
    assert_eq!(result["graph"]["nodes"][0]["sessionId"], "stopped");
    assert_eq!(result["removedSessionIds"], json!([ROOT]));
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM window_scene_focus WHERE active_session_id IS NULL"
        )?,
        1,
    );
    Ok(())
}

#[test]
fn remove_branch_stays_in_scene_and_only_disposes_live_members() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.sibling("branch", Some(ROOT), 3)?;
    fixture.sibling("child", Some("branch"), 4)?;
    fixture.sibling("stopped", Some("child"), 5)?;
    fixture.extra_canvas()?;
    fixture.db.execute_batch(
        "UPDATE sessions SET status = 'archived', archived_at = 7 WHERE id = 'stopped';
         INSERT INTO session_relation_events (
           event_id, relation_id, operation, task_id, from_session_id, to_session_id,
           relation_kind, command_id, occurred_at
         ) VALUES ('cross-event', 'cross', 'created', 'task', 'other-session', 'child', 'forked-from', 'seed', 8);
         INSERT INTO session_relations_current (
           relation_id, task_id, from_session_id, to_session_id, relation_kind, created_at, updated_at, source_event_sequence
         ) VALUES ('cross', 'task', 'other-session', 'child', 'forked-from', 8, 8, last_insert_rowid());",
    )?;
    fixture.focus("child")?;
    let result = fixture.remove("whole", "branch", RemoveScope::NodeAndDescendants)?;
    assert_eq!(
        result["removedSessionIds"],
        json!(["branch", "child", "stopped"])
    );
    assert_eq!(result["disposedSessionIds"], json!(["branch", "child"]));
    assert_eq!(result["graph"]["focusedSessionId"], ROOT);
    assert_eq!(result["graph"]["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(
        get_session(&fixture.db, "stopped")?.unwrap().archived_at,
        Some(7)
    );
    assert!(
        get_session(&fixture.db, "other-session")?
            .unwrap()
            .archived_at
            .is_none()
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_mounts WHERE session_id = 'other-session'"
        )?,
        1
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_relation_events
        WHERE relation_id = 'cross' AND operation = 'revoked'"
        )?,
        1
    );
    let payload: String = fixture.db.query_row(
        "SELECT payload_json FROM domain_events WHERE
        command_id = 'whole' AND event_type = 'session.graph-summary-changed'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(serde_json::from_str::<Value>(&payload)?, result);
    Ok(())
}

#[test]
fn removal_validation_and_late_sql_failure_do_not_write() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let before = fixture.snapshot()?;
    assert_eq!(
        fixture
            .remove("last", ROOT, RemoveScope::NodeOnly)
            .unwrap_err()
            .to_string(),
        "Scene must keep one Session"
    );
    assert_eq!(fixture.snapshot()?, before);
    fixture.sibling("middle", Some(ROOT), 3)?;
    fixture.sibling("child", Some("middle"), 4)?;
    fixture.focus("middle")?;
    let before = fixture.snapshot()?;
    assert_eq!(
        fixture
            .remove("whole-root", ROOT, RemoveScope::NodeAndDescendants)
            .unwrap_err()
            .to_string(),
        "Scene must keep one Session"
    );
    assert!(
        remove_branch(
            &mut fixture.db,
            &command("wrong-scene"),
            &RemoveSessionBranchInput {
                window_id: WINDOW.into(),
                scene_id: "wrong".into(),
                session_id: ROOT.into(),
                scope: RemoveScope::NodeOnly,
                now: AT,
            },
            AT
        )
        .is_err()
    );
    fixture.db.execute_batch(
        "CREATE TEMP TRIGGER reject_reconnect BEFORE INSERT ON session_relations_current
         BEGIN SELECT RAISE(ABORT, 'reconnect fault'); END;",
    )?;
    let error = fixture
        .remove("failed-reconnect", "middle", RemoveScope::NodeOnly)
        .unwrap_err();
    assert!(format!("{error:#}").contains("reconnect fault"));
    assert!(error.downcast_ref::<rusqlite::Error>().is_some());
    assert_eq!(fixture.snapshot()?, before);
    Ok(())
}

#[test]
fn close_card_requires_intent_and_preserves_membership_and_relations() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.sibling("focused", Some(ROOT), 3)?;
    fixture.sibling("other", None, 4)?;
    fixture.focus("focused")?;
    let before = fixture.snapshot()?;
    for intent in [None, Some("delete-session:other")] {
        assert_eq!(
            fixture
                .close("stale", ROOT, intent, false)
                .unwrap_err()
                .to_string(),
            "Session deletion intent is stale"
        );
    }
    assert_eq!(fixture.snapshot()?, before);
    let result = fixture.close("close-parent", ROOT, Some("delete-session:root"), false)?;
    assert_eq!(result["outcome"], "scene-remains");
    assert_eq!(result["session"]["id"], "focused");
    assert_eq!(result["disposedSessionIds"], json!([ROOT]));
    assert_eq!(result["rebuildCanvas"], false);
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_mounts WHERE session_id = 'root'"
        )?,
        0
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_canvas_memberships WHERE session_id = 'root'"
        )?,
        1
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_relations_current WHERE to_session_id = 'root'"
        )?,
        1
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM domain_events
        WHERE command_id = 'close-parent' AND event_type = 'session.archived'"
        )?,
        1
    );
    assert_eq!(
        fixture
            .close("re-close", ROOT, Some("delete-session:root"), false)
            .unwrap_err()
            .to_string(),
        "Session does not exist"
    );
    fixture.sibling("late", Some("other"), 8)?;
    assert_eq!(
        fixture.close("close-focused", "late", None, false)?["session"]["id"],
        "focused"
    );
    Ok(())
}

#[test]
fn close_rejects_unowned_active_run_and_rolls_back_owned_stop_on_error() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.sibling("shell", None, 3)?;
    start_run(
        &mut fixture.db,
        &command("start-run"),
        &StartRunInput {
            id: "run".into(),
            session_id: "shell".into(),
            runtime_generation: "generation".into(),
            pid: None,
            cols: 80,
            rows: 24,
            now: 4,
        },
        4,
    )?;
    fixture.db.execute(
        "UPDATE sessions SET work_status = 'needs-input' WHERE id = 'shell'",
        [],
    )?;
    let before = fixture.snapshot()?;
    assert_eq!(
        fixture
            .close("busy", "shell", None, false)
            .unwrap_err()
            .to_string(),
        "Session deletion intent is stale"
    );
    assert_eq!(
        fixture
            .close("running", "shell", Some("delete-session:shell"), false)
            .unwrap_err()
            .to_string(),
        "active SessionRuns must be stopped before archiving a Session"
    );
    let input = close_input("shell", Some("delete-session:shell"), false);
    let replacement = ids(
        "workspace",
        "replacement-task",
        "replacement-scene",
        "replacement-session",
    );
    fixture.db.execute_batch(
        "CREATE TEMP TRIGGER reject_mount_delete BEFORE DELETE ON session_mounts
        BEGIN SELECT RAISE(ABORT, 'mount fault'); END;",
    )?;
    let error = close_card(
        &mut fixture.db,
        &command("owned-fail"),
        &input,
        &replacement,
        "默认",
        Some("run"),
        AT,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("mount fault"));
    assert_eq!(fixture.snapshot()?, before);
    fixture
        .db
        .execute_batch("DROP TRIGGER reject_mount_delete;")?;
    close_card(
        &mut fixture.db,
        &command("owned"),
        &input,
        &replacement,
        "默认",
        Some("run"),
        AT,
    )?;
    assert_eq!(get_run(&fixture.db, "run")?.unwrap().status, "interrupted");
    let mut query = fixture
        .db
        .prepare("SELECT event_type FROM domain_events WHERE command_id = 'owned' ORDER BY seq")?;
    let events = query
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert_eq!(events, ["session.run-interrupted", "session.archived"]);
    drop(query);
    finish_run(
        &mut fixture.db,
        &command("late-exit"),
        "run",
        0,
        None,
        AT + 1,
        AT + 1,
    )?;
    assert_eq!(
        get_session(&fixture.db, "shell")?.unwrap().status,
        "archived"
    );
    Ok(())
}

#[test]
fn close_last_card_can_retain_canvas_and_clear_detached_window() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.db.execute_batch(
        "INSERT INTO scene_windows (id, scene_id, native_window_key, state, created_at, updated_at)
         VALUES ('detached', 'canvas', 'detached', 'detached', 3, 3);
         INSERT INTO app_windows (id, kind, state, created_at, updated_at)
         VALUES ('detached', 'detached-terminal', 'visible', 3, 3);
         UPDATE session_mounts SET scene_window_id = 'detached' WHERE session_id = 'root';",
    )?;
    let before = fixture.snapshot()?;
    assert!(close_shortcut_is_last(&fixture.db, SCENE, ROOT)?);
    assert_eq!(fixture.snapshot()?, before);
    let result = fixture.close("preserve", ROOT, Some("delete-session:root"), true)?;
    assert_eq!(result["outcome"], "session-stopped-remains");
    assert_eq!(result["rebuildCanvas"], false);
    assert_eq!(result["scene"]["id"], SCENE);
    assert!(result["session"].is_null());
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_mounts
        WHERE session_id = 'root' AND scene_window_id IS NULL"
        )?,
        1
    );
    for table in ["scene_windows", "app_windows"] {
        assert_eq!(
            count(
                &fixture.db,
                &format!("SELECT COUNT(*) FROM {table} WHERE id = 'detached' AND state = 'closed'")
            )?,
            1
        );
    }
    Ok(())
}

#[test]
fn close_last_mount_selects_scene_or_task_then_creates_default_only_for_available_path()
-> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.extra_canvas()?;
    fixture.focus(ROOT)?;
    let result = fixture.close("next-scene", ROOT, None, false)?;
    assert_eq!(result["outcome"], "scene-archives");
    assert_eq!(result["rebuildCanvas"], true);
    assert_eq!(result["scene"]["id"], "other-canvas");
    assert_eq!(result["session"]["id"], "other-session");
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM window_scene_focus WHERE scene_id = 'canvas'"
        )?,
        0
    );

    let mut fixture = Fixture::new()?;
    create_task(
        &mut fixture.db,
        &command("another-task"),
        &CreateTaskInput {
            window_id: WINDOW.into(),
            workspace_id: "workspace".into(),
            title: Some("Saved".into()),
            navigation: None,
            now: 5,
        },
        &ids("workspace", "other-task", "task-canvas", "task-shell"),
        "新事项",
        "不可用",
        5,
    )?;
    fixture.focus(ROOT)?;
    let result = fixture.close("next-task", ROOT, None, false)?;
    assert_eq!(result["outcome"], "scene-archives");
    assert_eq!(result["task"]["id"], "other-task");
    assert_eq!(result["workspace"]["taskOrder"], json!(["other-task"]));
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM window_task_focus WHERE task_id = 'task'"
        )?,
        0
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM window_task_placements WHERE task_id = 'task'"
        )?,
        0
    );

    let mut fixture = Fixture::new()?;
    let result = fixture.close("default", ROOT, Some("delete-session:root"), false)?;
    assert_eq!(result["outcome"], "default-task-created");
    assert_eq!(result["task"]["title"], "默认");
    assert_eq!(
        result["workspace"]["taskOrder"],
        json!(["replacement-task"])
    );
    assert_eq!(result["disposedSessionIds"], json!([ROOT]));
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM tasks WHERE id = 'task' AND archived_at IS NOT NULL"
        )?,
        1
    );
    assert_eq!(
        count(
            &fixture.db,
            "SELECT COUNT(*) FROM session_canvas_memberships WHERE session_id = 'replacement-session'"
        )?,
        1
    );

    let mut fixture = Fixture::new()?;
    fixture.db.execute(
        "INSERT INTO workspace_path_state (workspace_id, status, reason, checked_at, validation_generation)
         VALUES (?1, 'invalid', 'missing', ?2, 1)",
        params!["workspace", 8],
    )?;
    let result = fixture.close("invalid-path", ROOT, Some("delete-session:root"), false)?;
    assert_eq!(result["outcome"], "scene-archives");
    assert!(result["task"].is_null());
    assert_eq!(result["workspace"]["taskOrder"], json!([]));
    Ok(())
}

fn close_input(id: &str, intent: Option<&str>, preserve: bool) -> CloseSessionCardInput {
    CloseSessionCardInput {
        window_id: WINDOW.into(),
        scene_id: SCENE.into(),
        session_id: id.into(),
        confirmed_intent: intent.map(str::to_owned),
        preserve_scene_on_last_session: preserve,
        now: AT,
    }
}

fn count(db: &Connection, sql: &str) -> Result<i64> {
    Ok(db.query_row(sql, [], |row| row.get(0))?)
}

fn command(id: &str) -> CommandMetadata {
    CommandMetadata {
        command_id: id.into(),
        command_type: "test".into(),
        request_hash: format!("hash:{id}"),
        causation_id: None,
        correlation_id: None,
    }
}

fn ids(workspace: &str, task: &str, scene: &str, session: &str) -> HierarchyIds {
    HierarchyIds {
        workspace_id: workspace.into(),
        task_id: task.into(),
        scene_id: scene.into(),
        session_id: session.into(),
        execution_context_id: format!("context-{task}"),
        root_node_id: format!("node-{session}"),
        mount_id: format!("mount-{session}"),
    }
}

fn canvas_ids(scene: &str, session: &str) -> CanvasIds {
    CanvasIds {
        scene_id: scene.into(),
        session_id: session.into(),
        root_node_id: format!("node-{session}"),
        secondary_node_id: format!("split-{session}"),
        mount_id: format!("mount-{session}"),
        relation_id: format!("relation-{session}"),
    }
}
