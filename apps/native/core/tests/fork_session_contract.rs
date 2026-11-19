use anyhow::Result;
use matou_native_core::{
    hierarchy::{
        BootstrapWindowInput, HierarchyIds, bootstrap_window,
        canvas::{
            CanvasError, CanvasIds, CanvasMessages, ForkPlacement, ForkSessionInput,
            ForkWorktreeMode, create_fork_child,
        },
    },
    session_graph::project_scene_graph,
    sessions::{get_session, pending_claude_fork},
    storage::{CommandMetadata, configure_connection, migrate},
};
use rusqlite::Connection;
use serde_json::Value;

const MESSAGES: CanvasMessages<'static> = CanvasMessages {
    new_canvas: "新画布",
    duplicate_scene_name: "当前事项下已存在同名页签",
    workspace_path_invalid: "工作区路径不可用",
    detached_session_must_return: "请先把会话返回当前画布",
};

fn command(id: &str) -> CommandMetadata {
    CommandMetadata {
        command_id: id.into(),
        command_type: "test.fork-session".into(),
        request_hash: id.into(),
        causation_id: None,
        correlation_id: None,
    }
}

fn fixture(cwd: &str) -> Result<Connection> {
    let mut db = Connection::open_in_memory()?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    bootstrap_window(
        &mut db,
        &command("bootstrap"),
        &BootstrapWindowInput {
            window_id: "window".into(),
            default_root_directory: cwd.into(),
            default_name: "workspace".into(),
            now: 1,
        },
        &HierarchyIds {
            workspace_id: "workspace".into(),
            execution_context_id: "context".into(),
            task_id: "task".into(),
            scene_id: "scene".into(),
            root_node_id: "source-node".into(),
            session_id: "source".into(),
            mount_id: "source-mount".into(),
        },
        "任务",
        1,
    )?;
    db.execute_batch(
        r#"UPDATE sessions SET kind='claude-code', status='running', work_status='running' WHERE id='source';
         INSERT INTO provider_bindings (id, session_id, provider, provider_session_id, resume_state,
                                        metadata_json, created_at, updated_at, validated_at)
         VALUES ('source-binding', 'source', 'claude-code', 'source-provider-id', 'resumed',
                 '{"canFork":true,"providerConfigId":"source-config","model":"model","permissionMode":"plan"}',
                 2, 2, 2);"#,
    )?;
    Ok(db)
}

fn request() -> ForkSessionInput {
    ForkSessionInput {
        window_id: "window".into(),
        scene_id: "scene".into(),
        source_session_id: "source".into(),
        placement: ForkPlacement::Child,
        name: "  子分支  ".into(),
        worktree_mode: ForkWorktreeMode::Current,
        submission_key: "submission".into(),
        now: 3,
    }
}

fn create(db: &mut Connection, command_id: &str, input: &ForkSessionInput) -> Result<Value> {
    create_fork_child(
        db,
        &command(command_id),
        input,
        &CanvasIds {
            scene_id: "unused".into(),
            root_node_id: "child-node".into(),
            secondary_node_id: "split-node".into(),
            session_id: "child".into(),
            mount_id: "child-mount".into(),
            relation_id: "relation".into(),
        },
        &MESSAGES,
        input.now,
    )
}

#[test]
fn fork_validation_rejects_before_writing_any_card() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let cwd = directory.path().to_str().unwrap();
    let reject = |mut db: Connection, input: ForkSessionInput, code: &str, message: &str| {
        let error = create(&mut db, "rejected", &input).unwrap_err();
        assert_eq!(error.downcast_ref::<CanvasError>().unwrap().code, code);
        assert_eq!(error.to_string(), message);
        assert_eq!(
            db.query_row(
                "SELECT (SELECT COUNT(*) FROM sessions),
                    (SELECT COUNT(*) FROM session_fork_intents),
                    (SELECT COUNT(*) FROM session_mounts),
                    (SELECT COUNT(*) FROM session_relations_current)",
                [],
                |row| Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?
                )),
            )
            .unwrap(),
            (1, 0, 1, 0)
        );
    };
    for sql in [
        "DELETE FROM provider_bindings",
        "UPDATE provider_bindings SET resume_state='unknown'",
        "UPDATE provider_bindings SET validated_at=NULL",
        "UPDATE provider_bindings SET invalidated_at=3",
        "UPDATE provider_bindings SET restore_state='restoring'",
        r#"UPDATE provider_bindings SET metadata_json='{"canFork":false}'"#,
        r#"UPDATE provider_bindings SET metadata_json='{"observedUserPrompt":true}'"#,
        r#"UPDATE provider_bindings SET metadata_json='{"observedNormalStop":true}'"#,
        "UPDATE sessions SET kind='shell'",
        "INSERT INTO provider_bindings (id, session_id, provider, provider_session_id, resume_state,
                                        metadata_json, created_at, updated_at, restore_state)
         VALUES ('restore', 'source', 'claude-code', 'other', 'failed', '{}', 3, 3, 'failed')",
    ] {
        let db = fixture(cwd)?;
        db.execute_batch(sql)?;
        reject(db, request(), "FORK_SOURCE_NOT_READY", "完成首轮 AI 对话后可创建分支");
    }

    let db = fixture(cwd)?;
    db.execute_batch("UPDATE sessions SET archived_at=3, status='archived'")?;
    reject(db, request(), "SESSION_NOT_IN_CANVAS", "会话不在当前画布中");
    let db = fixture(cwd)?;
    db.execute_batch(
        "INSERT INTO scene_windows (id, scene_id, native_window_key, state, created_at, updated_at)
         VALUES ('detached', 'scene', 'detached', 'detached', 3, 3);
         UPDATE session_mounts SET scene_window_id='detached' WHERE session_id='source';",
    )?;
    reject(db, request(), "DETACHED_SOURCE", "请先把会话返回当前画布");

    for (input, code, message) in [
        (
            ForkSessionInput {
                scene_id: "other".into(),
                ..request()
            },
            "SESSION_NOT_IN_CANVAS",
            "会话不在当前画布中",
        ),
        (
            ForkSessionInput {
                name: "\u{feff} ".into(),
                ..request()
            },
            "EMPTY_NAME",
            "请输入分支名称",
        ),
        (
            ForkSessionInput {
                name: "🧪".repeat(65),
                ..request()
            },
            "TOO_LONG_NAME",
            "分支名称最多 64 个字符",
        ),
        (
            ForkSessionInput {
                submission_key: " \t".into(),
                ..request()
            },
            "EMPTY_SUBMISSION_KEY",
            "分叉提交标识不能为空",
        ),
        (
            ForkSessionInput {
                placement: ForkPlacement::Sibling,
                ..request()
            },
            "ROOT_HAS_NO_FORK_PARENT",
            "根层会话可创建子分支",
        ),
        (
            ForkSessionInput {
                worktree_mode: ForkWorktreeMode::New,
                ..request()
            },
            "FORK_WORKTREE_MODE_NOT_IMPLEMENTED",
            "当前仅支持在当前工作树创建分支",
        ),
    ] {
        reject(fixture(cwd)?, input, code, message);
    }
    Ok(())
}

fn add_parent(db: &Connection) -> Result<()> {
    db.execute_batch(
        r#"INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
           SELECT 'parent-context', workspace_id, kind, cwd || '/parent', created_at
           FROM execution_contexts WHERE id='context';
         INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, cwd,
                               work_status, created_at, updated_at, last_activity_at)
           SELECT 'parent', task_id, 'parent-context', kind, 'running', 'Parent', cwd || '/parent',
                  'running', created_at, updated_at, last_activity_at
           FROM sessions WHERE id='source';
         INSERT INTO provider_bindings (id, session_id, provider, provider_session_id, resume_state,
                                        metadata_json, created_at, updated_at, validated_at)
         VALUES ('parent-binding', 'parent', 'claude-code', 'parent-provider-id', 'available',
                 '{"canFork":true,"providerConfigId":"parent-config","model":"parent-model","permissionMode":"acceptEdits"}',
                 2, 2, 2);
         INSERT INTO session_relation_events (event_id, relation_id, operation, task_id,
                                              from_session_id, to_session_id, relation_kind, command_id, occurred_at)
         VALUES ('parent-event', 'parent-relation', 'created', 'task',
                 'source', 'parent', 'derived-from', 'fixture', 2);
         INSERT INTO session_relations_current (relation_id, task_id, from_session_id, to_session_id,
                                                relation_kind, created_at, updated_at, source_event_sequence)
         VALUES ('parent-relation', 'task', 'source', 'parent', 'derived-from', 2, 2, last_insert_rowid());"#,
    )?;
    Ok(())
}

#[test]
fn sibling_and_peer_use_their_own_fork_source_and_structural_parent() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let cwd = directory.path().to_str().unwrap();
    for (placement, parent_state, fork_source, expected_relation) in [
        (
            ForkPlacement::Sibling,
            "running",
            "parent",
            Some(("parent", "forked-from")),
        ),
        (
            ForkPlacement::Peer,
            "running",
            "source",
            Some(("parent", "derived-from")),
        ),
        (
            ForkPlacement::Peer,
            "archived",
            "source",
            Some(("parent", "derived-from")),
        ),
        (ForkPlacement::Peer, "removed", "source", None),
        (ForkPlacement::Peer, "absent", "source", None),
    ] {
        let mut db = fixture(cwd)?;
        if parent_state != "absent" {
            add_parent(&db)?;
        }
        match parent_state {
            "archived" => db.execute_batch(
                "UPDATE sessions SET archived_at=3, status='archived' WHERE id='parent'",
            )?,
            "removed" => remove_parent_with_stale_relation(&db)?,
            _ => {}
        }
        if placement == ForkPlacement::Sibling {
            // The selected card only provides the structural relation and mount.
            db.execute_batch("DELETE FROM provider_bindings WHERE session_id='source'")?;
        }
        let input = ForkSessionInput {
            placement,
            ..request()
        };
        let origin = get_session(&db, fork_source)?.unwrap();
        let accepted = create(&mut db, "first", &input)?;
        assert_eq!(accepted["sessionId"], "child");
        let child = get_session(&db, "child")?.unwrap();
        assert_eq!(child.cwd, origin.cwd);
        assert_eq!(child.execution_context_id, origin.execution_context_id);
        let graph = project_scene_graph(&db, "scene", Some("window"))?;
        let node = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["sessionId"] == "child")
            .unwrap();
        assert_eq!(
            node["parentSessionId"].as_str(),
            expected_relation.map(|(id, _)| id)
        );
        assert_eq!(
            node["relationKind"].as_str(),
            expected_relation.map(|(_, kind)| kind)
        );
        assert_eq!(
            db.query_row(
                "SELECT split.direction, source.ordinal, child.ordinal
                 FROM scene_nodes AS split
                 JOIN scene_nodes AS source ON source.parent_node_id=split.id AND source.id='source-node'
                 JOIN scene_nodes AS child ON child.parent_node_id=split.id AND child.id='child-node'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
            )?,
            ("horizontal".into(), 0, 1)
        );
        let target = pending_claude_fork(&db, "child")?.unwrap();
        assert_eq!(target.source_id, format!("{fork_source}-provider-id"));
        assert_eq!(
            target.metadata["providerConfigId"],
            format!("{fork_source}-config")
        );
        assert_eq!(
            target.metadata["permissionMode"],
            if fork_source == "parent" {
                "acceptEdits"
            } else {
                "plan"
            }
        );
        assert_eq!(get_session(&db, fork_source)?.unwrap(), origin);
        let replay = create(&mut db, "replay", &input)?;
        assert_eq!(replay["sessionId"], "child");
        assert_eq!(replay["replayed"], true);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM session_fork_intents", [], |row| row
                .get::<_, i64>(
                0
            ))?,
            1
        );
    }
    Ok(())
}

fn remove_parent_with_stale_relation(db: &Connection) -> Result<()> {
    db.execute_batch(
        "PRAGMA foreign_keys=OFF;
         DELETE FROM provider_bindings WHERE session_id='parent';
         DELETE FROM sessions WHERE id='parent';
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(())
}

#[test]
fn sibling_rejects_an_unusable_parent_without_creating_a_card() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let cwd = directory.path().to_str().unwrap();
    for sql in [
        None,
        Some("UPDATE sessions SET status='archived', archived_at=3 WHERE id='parent'"),
        Some("DELETE FROM provider_bindings WHERE session_id='parent'"),
        Some(
            r#"UPDATE provider_bindings SET metadata_json='{"canFork":false}' WHERE session_id='parent'"#,
        ),
    ] {
        let mut db = fixture(cwd)?;
        add_parent(&db)?;
        match sql {
            Some(sql) => db.execute_batch(sql)?,
            None => remove_parent_with_stale_relation(&db)?,
        }
        let input = ForkSessionInput {
            placement: ForkPlacement::Sibling,
            ..request()
        };
        let error = create(&mut db, "rejected", &input).unwrap_err();
        assert_eq!(
            error.downcast_ref::<CanvasError>().unwrap().code,
            "FORK_SOURCE_NOT_READY"
        );
        assert!(get_session(&db, "child")?.is_none());
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM session_fork_intents", [], |row| row
                .get::<_, i64>(
                0
            ))?,
            0
        );
    }
    Ok(())
}

#[test]
fn fork_is_atomic_and_reuses_the_submission_even_after_source_is_removed() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut db = fixture(directory.path().to_str().unwrap())?;
    // Native bindings recorded before this workflow have the two observations.
    db.execute_batch(
        "UPDATE provider_bindings SET metadata_json=json_remove(metadata_json, '$.canFork');
         UPDATE provider_bindings SET metadata_json=json_set(metadata_json,
           '$.observedUserPrompt', json('true'), '$.observedNormalStop', json('true'));",
    )?;
    let input = ForkSessionInput {
        name: format!("  {}  ", "🧪".repeat(64)),
        ..request()
    };
    let source = get_session(&db, "source")?.unwrap();
    let accepted = create(&mut db, "first", &input)?;
    assert_eq!(accepted["sessionId"], "child");
    assert_eq!(accepted["replayed"], false);
    let child = get_session(&db, "child")?.unwrap();
    assert_eq!(child.title, "🧪".repeat(64));
    assert_eq!(
        (
            &child.kind,
            &child.cwd,
            &child.task_id,
            &child.execution_context_id
        ),
        (
            &source.kind,
            &source.cwd,
            &source.task_id,
            &source.execution_context_id
        )
    );
    assert_eq!(get_session(&db, "source")?.unwrap(), source);
    let graph = project_scene_graph(&db, "scene", Some("window"))?;
    let child_node = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["sessionId"] == "child")
        .unwrap();
    assert_eq!(child_node["parentSessionId"], "source");
    assert_eq!(child_node["relationKind"], "forked-from");
    assert_eq!(child_node["forkProgress"]["stage"], "queued");
    assert_eq!(child_node["workStatus"], "starting");
    assert_eq!(
        db.query_row(
            "SELECT split.direction, source.ordinal, child.ordinal
         FROM scene_nodes AS split
         JOIN scene_nodes AS source ON source.parent_node_id=split.id AND source.id='source-node'
         JOIN scene_nodes AS child ON child.parent_node_id=split.id AND child.id='child-node'
         JOIN scenes ON root_node_id=split.id",
            [],
            |r| Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?
            )),
        )?,
        ("horizontal".into(), 0, 1)
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM provider_bindings WHERE session_id='child'",
            [],
            |row| row.get::<_, i64>(0),
        )?,
        0
    );
    assert!(
        create(
            &mut db,
            "same-name",
            &ForkSessionInput {
                submission_key: "separate submission".into(),
                ..input.clone()
            }
        )
        .unwrap_err()
        .downcast_ref::<CanvasError>()
        .is_some_and(|error| error.code == "DUPLICATE_NAME")
    );
    db.execute_batch("UPDATE sessions SET archived_at=4 WHERE id='source'; DELETE FROM session_mounts WHERE session_id='source';")?;
    let target = pending_claude_fork(&db, "child")?.unwrap();
    assert_eq!(target.source_id, "source-provider-id");
    assert_eq!(target.metadata["providerConfigId"], "source-config");
    assert_eq!(target.metadata["permissionMode"], "plan");
    let duplicate = create(&mut db, "second", &input)?;
    assert_eq!(duplicate["sessionId"], "child");
    assert_eq!(duplicate["replayed"], true);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM session_fork_intents", [], |row| row
            .get::<_, i64>(
            0
        ),)?,
        1
    );
    Ok(())
}
