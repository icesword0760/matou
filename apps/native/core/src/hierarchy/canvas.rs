use std::{collections::HashSet, error::Error, fmt};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::navigation::{
    activate_session_in_transaction, read_created_path, read_hierarchy, register_window,
    restore_navigation_in_transaction,
};
use crate::{
    session_graph::project_scene_graph,
    storage::{CommandMetadata, DomainEventInput, execute},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasIds {
    pub scene_id: String,
    pub root_node_id: String,
    pub secondary_node_id: String,
    pub session_id: String,
    pub mount_id: String,
    pub relation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCanvasInput {
    pub window_id: String,
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigation: Option<String>,
    pub now: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateShellSiblingInput {
    pub window_id: String,
    pub scene_id: String,
    pub source_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_context_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigation: Option<String>,
    pub now: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForkPlacement {
    #[serde(alias = "Child")]
    Child,
    #[serde(alias = "Sibling")]
    Sibling,
    #[serde(alias = "Peer")]
    Peer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForkWorktreeMode {
    #[serde(alias = "Current")]
    Current,
    #[serde(alias = "New")]
    New,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionInput {
    pub window_id: String,
    pub scene_id: String,
    pub source_session_id: String,
    pub placement: ForkPlacement,
    pub name: String,
    pub worktree_mode: ForkWorktreeMode,
    pub submission_key: String,
    pub now: i64,
}

pub struct CanvasMessages<'a> {
    pub new_canvas: &'a str,
    pub duplicate_scene_name: &'a str,
    pub workspace_path_invalid: &'a str,
    pub detached_session_must_return: &'a str,
}

#[derive(Debug)]
pub struct CanvasError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for CanvasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CanvasError {}

pub fn create_canvas(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &CreateCanvasInput,
    ids: &CanvasIds,
    messages: &CanvasMessages<'_>,
    committed_at: i64,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let window_id = &input.window_id;
        let task_id = &input.task_id;
        let now = input.now;
        register_window(tx, window_id, now)?;
        let navigation = read_hierarchy(tx, window_id)?.navigation;
        let (workspace_id, execution_context_id) = require_task(tx, task_id)?;
        let root: String = tx
            .query_row(
                "SELECT root_directory FROM workspaces WHERE id = ? AND archived_at IS NULL",
                [&workspace_id],
                |row| row.get(0),
            )
            .optional()?
            .context("Workspace does not exist")?;
        assert_workspace_path_available(tx, &workspace_id, messages.workspace_path_invalid)?;

        let title_pinned = input.title.is_some();
        let name = match &input.title {
            Some(title) => {
                let name = required_title(title, "Canvas title")?;
                let duplicate = tx
                    .query_row(
                        "SELECT id FROM scenes
                     WHERE task_id = ? AND name = ? AND title_pinned = 1 AND archived_at IS NULL",
                        params![task_id, name],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if duplicate {
                    return Err(CanvasError {
                        code: "DUPLICATE_SCENE_NAME",
                        message: messages.duplicate_scene_name.into(),
                    }
                    .into());
                }
                name.to_owned()
            }
            None => next_canvas_name(tx, task_id, messages.new_canvas)?,
        };
        let ordinal: i64 = tx.query_row(
            "SELECT COUNT(*) FROM scenes WHERE task_id = ? AND archived_at IS NULL",
            [task_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO scenes (
               id, task_id, name, mode, root_node_id, title_pinned, sort_key,
               layout_revision, created_at, updated_at
             ) VALUES (?, ?, ?, 'tile', ?, ?, ?, 1, ?, ?)",
            params![
                ids.scene_id,
                task_id,
                name,
                ids.root_node_id,
                title_pinned,
                format!("a{ordinal:08}"),
                now,
                now
            ],
        )?;
        tx.execute(
            "INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at) VALUES (?, ?, 'root', 0, ?)",
            params![ids.root_node_id, ids.scene_id, now],
        )?;
        tx.execute(
            "INSERT INTO sessions (
               id, task_id, execution_context_id, kind, status, title, cwd,
               created_at, updated_at, last_activity_at, version
             ) VALUES (?, ?, ?, 'shell', 'created', 'Shell', ?, ?, ?, ?, 1)",
            params![
                ids.session_id,
                task_id,
                execution_context_id,
                root,
                now,
                now,
                now
            ],
        )?;
        mount_session(tx, ids, &ids.scene_id, now)?;
        activate_session_in_transaction(tx, window_id, &ids.session_id, now)?;
        let created = read_created_path(tx, &ids.session_id)?;
        let membership = read_membership(tx, &ids.session_id)?;
        if input.navigation.as_deref() == Some("preserve") {
            restore_navigation_in_transaction(tx, &navigation, now)?;
        }

        let hierarchy = read_hierarchy(tx, window_id)?;
        let graph = project_scene_graph(tx, &ids.scene_id, Some(window_id))?;
        let context = CreationEvents {
            command_id: &command.command_id,
            workspace_id: &workspace_id,
            task_id,
            scene_id: &ids.scene_id,
            session_id: &ids.session_id,
            now,
        };
        events.push(context.event(
            "scene-created",
            "scene.created",
            "scene",
            &ids.scene_id,
            None,
            created["scene"].clone(),
        ));
        context.emit_session_creation(events, &created, membership);
        events.push(context.event(
            "canvas-created",
            "scene.canvas-created",
            "scene",
            &ids.scene_id,
            Some(&ids.session_id),
            json!({"graph": graph}),
        ));
        let mut result = serde_json::to_value(hierarchy)?;
        result["created"] = created;
        result["graph"] = graph;
        Ok(result)
    })?;
    Ok(commit.result)
}

pub fn create_shell_sibling(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &CreateShellSiblingInput,
    ids: &CanvasIds,
    messages: &CanvasMessages<'_>,
    committed_at: i64,
) -> Result<Value> {
    create_session_sibling(
        connection,
        command,
        input,
        ids,
        messages,
        committed_at,
        &SessionSiblingProfile {
            kind: crate::pty::LaunchProfile::Shell,
            cwd: None,
            permission_mode: None,
        },
    )
}

pub struct SessionSiblingProfile<'a> {
    pub kind: crate::pty::LaunchProfile,
    pub cwd: Option<&'a str>,
    pub permission_mode: Option<&'a str>,
}

pub fn create_session_sibling(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &CreateShellSiblingInput,
    ids: &CanvasIds,
    messages: &CanvasMessages<'_>,
    committed_at: i64,
    profile: &SessionSiblingProfile<'_>,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let window_id = &input.window_id;
        let scene_id = &input.scene_id;
        let now = input.now;
        register_window(tx, window_id, now)?;
        let navigation = read_hierarchy(tx, window_id)?.navigation;
        let task_id: String = tx
            .query_row(
                "SELECT task_id FROM scenes WHERE id = ? AND archived_at IS NULL",
                [scene_id],
                |row| row.get(0),
            )
            .optional()?
            .context("Scene does not exist")?;
        let (workspace_id, _) = require_task(tx, &task_id)?;
        assert_workspace_path_available(tx, &workspace_id, messages.workspace_path_invalid)?;
        let source = tx
            .query_row(
                "SELECT sessions.execution_context_id, sessions.cwd
             FROM sessions
             JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
             WHERE sessions.id = ? AND membership.scene_id = ?
               AND (sessions.archived_at IS NULL OR ? IS NOT NULL)",
                params![input.source_session_id, scene_id, input.parent_session_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .context("Session does not exist")?;
        let (execution_context_id, cwd) = match input
            .execution_context_id
            .as_deref()
            .filter(|id| !id.is_empty())
        {
            Some(id) => tx
                .query_row(
                    "SELECT id, cwd FROM execution_contexts
                 WHERE id = ? AND workspace_id = ? AND archived_at IS NULL",
                    params![id, workspace_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .context("ExecutionContext does not exist")?,
            None => source,
        };
        let (source_node, source_window): (Option<String>, Option<String>) = tx
            .query_row(
                "SELECT mounts.scene_node_id, mounts.scene_window_id
             FROM session_mounts AS mounts
             JOIN sessions ON sessions.id = mounts.session_id
             WHERE mounts.scene_id = ? AND mounts.scene_node_id IS NOT NULL
               AND mounts.scene_window_id IS NULL AND sessions.archived_at IS NULL
             ORDER BY
               CASE WHEN mounts.session_id = ? THEN 0
                    WHEN mounts.session_id = ? THEN 1 ELSE 2 END,
               sessions.last_activity_at DESC, mounts.created_at, mounts.id LIMIT 1",
                params![scene_id, input.source_session_id, input.parent_session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("SessionMount does not exist")?;
        let source_node = match (source_node, source_window) {
            (Some(node), None) => node,
            _ => bail!("{}", messages.detached_session_must_return),
        };
        let structural_parent: Option<String> =
            match input.parent_session_id.as_ref().filter(|id| !id.is_empty()) {
                Some(id) => Some(id.clone()),
                None => tx
                    .query_row(
                        "SELECT to_session_id FROM session_relations_current
                 WHERE from_session_id = ? AND relation_kind IN ('derived-from', 'forked-from')",
                        [&input.source_session_id],
                        |row| row.get(0),
                    )
                    .optional()?,
            };
        let (title, title_source) = match &input.title {
            Some(title) => (required_title(title, "Session title")?, "manual"),
            None => (
                match profile.kind {
                    crate::pty::LaunchProfile::Shell => "Shell",
                    crate::pty::LaunchProfile::ClaudeCode => "Claude",
                    crate::pty::LaunchProfile::Codex => "Codex",
                    crate::pty::LaunchProfile::Dsh => "DSH",
                    crate::pty::LaunchProfile::Antigravity => "Antigravity",
                },
                "default",
            ),
        };

        insert_horizontal_mount(tx, ids, scene_id, &source_node, now)?;
        tx.execute(
            "INSERT INTO sessions (
               id, task_id, execution_context_id, kind, status, title, title_source, cwd,
               created_at, updated_at, last_activity_at, version
             ) VALUES (?, ?, ?, ?, 'created', ?, ?, ?, ?, ?, ?, 1)",
            params![
                ids.session_id,
                task_id,
                execution_context_id,
                profile.kind.kind(),
                title,
                title_source,
                profile.cwd.unwrap_or(&cwd),
                now,
                now,
                now
            ],
        )?;
        if profile.kind != crate::pty::LaunchProfile::Shell
            && let Some(mode) = profile.permission_mode
        {
            tx.execute(
                "INSERT INTO provider_bindings (id, session_id, provider, provider_session_id,
                   resume_state, metadata_json, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'unknown', ?, ?, ?)",
                params![
                    format!("pending:{}", ids.session_id),
                    ids.session_id,
                    profile.kind.kind(),
                    format!("pending-{}", ids.session_id),
                    json!({"permissionMode": mode}).to_string(),
                    now,
                    now
                ],
            )?;
        }
        mount_session(tx, ids, scene_id, now)?;
        activate_session_in_transaction(tx, window_id, &ids.session_id, now)?;
        let created = read_created_path(tx, &ids.session_id)?;
        let membership = read_membership(tx, &ids.session_id)?;
        if input.navigation.as_deref() == Some("preserve") {
            restore_navigation_in_transaction(tx, &navigation, now)?;
        }

        let context = CreationEvents {
            command_id: &command.command_id,
            workspace_id: &workspace_id,
            task_id: &task_id,
            scene_id,
            session_id: &ids.session_id,
            now,
        };
        context.emit_session_creation(events, &created, membership);
        if let Some(parent) = structural_parent {
            context.create_relation(
                tx,
                events,
                &ids.relation_id,
                &parent,
                "derived-from",
                json!({}),
            )?;
        }
        let hierarchy = read_hierarchy(tx, window_id)?;
        let graph = project_scene_graph(tx, scene_id, Some(window_id))?;
        events.push(context.event(
            "shell-sibling-created",
            "session.graph-summary-changed",
            "scene",
            scene_id,
            Some(&ids.session_id),
            json!({"graph": graph, "sourceSessionId": input.source_session_id}),
        ));
        let mut result = serde_json::to_value(hierarchy)?;
        result["created"] = created;
        result["graph"] = graph;
        Ok(result)
    })?;
    Ok(commit.result)
}

pub fn create_fork_child(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &ForkSessionInput,
    ids: &CanvasIds,
    messages: &CanvasMessages<'_>,
    committed_at: i64,
) -> Result<Value> {
    create_fork_in_environment(
        connection,
        command,
        input,
        ids,
        messages,
        committed_at,
        &ForkEnvironment::Current,
        false,
    )
}

pub enum ForkEnvironment<'a> {
    Current,
    Existing { id: &'a str, branch: &'a str },
    New { branch: &'a str },
}

pub fn create_fork_in_environment(
    connection: &mut Connection,
    command: &CommandMetadata,
    input: &ForkSessionInput,
    ids: &CanvasIds,
    messages: &CanvasMessages<'_>,
    committed_at: i64,
    environment: &ForkEnvironment<'_>,
    preserve_navigation: bool,
) -> Result<Value> {
    let commit = execute(connection, command, committed_at, |tx, events| {
        let ForkSessionInput {
            window_id,
            scene_id,
            source_session_id,
            placement,
            name,
            worktree_mode,
            submission_key,
            now,
        } = input;
        let now = *now;
        if trim_display_name(submission_key).is_empty() {
            return Err(fork_error("EMPTY_SUBMISSION_KEY", "分叉提交标识不能为空"));
        }
        if let Some(accepted) = tx.query_row(
                "SELECT session_id, stage, error_message FROM session_fork_intents WHERE submission_key = ?",
                [submission_key],
                |row| Ok(json!({
                    "sessionId": row.get::<_, String>(0)?,
                    "stage": row.get::<_, String>(1)?,
                    "error": row.get::<_, Option<String>>(2)?,
                    "replayed": true,
                })),
            ).optional()?
        {
            return Ok(accepted);
        }
        let (task_id, execution_context, kind, cwd, workspace_id) = tx.query_row(
            "SELECT session.task_id, session.execution_context_id, session.kind, session.cwd, task.workspace_id
             FROM sessions AS session
             JOIN session_canvas_memberships AS membership ON membership.session_id = session.id
             JOIN scenes AS scene ON scene.id = membership.scene_id
             JOIN tasks AS task ON task.id = scene.task_id AND task.id = session.task_id
             WHERE session.id = ? AND scene.id = ?
               AND session.archived_at IS NULL AND scene.archived_at IS NULL AND task.archived_at IS NULL",
            params![source_session_id, scene_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                      row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?)),
        ).optional()?.ok_or_else(|| fork_error("SESSION_NOT_IN_CANVAS", "会话不在当前画布中"))?;
        assert_workspace_path_available(tx, &workspace_id, messages.workspace_path_invalid)?;
        let (source_node, source_window): (Option<String>, Option<String>) = tx
            .query_row(
                "SELECT scene_node_id, scene_window_id FROM session_mounts
             WHERE scene_id = ? AND session_id = ? ORDER BY created_at, id LIMIT 1",
                params![scene_id, source_session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| fork_error("SESSION_NOT_IN_CANVAS", "会话不在当前画布中"))?;
        let (Some(source_node), None) = (source_node, source_window) else {
            return Err(fork_error("DETACHED_SOURCE", "请先把会话返回当前画布"));
        };
        let parent: Option<(String, bool)> = tx
            .query_row(
                "SELECT relation.to_session_id, parent.id IS NOT NULL
                 FROM session_relations_current AS relation
                 LEFT JOIN sessions AS parent ON parent.id = relation.to_session_id
                 WHERE relation.from_session_id = ? AND relation.relation_kind IN ('derived-from', 'forked-from')",
                [source_session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let parent_id = parent.as_ref().map(|(id, _)| id.as_str());
        let not_ready = || fork_error("FORK_SOURCE_NOT_READY", "完成首轮 AI 对话后可创建分支");
        let (fork_source_id, execution_context, kind, cwd) = if *placement == ForkPlacement::Sibling
        {
            let parent_id = parent_id
                .ok_or_else(|| fork_error("ROOT_HAS_NO_FORK_PARENT", "根层会话可创建子分支"))?;
            let (execution_context, kind, cwd) = tx
                .query_row(
                    "SELECT execution_context_id, kind, cwd FROM sessions
                     WHERE id = ? AND archived_at IS NULL",
                    [parent_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?
                .ok_or_else(not_ready)?;
            (parent_id, execution_context, kind, cwd)
        } else {
            (source_session_id.as_str(), execution_context, kind, cwd)
        };
        let structural_parent = match placement {
            ForkPlacement::Child | ForkPlacement::Sibling => Some((fork_source_id, "forked-from")),
            ForkPlacement::Peer => parent
                .as_ref()
                .filter(|(_, exists)| *exists)
                .map(|(id, _)| (id.as_str(), "derived-from")),
        };
        if !matches!(kind.as_str(), "claude-code" | "codex" | "antigravity") {
            return Err(not_ready());
        }
        if *worktree_mode == ForkWorktreeMode::New
            && kind != "codex"
            && !matches!(environment, ForkEnvironment::New { .. })
        {
            return Err(fork_error(
                "FORK_WORKTREE_MODE_NOT_IMPLEMENTED",
                "当前仅支持在当前工作树创建分支",
            ));
        }
        let mode = if *worktree_mode == ForkWorktreeMode::New {
            "new"
        } else {
            "current"
        };
        let (provider_id, metadata, restore_state): (String, String, String) = tx
            .query_row(
                "SELECT provider_session_id, metadata_json, restore_state FROM provider_bindings
             WHERE session_id = ? AND provider = ? AND resume_state IN ('available', 'resumed')
               AND validated_at IS NOT NULL AND invalidated_at IS NULL
             ORDER BY updated_at DESC, id DESC LIMIT 1",
                params![fork_source_id, kind],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .ok_or_else(not_ready)?;
        let binding: Value = serde_json::from_str(&metadata).unwrap_or(Value::Null);
        if provider_id.trim().is_empty()
            || restore_state != "none"
            || !crate::session_graph::binding_can_fork(&binding)
            || (*placement != ForkPlacement::Sibling
                && !project_scene_graph(tx, scene_id, Some(window_id))?["nodes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|node| node["sessionId"] == fork_source_id && node["canFork"] == true))
        {
            return Err(not_ready());
        }
        let (execution_context, cwd, existing_worktree, branch_name) = match environment {
            ForkEnvironment::Current => (execution_context, cwd, None, None),
            ForkEnvironment::New { branch } => (execution_context, cwd, None, Some(*branch)),
            ForkEnvironment::Existing { id, branch } => {
                let (context, cwd) = tx.query_row(
                    "SELECT context.id, worktree.worktree_path FROM worktrees AS worktree
                     JOIN execution_contexts AS context ON context.id = worktree.execution_context_id
                     JOIN execution_context_git_states AS git ON git.execution_context_id = context.id
                     WHERE worktree.id = ? AND worktree.branch_name = ? AND context.workspace_id = ?
                       AND context.archived_at IS NULL AND worktree.state IN ('ready', 'dirty', 'retained')
                       AND git.state = 'ready' AND git.branch = ?",
                    params![id, branch, workspace_id, branch],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                ).optional()?.ok_or_else(|| fork_error("WORKTREE_CONFLICT", "指定 Worktree 不可用"))?;
                (context, cwd, Some(*id), Some(*branch))
            }
        };
        let name = trim_display_name(name);
        if name.is_empty() {
            return Err(fork_error("EMPTY_NAME", "请输入分支名称"));
        }
        if name.chars().count() > 64 {
            return Err(fork_error("TOO_LONG_NAME", "分支名称最多 64 个字符"));
        }
        let structural_parent_id = structural_parent.map(|(id, _)| id);
        // Child already checks names in the selected card's level as well.
        let selected_level_parent = if *placement == ForkPlacement::Child {
            parent_id
        } else {
            structural_parent_id
        };
        if tx.prepare(
            "SELECT 1 FROM sessions AS session
             JOIN session_canvas_memberships AS membership ON membership.session_id = session.id
             LEFT JOIN session_relations_current AS relation
               ON relation.from_session_id = session.id AND relation.relation_kind IN ('derived-from', 'forked-from')
             WHERE membership.scene_id = ? AND session.archived_at IS NULL AND session.title = ?
               AND (relation.to_session_id IS ? OR relation.to_session_id = ?)",
        )?.exists(params![scene_id, name, selected_level_parent, structural_parent_id])? {
            return Err(fork_error("DUPLICATE_NAME", format!("同一层已存在“{name}”")));
        }
        let permission = match binding["permissionMode"].as_str() {
            Some(mode @ ("auto" | "acceptEdits" | "plan" | "bypassPermissions")) => mode,
            _ => "default",
        };
        register_window(tx, window_id, now)?;
        let navigation = preserve_navigation
            .then(|| read_hierarchy(tx, window_id))
            .transpose()?
            .map(|hierarchy| hierarchy.navigation);
        insert_horizontal_mount(tx, ids, scene_id, &source_node, now)?;
        tx.execute(
            "INSERT INTO sessions (
               id, task_id, execution_context_id, kind, status, title, title_source, cwd,
               work_status, created_at, updated_at, last_activity_at, version
             ) VALUES (?, ?, ?, ?, 'starting', ?, 'manual', ?, 'starting', ?, ?, ?, 1)",
            params![
                ids.session_id,
                task_id,
                execution_context,
                kind,
                name,
                cwd,
                now,
                now,
                now
            ],
        )?;
        mount_session(tx, ids, scene_id, now)?;
        tx.execute(
            "INSERT INTO session_fork_intents (
               session_id, source_session_id, source_provider, source_provider_session_id,
               state, stage, display_name, worktree_mode, operation_id, submission_key,
               permission_mode, total_steps, created_at, updated_at, worktree_id,
               target_execution_context_id, worktree_path, branch_name
             ) VALUES (?, ?, ?, ?, 'pending', 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                ids.session_id,
                fork_source_id,
                kind,
                provider_id,
                name,
                mode,
                format!("{}{}", crate::sessions::NATIVE_FORK_PREFIX, ids.session_id),
                submission_key,
                permission,
                if mode == "new" { 5 } else { 2 },
                now,
                now,
                Option::<&str>::None,
                existing_worktree.map(|_| &execution_context),
                existing_worktree.map(|_| &cwd),
                branch_name,
            ],
        )?;
        let context = CreationEvents {
            command_id: &command.command_id,
            workspace_id: &workspace_id,
            task_id: &task_id,
            scene_id,
            session_id: &ids.session_id,
            now,
        };
        activate_session_in_transaction(tx, window_id, &ids.session_id, now)?;
        let created = read_created_path(tx, &ids.session_id)?;
        if let Some(navigation) = navigation {
            restore_navigation_in_transaction(tx, &navigation, now)?;
        }
        context.emit_session_creation(events, &created, read_membership(tx, &ids.session_id)?);
        if let Some((parent_id, relation_kind)) = structural_parent {
            context.create_relation(
                tx,
                events,
                &ids.relation_id,
                parent_id,
                relation_kind,
                json!({"worktreeMode": mode}),
            )?;
        }
        let result = json!({"sessionId": ids.session_id, "stage": "queued", "error": null, "replayed": false});
        events.push(context.event(
            "fork-created",
            "session.graph-summary-changed",
            "scene",
            scene_id,
            Some(&ids.session_id),
            json!({"sourceSessionId": fork_source_id, "fork": result}),
        ));
        Ok(result)
    })?;
    if commit.replayed {
        let mut result = commit.result;
        result["replayed"] = json!(true);
        return Ok(result);
    }
    Ok(commit.result)
}

/// The authoritative intent survives runtime and PTY replacement.
pub struct NativeFork {
    pub operation_id: String,
    pub submission_key: String,
    pub source_session_id: String,
    pub source_provider_session_id: String,
    pub worktree_mode: String,
    pub display_name: String,
    pub branch_name: Option<String>,
    pub lease_fence: i64,
    pub worktree: Option<NativeForkWorktree>,
}

pub struct NativeForkWorktree {
    pub id: String,
    pub execution_context_id: String,
    pub repository_root: String,
    pub cwd: String,
    pub branch: String,
    pub base_revision: String,
}

pub struct NativeForkLease<'a> {
    pub operation_id: &'a str,
    pub token: &'a str,
    pub fence: i64,
    pub owner: &'a str,
}

pub fn native_codex_fork(connection: &Connection, session_id: &str) -> Result<Option<NativeFork>> {
    Ok(connection.query_row(
        "SELECT fork.operation_id, fork.submission_key, fork.source_session_id,
                fork.source_provider_session_id, fork.worktree_mode, fork.display_name, fork.lease_fence,
                worktree.id, worktree.execution_context_id, worktree.repository_root,
                worktree.worktree_path, worktree.branch_name, COALESCE(worktree.base_revision, worktree.base_ref, ''), fork.branch_name
         FROM session_fork_intents AS fork
         LEFT JOIN worktrees AS worktree ON worktree.id = fork.worktree_id
            OR (fork.worktree_mode = 'current' AND fork.worktree_id IS NULL
                AND worktree.execution_context_id = fork.target_execution_context_id)
         WHERE fork.session_id = ? AND fork.source_provider = 'codex'
           AND fork.state IN ('pending', 'starting')",
        [session_id],
        |row| Ok(NativeFork {
            operation_id: row.get(0)?, submission_key: row.get(1)?, source_session_id: row.get(2)?,
            source_provider_session_id: row.get(3)?, worktree_mode: row.get(4)?,
            display_name: row.get(5)?, lease_fence: row.get(6)?, branch_name: row.get(13)?,
            worktree: match row.get::<_, Option<String>>(7)? {
                Some(id) => Some(NativeForkWorktree {
                    id, execution_context_id: row.get(8)?, repository_root: row.get(9)?,
                    cwd: row.get(10)?, branch: row.get(11)?, base_revision: row.get(12)?,
                }),
                None => None,
            },
        }),
    ).optional()?)
}

/// Record progress and atomically associate an owned checkout with its child.
pub fn checkpoint_codex_fork(
    connection: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    lease: &NativeForkLease<'_>,
    stage: &str,
    worktree: Option<&NativeForkWorktree>,
    now: i64,
) -> Result<crate::storage::DomainCommit> {
    ensure!(
        matches!(
            stage,
            "creating-worktree"
                | "applying-setup"
                | "binding-session"
                | "restoring-provider"
                | "starting-window"
        ),
        "Invalid Codex Fork stage"
    );
    ensure!(
        worktree.is_none() || stage == "restoring-provider",
        "Invalid worktree binding stage"
    );
    execute(connection, command, now, |tx, events| {
        let updated = tx.execute(
            "UPDATE session_fork_intents
             SET state = 'starting', stage = ?1, attempt = MAX(attempt, 1),
                 attempt_count = MAX(attempt_count, 1), started_at = COALESCE(started_at, ?2),
                 lease_owner = ?3, lease_token = ?4, lease_fence = ?5, last_heartbeat_at = ?2,
                 completed_steps = CASE ?1
                    WHEN 'applying-setup' THEN 1 WHEN 'binding-session' THEN 2
                    WHEN 'restoring-provider' THEN total_steps - 2
                    WHEN 'starting-window' THEN total_steps - 1 ELSE 0 END,
                 updated_at = ?2
             WHERE session_id = ?6 AND operation_id = ?7 AND source_provider = 'codex'
               AND EXISTS (SELECT 1 FROM sessions WHERE id = ?6 AND archived_at IS NULL)
               AND (?8 = 0 OR (stage = 'binding-session' AND worktree_mode = 'new' AND worktree_id IS NULL))
               AND ((state = 'pending' AND lease_fence + 1 = ?5 AND stage = 'queued'
                       AND ?1 IN ('creating-worktree', 'restoring-provider'))
                    OR (state = 'starting' AND lease_owner = ?3 AND lease_token = ?4 AND lease_fence = ?5
                       AND ((stage = 'creating-worktree' AND ?1 = 'applying-setup')
                         OR (stage = 'applying-setup' AND ?1 = 'binding-session')
                         OR (stage = 'binding-session' AND ?1 = 'restoring-provider')
                         OR (stage = 'restoring-provider' AND ?1 = 'starting-window'))))",
            params![stage, now, lease.owner, lease.token, lease.fence, session_id, lease.operation_id, worktree.is_some()],
        )?;
        ensure!(
            updated == 1,
            "Codex Fork lease or stage no longer owns the child"
        );
        let (task_id, local_context, workspace_id): (String, String, String) = tx.query_row(
            "SELECT sessions.task_id, sessions.execution_context_id, tasks.workspace_id
             FROM sessions JOIN tasks ON sessions.task_id = tasks.id WHERE sessions.id = ?",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if let Some(worktree) = worktree {
            bind_native_fork_worktree(
                tx,
                session_id,
                &local_context,
                &workspace_id,
                worktree,
                now,
            )?;
        }
        let created = read_created_path(tx, session_id)?;
        let context = CreationEvents {
            command_id: &command.command_id,
            workspace_id: &workspace_id,
            task_id: &task_id,
            scene_id: created["scene"]["id"].as_str().context("Fork scene")?,
            session_id,
            now,
        };
        let payload = json!({"sessionId": session_id, "stage": stage});
        events.push(context.event(
            "fork-progress",
            "session.fork-progress",
            "session",
            session_id,
            Some(session_id),
            payload.clone(),
        ));
        Ok(payload)
    })
}

/// Bind a checkout created by the control plane before the non-Codex provider
/// launch. The existing launch path still owns the transition out of queued.
pub fn bind_control_fork_worktree(
    connection: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    worktree: &NativeForkWorktree,
    now: i64,
) -> Result<crate::storage::DomainCommit> {
    execute(connection, command, now, |tx, _| {
        let (local_context, workspace): (String, String) = tx.query_row(
            "SELECT sessions.execution_context_id, tasks.workspace_id
             FROM session_fork_intents AS fork
             JOIN sessions ON sessions.id = fork.session_id AND sessions.archived_at IS NULL
             JOIN tasks ON tasks.id = sessions.task_id AND tasks.archived_at IS NULL
             WHERE session_id = ? AND fork.source_provider IN ('claude-code', 'antigravity')
               AND fork.state = 'pending' AND fork.stage = 'queued'
               AND fork.worktree_mode = 'new' AND fork.worktree_id IS NULL AND fork.branch_name = ?",
            params![session_id, worktree.branch],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        bind_native_fork_worktree(tx, session_id, &local_context, &workspace, worktree, now)?;
        Ok(json!({"worktreeId": worktree.id}))
    })
}

fn bind_native_fork_worktree(
    tx: &Connection,
    session_id: &str,
    local_context: &str,
    workspace_id: &str,
    worktree: &NativeForkWorktree,
    now: i64,
) -> Result<()> {
    tx.execute(
        "INSERT INTO execution_contexts(id, workspace_id, kind, cwd, created_at)
                 VALUES (?, ?, 'git-worktree', ?, ?)",
        params![
            worktree.execution_context_id,
            workspace_id,
            worktree.cwd,
            now
        ],
    )?;
    tx.execute(
        "INSERT INTO worktrees(id, execution_context_id, repository_root, worktree_path,
                   branch_name, base_revision, state, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, 'ready', ?, ?)",
        params![
            worktree.id,
            worktree.execution_context_id,
            worktree.repository_root,
            worktree.cwd,
            worktree.branch,
            worktree.base_revision,
            now,
            now
        ],
    )?;
    ensure!(tx.execute(
                "UPDATE session_environment_bindings SET managed_worktree_id = ?, active_target = 'worktree',
                   state = 'ready', updated_at = ? WHERE session_id = ? AND local_execution_context_id = ?
                   AND managed_worktree_id IS NULL",
                params![worktree.id, now, session_id, local_context],
            )? == 1, "Codex Fork environment no longer belongs to its lease");
    tx.execute("UPDATE sessions SET execution_context_id = ?, cwd = ?, updated_at = ?, version = version + 1 WHERE id = ?",
                params![worktree.execution_context_id, worktree.cwd, now, session_id])?;
    tx.execute(
        "UPDATE session_fork_intents SET worktree_id = ?, target_execution_context_id = ?,
                   worktree_path = ?, branch_name = ? WHERE session_id = ?",
        params![
            worktree.id,
            worktree.execution_context_id,
            worktree.cwd,
            worktree.branch,
            session_id
        ],
    )?;
    Ok(())
}

pub fn retry_fork(
    connection: &mut Connection,
    command: &CommandMetadata,
    window_id: &str,
    session_id: &str,
    now: i64,
) -> Result<Value> {
    retry_fork_with_navigation(connection, command, window_id, session_id, now, false)
}

pub fn retry_fork_with_navigation(
    connection: &mut Connection,
    command: &CommandMetadata,
    window_id: &str,
    session_id: &str,
    now: i64,
    preserve_navigation: bool,
) -> Result<Value> {
    let commit = execute(connection, command, now, |tx, events| {
        let submission_key: String = tx
            .query_row(
                "UPDATE session_fork_intents
                 SET state = 'pending', stage = 'queued', error_message = NULL,
                     completed_steps = 0, started_at = NULL, completed_at = NULL,
                     attempt = attempt + 1, attempt_count = attempt_count + 1,
                     lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL,
                     last_heartbeat_at = NULL, updated_at = ?
                 WHERE session_id = ? AND operation_id = ? || session_id AND state = 'failed'
                   AND EXISTS (SELECT 1 FROM sessions WHERE id = session_fork_intents.session_id
                     AND archived_at IS NULL AND status NOT IN ('running', 'starting'))
                   AND NOT EXISTS (SELECT 1 FROM session_runs
                     WHERE session_runs.session_id = session_fork_intents.session_id
                       AND session_runs.status IN ('starting', 'running'))
                 RETURNING submission_key",
                params![now, session_id, crate::sessions::NATIVE_FORK_PREFIX],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| fork_error("FORK_NOT_FAILED", "当前分支无需重试"))?;
        tx.execute(
            "UPDATE sessions SET status = 'starting', work_status = 'starting',
                 updated_at = ?, version = version + 1 WHERE id = ?",
            params![now, session_id],
        )?;
        register_window(tx, window_id, now)?;
        if !preserve_navigation {
            activate_session_in_transaction(tx, window_id, session_id, now)?;
        }
        let created = read_created_path(tx, session_id)?;
        let context = CreationEvents {
            command_id: &command.command_id,
            workspace_id: created["workspace"]["id"]
                .as_str()
                .context("Fork workspace")?,
            task_id: created["task"]["id"].as_str().context("Fork task")?,
            scene_id: created["scene"]["id"].as_str().context("Fork scene")?,
            session_id,
            now,
        };
        let graph = project_scene_graph(tx, context.scene_id, Some(window_id))?;
        events.push(context.event(
            "fork-retrying",
            "session.fork-retrying",
            "session",
            session_id,
            Some(session_id),
            json!({"graph": graph}),
        ));
        Ok(json!({
            "sessionId": session_id, "submissionKey": submission_key,
            "stage": "queued", "forkState": "pending", "error": null, "replayed": false,
        }))
    })?;
    let mut result = commit.result;
    result["replayed"] = json!(commit.replayed);
    Ok(result)
}

fn fork_error(code: &'static str, message: impl Into<String>) -> anyhow::Error {
    CanvasError {
        code,
        message: message.into(),
    }
    .into()
}

fn require_task(connection: &Connection, task_id: &str) -> Result<(String, String)> {
    connection.query_row(
        "SELECT workspace_id, execution_context_id FROM tasks WHERE id = ? AND archived_at IS NULL",
        [task_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?.context("Task does not exist")
}

fn assert_workspace_path_available(
    connection: &Connection,
    workspace_id: &str,
    message: &str,
) -> Result<()> {
    let status: Option<String> = connection
        .query_row(
            "SELECT status FROM workspace_path_state WHERE workspace_id = ?",
            [workspace_id],
            |row| row.get(0),
        )
        .optional()?;
    if status.as_deref() == Some("invalid") {
        return Err(CanvasError {
            code: "WORKSPACE_PATH_INVALID",
            message: message.into(),
        }
        .into());
    }
    Ok(())
}

fn insert_horizontal_mount(
    tx: &Connection,
    ids: &CanvasIds,
    scene_id: &str,
    source_node: &str,
    now: i64,
) -> Result<()> {
    let (parent_node, ordinal): (Option<String>, i64) = tx
        .query_row(
            "SELECT parent_node_id, ordinal FROM scene_nodes WHERE id = ?",
            [source_node],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .context("SceneNode does not exist")?;
    tx.execute(
        "INSERT INTO scene_nodes (
           id, scene_id, parent_node_id, kind, direction, ordinal, created_at
         ) VALUES (?, ?, ?, 'split', 'horizontal', ?, ?)",
        params![ids.secondary_node_id, scene_id, parent_node, ordinal, now],
    )?;
    tx.execute(
        "UPDATE scene_nodes SET parent_node_id = ?, kind = 'mount', direction = NULL, ordinal = 0
         WHERE id = ?",
        params![ids.secondary_node_id, source_node],
    )?;
    tx.execute(
        "INSERT INTO scene_nodes (id, scene_id, parent_node_id, kind, ordinal, created_at)
         VALUES (?, ?, ?, 'mount', 1, ?)",
        params![ids.root_node_id, scene_id, ids.secondary_node_id, now],
    )?;
    tx.execute(
        "UPDATE scenes
         SET root_node_id = CASE WHEN root_node_id = ? THEN ? ELSE root_node_id END,
             layout_revision = layout_revision + 1, updated_at = ?
         WHERE id = ?",
        params![source_node, ids.secondary_node_id, now, scene_id],
    )?;
    Ok(())
}

fn mount_session(connection: &Connection, ids: &CanvasIds, scene_id: &str, now: i64) -> Result<()> {
    connection.execute(
        "INSERT INTO session_mounts (id, scene_id, scene_node_id, session_id, created_at)
         VALUES (?, ?, ?, ?, ?)",
        params![
            ids.mount_id,
            scene_id,
            ids.root_node_id,
            ids.session_id,
            now
        ],
    )?;
    Ok(())
}

fn next_canvas_name(connection: &Connection, task_id: &str, base_name: &str) -> Result<String> {
    let mut query =
        connection.prepare("SELECT name FROM scenes WHERE task_id = ? AND archived_at IS NULL")?;
    let names = query
        .query_map([task_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    if !names.contains(base_name) {
        return Ok(base_name.to_owned());
    }
    for suffix in 2.. {
        let candidate = format!("{base_name} {suffix}");
        if !names.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!()
}

fn required_title<'a>(title: &'a str, label: &str) -> Result<&'a str> {
    let title = trim_display_name(title);
    ensure!(!title.is_empty(), "{label} must not be empty");
    Ok(title)
}

fn trim_display_name(title: &str) -> &str {
    // ECMAScript String.trim includes U+FEFF and does not remove U+0085.
    title.trim_matches(|character| {
        matches!(character,
            '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}'
            | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}'
            | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
        )
    })
}

fn read_membership(connection: &Connection, id: &str) -> Result<Value> {
    connection.query_row(
        "SELECT session_id, scene_id, sibling_created_seq, last_user_interaction_seq, created_at, updated_at
         FROM session_canvas_memberships WHERE session_id = ?",
        [id],
        |row| Ok(json!({
            "sessionId": row.get::<_, String>(0)?,
            "sceneId": row.get::<_, String>(1)?,
            "siblingCreatedSeq": row.get::<_, i64>(2)?,
            "lastUserInteractionSeq": row.get::<_, i64>(3)?,
            "createdAt": row.get::<_, i64>(4)?,
            "updatedAt": row.get::<_, i64>(5)?,
        })),
    ).optional()?.context("SessionCanvasMembership does not exist")
}

struct CreationEvents<'a> {
    command_id: &'a str,
    workspace_id: &'a str,
    task_id: &'a str,
    scene_id: &'a str,
    session_id: &'a str,
    now: i64,
}

impl CreationEvents<'_> {
    fn event(
        &self,
        suffix: &str,
        event_type: &str,
        aggregate_type: &str,
        aggregate_id: &str,
        session_id: Option<&str>,
        payload: Value,
    ) -> DomainEventInput {
        DomainEventInput {
            event_id: format!("{}:{suffix}", self.command_id),
            event_type: event_type.to_owned(),
            aggregate_type: aggregate_type.to_owned(),
            aggregate_id: aggregate_id.to_owned(),
            workspace_id: Some(self.workspace_id.to_owned()),
            task_id: Some(self.task_id.to_owned()),
            session_id: session_id.map(str::to_owned),
            payload,
            schema_version: None,
            required_terminal_sequence: None,
            occurred_at: self.now,
        }
    }

    fn create_relation(
        &self,
        tx: &Connection,
        events: &mut Vec<DomainEventInput>,
        relation_id: &str,
        parent: &str,
        kind: &str,
        metadata: Value,
    ) -> Result<()> {
        let serialized = serde_json::to_string(&metadata)?;
        let sequence = tx
            .prepare(
                "INSERT INTO session_relation_events (
               event_id, relation_id, operation, task_id, from_session_id, to_session_id,
               relation_kind, metadata_json, command_id, occurred_at
             ) VALUES (?, ?, 'created', ?, ?, ?, ?, ?, ?, ?)",
            )?
            .insert(params![
                format!(
                    "{}:{}",
                    self.command_id,
                    if kind == "forked-from" {
                        "fork-relation-created"
                    } else {
                        "derived-relation-created"
                    }
                ),
                relation_id,
                self.task_id,
                self.session_id,
                parent,
                kind,
                serialized,
                self.command_id,
                self.now,
            ])?;
        tx.execute(
            "INSERT INTO session_relations_current (
               relation_id, task_id, from_session_id, to_session_id, relation_kind,
               metadata_json, created_at, updated_at, source_event_sequence
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                relation_id,
                self.task_id,
                self.session_id,
                parent,
                kind,
                serialized,
                self.now,
                self.now,
                sequence
            ],
        )?;
        events.push(self.event(
            "domain-relation-created",
            "session-relation.created",
            "session-relation",
            relation_id,
            Some(self.session_id),
            json!({
                "id": relation_id,
                "taskId": self.task_id,
                "fromSessionId": self.session_id,
                "toSessionId": parent,
                "kind": kind,
                "metadata": metadata,
                "createdAt": self.now,
                "updatedAt": self.now,
            }),
        ));
        Ok(())
    }

    fn emit_session_creation(
        &self,
        events: &mut Vec<DomainEventInput>,
        created: &Value,
        membership: Value,
    ) {
        let session = Some(self.session_id);
        events.push(self.event(
            "session-created",
            "session.created",
            "session",
            self.session_id,
            session,
            created["session"].clone(),
        ));
        events.push(self.event(
            "session-mounted",
            "scene.session-mounted",
            "scene",
            self.scene_id,
            session,
            created["mount"].clone(),
        ));
        events.push(self.event(
            "canvas-membership-created",
            "session.canvas-membership-created",
            "session",
            self.session_id,
            session,
            json!({"membership": membership}),
        ));
    }
}
