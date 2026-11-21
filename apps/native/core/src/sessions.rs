//! Persistent lifecycle of sessions and their process runs.
use std::{fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    hierarchy::Session,
    provider_launch::{
        ClaudeRoleAssignment, ClaudeRoleAssignments, parse_claude_compaction,
        parse_claude_role_assignments, parse_claude_web_search,
    },
    pty::LaunchProfile,
    storage::{CommandMetadata, DomainCommit, DomainEventInput, execute},
};

const AGENT_MODEL_PREFERENCES: &str = "agent.modelPreferences";
const CLAUDE_TIME_ZONE: &str = "claude.timeZone";
pub const LOCALE_FILE: &str = "locale.json";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum LocalePreference {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "zh-CN")]
    Chinese,
    #[serde(rename = "en")]
    English,
}

impl LocalePreference {
    pub fn from_value(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "zh-CN" => Some(Self::Chinese),
            "en" => Some(Self::English),
            _ => None,
        }
    }

    pub fn value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Chinese => "zh-CN",
            Self::English => "en",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InterfaceLocale {
    #[default]
    Chinese,
    English,
}

impl InterfaceLocale {
    pub fn text(self, chinese: &'static str, english: &'static str) -> &'static str {
        match self {
            Self::Chinese => chinese,
            Self::English => english,
        }
    }

    pub fn value(self) -> &'static str {
        self.text("zh-CN", "en")
    }
}

pub fn read_locale_preference(data_root: &Path) -> LocalePreference {
    fs::read(data_root.join(LOCALE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| {
            value["preference"]
                .as_str()
                .and_then(LocalePreference::from_value)
        })
        .unwrap_or_default()
}

pub fn resolve_interface_locale(
    preference: LocalePreference,
    environment: Option<&str>,
    system: Option<&str>,
) -> InterfaceLocale {
    use InterfaceLocale::{Chinese, English};
    match environment {
        Some("en") => English,
        Some("zh-CN") => Chinese,
        _ => match preference {
            LocalePreference::English => English,
            LocalePreference::Chinese => Chinese,
            LocalePreference::System => match system.filter(|locale| !locale.is_empty()) {
                Some(locale) if !locale.to_ascii_lowercase().starts_with("zh") => English,
                _ => Chinese,
            },
        },
    }
}

pub fn current_interface_locale(preference: LocalePreference) -> InterfaceLocale {
    let environment = std::env::var("MATOU_LOCALE").ok();
    let system = (preference == LocalePreference::System
        && !matches!(environment.as_deref(), Some("zh-CN" | "en")))
    .then(system_locale)
    .flatten();
    resolve_interface_locale(preference, environment.as_deref(), system.as_deref())
}

#[cfg(target_os = "macos")]
fn system_locale() -> Option<String> {
    use std::ffi::{CStr, c_char, c_void};

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> *const c_void;
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFStringGetCString(
            string: *const c_void,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(value: *const c_void);
    }

    // The language array owns its elements. Copy the first string before releasing it.
    unsafe {
        let languages = CFLocaleCopyPreferredLanguages();
        if languages.is_null() {
            return None;
        }
        let mut buffer = [0; 128];
        let locale = if CFArrayGetCount(languages) > 0
            && CFStringGetCString(
                CFArrayGetValueAtIndex(languages, 0),
                buffer.as_mut_ptr(),
                128,
                0x0800_0100,
            ) != 0
        {
            CStr::from_ptr(buffer.as_ptr())
                .to_str()
                .ok()
                .map(str::to_owned)
        } else {
            None
        };
        CFRelease(languages);
        locale
    }
}

#[cfg(windows)]
fn system_locale() -> Option<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultLocaleName(name: *mut u16, count: i32) -> i32;
    }
    let mut buffer = [0; 85];
    let length = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    (length > 1).then(|| String::from_utf16_lossy(&buffer[..length as usize - 1]))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn system_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
}

pub fn read_claude_time_zone(db: &Connection) -> Result<String> {
    let row = db
        .query_row(
            "SELECT CAST(value_json AS BLOB) FROM preferences WHERE key = ?",
            [CLAUDE_TIME_ZONE],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    Ok(row
        .and_then(|bytes| serde_json::from_slice::<String>(&bytes).ok())
        .filter(|value| crate::provider_launch::valid_claude_time_zone(value))
        .unwrap_or_else(|| "auto".into()))
}

pub fn save_claude_time_zone(db: &Connection, time_zone: &str, updated_at: i64) -> Result<()> {
    ensure!(
        crate::provider_launch::valid_claude_time_zone(time_zone),
        "Invalid Claude Code time zone"
    );
    db.execute(
        "INSERT INTO preferences (key, value_json, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
        params![CLAUDE_TIME_ZONE, serde_json::to_string(time_zone)?, updated_at],
    )?;
    Ok(())
}

fn validate_model_preferences(
    value: &Value,
) -> Result<(Map<String, Value>, ClaudeRoleAssignments)> {
    let windows = value["contextWindows"]
        .as_object()
        .context("Invalid context window preferences")?;
    ensure!(
        windows.iter().all(|(id, mode)| {
            !id.is_empty()
                && id.encode_utf16().count() <= 256
                && matches!(mode.as_str(), Some("default" | "512k" | "maximum"))
        }),
        "Invalid context window preferences"
    );

    let mut fields = Map::from_iter([
        ("contextWindows".into(), Value::Object(windows.clone())),
        (
            "compaction".into(),
            parse_claude_compaction(&value["compaction"])?,
        ),
    ]);
    if let Some(search) = value.get("claudeWebSearch") {
        fields.insert(
            "claudeWebSearch".into(),
            serde_json::to_value(parse_claude_web_search(search)?)?,
        );
    }
    let assignments = value
        .get("claudeRoleAssignments")
        .map(parse_claude_role_assignments)
        .transpose()?
        .unwrap_or_default();
    Ok((fields, assignments))
}

fn model_preferences(db: &Connection) -> (Map<String, Value>, ClaudeRoleAssignments) {
    let row: Option<String> = db
        .query_row(
            "SELECT value_json FROM preferences WHERE key = ?",
            [AGENT_MODEL_PREFERENCES],
            |row| row.get(0),
        )
        .ok();
    row.and_then(|value| serde_json::from_str::<Value>(&value).ok())
        .and_then(|value| validate_model_preferences(&value).ok())
        .unwrap_or_else(|| {
            (
                Map::from_iter([
                    ("contextWindows".into(), json!({})),
                    ("compaction".into(), default_claude_compaction()),
                ]),
                ClaudeRoleAssignments::default(),
            )
        })
}

fn default_claude_compaction() -> Value {
    json!({"mode": "follow", "effort": "follow"})
}

pub fn read_claude_role_assignments(db: &Connection) -> ClaudeRoleAssignments {
    let (_, assignments) = model_preferences(db);
    assignments
}

pub struct ClaudeRoleLaunchPreferences {
    pub assignments: ClaudeRoleAssignments,
    pub web_search: Option<ClaudeRoleAssignment>,
    pub(crate) context_windows: Map<String, Value>,
    pub compaction: Value,
}

impl Default for ClaudeRoleLaunchPreferences {
    fn default() -> Self {
        Self {
            assignments: ClaudeRoleAssignments::default(),
            web_search: None,
            context_windows: Map::new(),
            compaction: default_claude_compaction(),
        }
    }
}

pub fn read_claude_role_launch_preferences(db: &Connection) -> ClaudeRoleLaunchPreferences {
    let (mut fields, assignments) = model_preferences(db);
    ClaudeRoleLaunchPreferences {
        assignments,
        web_search: fields
            .remove("claudeWebSearch")
            .and_then(|value| serde_json::from_value(value).ok()),
        context_windows: fields["contextWindows"]
            .as_object()
            .cloned()
            .unwrap_or_default(),
        compaction: fields
            .remove("compaction")
            .unwrap_or_else(default_claude_compaction),
    }
}

pub fn save_claude_role_assignments(
    db: &mut Connection,
    assignments: &ClaudeRoleAssignments,
    updated_at: i64,
) -> Result<()> {
    save_claude_model_assignments(db, assignments, None, updated_at)?;
    Ok(())
}

/// The outer option is the patch, and the inner option is the new value.
pub fn save_claude_model_assignments(
    db: &mut Connection,
    assignments: &ClaudeRoleAssignments,
    web_search: Option<Option<&ClaudeRoleAssignment>>,
    updated_at: i64,
) -> Result<Option<ClaudeRoleAssignment>> {
    save_claude_model_preferences(db, assignments, web_search, None, updated_at)
        .map(|saved| saved.0)
}

pub fn save_claude_model_preferences(
    db: &mut Connection,
    assignments: &ClaudeRoleAssignments,
    web_search: Option<Option<&ClaudeRoleAssignment>>,
    compaction: Option<Option<&Value>>,
    updated_at: i64,
) -> Result<(Option<ClaudeRoleAssignment>, Value)> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (mut preferences, _) = model_preferences(&tx);
    preferences.insert(
        "claudeRoleAssignments".into(),
        serde_json::to_value(assignments)?,
    );
    match web_search {
        Some(Some(target)) => {
            preferences.insert("claudeWebSearch".into(), serde_json::to_value(target)?);
        }
        Some(None) => {
            preferences.remove("claudeWebSearch");
        }
        None => {}
    }
    let saved_search = preferences
        .get("claudeWebSearch")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?;
    if let Some(compaction) = compaction {
        let value = compaction
            .map(parse_claude_compaction)
            .transpose()?
            .unwrap_or_else(default_claude_compaction);
        preferences.insert("compaction".into(), value);
    }
    let saved_compaction = preferences["compaction"].clone();
    tx.execute(
        "INSERT INTO preferences (key, value_json, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json,
                                       updated_at = excluded.updated_at",
        params![
            AGENT_MODEL_PREFERENCES,
            serde_json::to_string(&preferences)?,
            updated_at
        ],
    )?;
    tx.commit()?;
    Ok((saved_search, saved_compaction))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateShellSessionInput {
    pub id: String,
    pub task_id: String,
    pub execution_context_id: String,
    pub title: String,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunInput {
    pub id: String,
    pub session_id: String,
    pub runtime_generation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    pub cols: i64,
    pub rows: i64,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRun {
    pub id: String,
    pub session_id: String,
    pub ordinal: i64,
    pub runtime_generation: String,
    pub profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    pub status: String,
    pub cols: i64,
    pub rows: i64,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i64>,
}

impl SessionRun {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            session_id: row.get("session_id")?,
            ordinal: row.get("ordinal")?,
            runtime_generation: row.get("runtime_generation")?,
            profile: row.get("profile")?,
            pid: row.get("pid")?,
            status: row.get("status")?,
            cols: row.get("cols")?,
            rows: row.get("rows")?,
            started_at: row.get("started_at")?,
            ended_at: row.get("ended_at")?,
            exit_code: row.get("exit_code")?,
            signal: row.get("signal")?,
        })
    }
}

pub fn create_shell_session(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &CreateShellSessionInput,
    committed_at: i64,
) -> Result<DomainCommit> {
    let title = trim(&input.title);
    ensure!(!title.is_empty(), "Session title must not be empty");
    execute(db, command, committed_at, |tx, events| {
        let task_workspace: String = tx
            .query_row(
                "SELECT workspace_id FROM tasks WHERE id = ? AND archived_at IS NULL",
                [&input.task_id],
                |row| row.get(0),
            )
            .optional()?
            .with_context(|| format!("Task {} does not exist or is archived", input.task_id))?;
        let context_workspace: Option<String> = tx
            .query_row(
                "SELECT workspace_id FROM execution_contexts WHERE id = ? AND archived_at IS NULL",
                [&input.execution_context_id],
                |row| row.get(0),
            )
            .optional()?;
        ensure!(
            context_workspace.as_deref() == Some(task_workspace.as_str()),
            "Session execution context must belong to the Task Workspace"
        );
        let title_source = if title == "Shell" {
            "default"
        } else {
            "manual"
        };
        tx.execute(
            "INSERT INTO sessions (
               id, task_id, execution_context_id, kind, status, title, title_source,
               created_at, updated_at, last_activity_at, version
             ) VALUES (?, ?, ?, 'shell', 'created', ?, ?, ?, ?, ?, 1)",
            params![
                input.id,
                input.task_id,
                input.execution_context_id,
                title,
                title_source,
                input.now,
                input.now,
                input.now
            ],
        )?;
        let session = require_session(tx, &input.id)?;
        let value = serde_json::to_value(&session)?;
        let mut event = session_event(
            command,
            "session.created",
            &session,
            input.now,
            value.clone(),
        );
        event.event_id = format!("{}:session-created", command.command_id);
        event.workspace_id = Some(task_workspace);
        events.push(event);
        Ok(value)
    })
}

pub fn promote_shell_to_claude(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    now: i64,
) -> Result<DomainCommit> {
    promote_shell_to_agent(db, command, session_id, LaunchProfile::ClaudeCode, now)
}

pub fn promote_shell_to_agent(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    profile: LaunchProfile,
    now: i64,
) -> Result<DomainCommit> {
    let title = match profile {
        LaunchProfile::ClaudeCode => "Claude",
        LaunchProfile::Codex => "Codex",
        LaunchProfile::Dsh => "DSH",
        LaunchProfile::Antigravity => "Antigravity",
        LaunchProfile::Shell => anyhow::bail!("Shell is not an agent profile"),
    };
    execute(db, command, now, |tx, events| {
        let before = require_session(tx, session_id)?;
        ensure!(
            before.kind == "shell" && before.archived_at.is_none(),
            "only an active Shell card can be promoted"
        );
        require_no_active_run(tx, session_id)?;
        tx.execute(
            "UPDATE sessions SET kind = ?, work_status = 'starting',
                 title = CASE WHEN title = 'Shell' OR title = id
                              THEN ? ELSE title END,
                 updated_at = ?, last_activity_at = ?, version = version + 1
             WHERE id = ?",
            params![profile.kind(), title, now, now, session_id],
        )?;
        if profile == LaunchProfile::ClaudeCode {
            tx.execute(
                "UPDATE provider_bindings SET resume_state = 'expired', restore_state = 'none',
                    restore_error = NULL, invalidated_at = ?, updated_at = ?,
                    metadata_json = json_patch(
                        CASE WHEN json_valid(metadata_json) THEN metadata_json ELSE '{}' END,
                        '{\"canFork\":false,\"observedUserPrompt\":false,\"observedNormalStop\":false}'
                    )
                 WHERE session_id = ?",
                params![now, now, session_id],
            )?;
        }
        let session = require_session(tx, session_id)?;
        events.push(session_event(
            command,
            "session.updated",
            &before,
            now,
            json!({"session": session}),
        ));
        Ok(serde_json::to_value(session)?)
    })
}

pub fn return_promoted_claude_to_shell(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    now: i64,
) -> Result<DomainCommit> {
    return_promoted_agent_to_shell(db, command, session_id, LaunchProfile::ClaudeCode, now)
}

pub fn return_promoted_agent_to_shell(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    profile: LaunchProfile,
    now: i64,
) -> Result<DomainCommit> {
    execute(db, command, now, |tx, events| {
        let before = require_session(tx, session_id)?;
        ensure!(
            profile != LaunchProfile::Shell
                && before.kind == profile.kind()
                && before.archived_at.is_none(),
            "only an active {} card can return to Shell",
            profile.kind()
        );
        require_no_active_run(tx, session_id)?;
        tx.execute(
            "UPDATE sessions SET kind = 'shell', work_status = 'idle',
                 title = CASE WHEN title IN ('Claude', 'Codex', 'DSH', 'Antigravity') OR title = id
                              THEN 'Shell' ELSE title END,
                 updated_at = ?, last_activity_at = ?, version = version + 1
             WHERE id = ?",
            params![now, now, session_id],
        )?;
        let session = require_session(tx, session_id)?;
        events.push(session_event(
            command,
            "session.updated",
            &before,
            now,
            json!({"session": session}),
        ));
        Ok(serde_json::to_value(session)?)
    })
}

pub fn start_run(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &StartRunInput,
    committed_at: i64,
) -> Result<DomainCommit> {
    execute(db, command, committed_at, |tx, events| {
        let session = require_session(tx, &input.session_id)?;
        ensure!(
            session.archived_at.is_none(),
            "archived Session cannot start a new run"
        );
        let ordinal: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM session_runs WHERE session_id = ?",
            [&input.session_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO session_runs (
               id, session_id, ordinal, runtime_generation, profile, pid,
               status, cols, rows, started_at
             ) VALUES (?, ?, ?, ?, ?, ?, 'running', ?, ?, ?)",
            params![
                input.id,
                input.session_id,
                ordinal,
                input.runtime_generation,
                session.kind,
                input.pid,
                input.cols,
                input.rows,
                input.now
            ],
        )?;
        tx.execute(
            "UPDATE session_fork_intents
             SET state = 'starting', stage = 'restoring-provider', completed_steps = 1,
                 attempt = MAX(attempt, 1), attempt_count = MAX(attempt_count, 1),
                 started_at = ?, updated_at = ?
             WHERE session_id = ? AND operation_id = ? || session_id AND state = 'pending'",
            params![input.now, input.now, input.session_id, NATIVE_FORK_PREFIX],
        )?;
        tx.execute(
            "UPDATE sessions
             SET status = 'running',
                 work_status = CASE kind
                     WHEN 'claude-code' THEN work_status
                     WHEN 'codex' THEN 'starting'
                     WHEN 'dsh' THEN 'starting'
                     WHEN 'antigravity' THEN 'starting'
                     ELSE 'idle'
                 END,
                 updated_at = ?, last_activity_at = ?,
                 version = version + 1
             WHERE id = ?",
            params![input.now, input.now, input.session_id],
        )?;
        let run = serde_json::to_value(require_run(tx, &input.id)?)?;
        events.push(session_event(
            command,
            "session.run-started",
            &session,
            input.now,
            json!({"run": run}),
        ));
        Ok(run)
    })
}

pub fn finish_run(
    db: &mut Connection,
    command: &CommandMetadata,
    run_id: &str,
    exit_code: i64,
    signal: Option<i64>,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    execute(db, command, committed_at, |tx, events| {
        let before = require_run(tx, run_id)?;
        let session = require_session(tx, &before.session_id)?;
        let owns_work_status = before.profile != "claude-code"
            || tx
                .prepare(
                    "SELECT 1 FROM domain_events WHERE session_id = ?
                 AND event_type = 'provider-binding.recorded'
                 AND json_extract(payload_json, '$.runId') = ?",
                )?
                .exists(params![before.session_id, run_id])?;
        tx.execute(
            "UPDATE session_runs SET status = 'exited', ended_at = ?, exit_code = ?, signal = ?
             WHERE id = ?",
            params![now, exit_code, signal, run_id],
        )?;
        tx.execute(
            "UPDATE sessions
             SET status = CASE WHEN archived_at IS NULL THEN 'exited' ELSE 'archived' END,
                 work_status = CASE WHEN ? AND NOT (EXISTS (
                     SELECT 1 FROM provider_bindings WHERE session_id = sessions.id
                       AND provider = sessions.kind
                       AND ((work_status = 'error' AND restore_state = 'failed')
                         OR (provider = 'codex' AND work_status = 'starting' AND restore_state = 'restoring'))
                 )) THEN 'exited' ELSE work_status END,
                 updated_at = ?, last_activity_at = ?,
                 version = version + 1
             WHERE id = ?",
            params![owns_work_status, now, now, before.session_id],
        )?;
        let run = serde_json::to_value(require_run(tx, run_id)?)?;
        events.push(session_event(
            command,
            "session.run-exited",
            &session,
            now,
            json!({"run": run}),
        ));
        Ok(run)
    })
}

pub struct ProviderBinding {
    pub id: String,
    pub provider_session_id: String,
    pub metadata: Value,
    pub resumable: bool,
    pub restore_state: String,
}

pub type ClaudeBinding = ProviderBinding;

pub(crate) const NATIVE_FORK_PREFIX: &str = "native-fork:";

pub struct ClaudeFork {
    pub source_id: String,
    pub metadata: Value,
}

pub fn pending_claude_fork(db: &Connection, session_id: &str) -> Result<Option<ClaudeFork>> {
    pending_provider_fork(db, session_id, "claude-code")
}

pub fn pending_provider_fork(
    db: &Connection,
    session_id: &str,
    provider: &str,
) -> Result<Option<ClaudeFork>> {
    Ok(db
        .query_row(
            "SELECT fork.source_provider_session_id, fork.permission_mode, binding.metadata_json
         FROM session_fork_intents AS fork
         LEFT JOIN provider_bindings AS binding ON binding.session_id = fork.source_session_id
           AND binding.provider = fork.source_provider
           AND binding.provider_session_id = fork.source_provider_session_id
         WHERE fork.session_id = ? AND fork.operation_id = ? || fork.session_id
           AND fork.state = 'pending' AND fork.source_provider = ?",
            params![session_id, NATIVE_FORK_PREFIX, provider],
            |row| {
                let permission: String = row.get(1)?;
                let stored: Option<String> = row.get(2)?;
                let binding: Value = stored
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or(Value::Null);
                Ok(ClaudeFork {
                    source_id: row.get(0)?,
                    metadata: json!({
                        "providerConfigId": binding["providerConfigId"],
                        "model": binding["model"],
                        "permissionMode": permission,
                    }),
                })
            },
        )
        .optional()?)
}

pub fn unfinished_claude_forks(db: &Connection) -> Result<Vec<String>> {
    let mut query = db.prepare(
        "SELECT fork.session_id FROM session_fork_intents AS fork
         JOIN sessions ON sessions.id = fork.session_id
         WHERE fork.operation_id = ? || fork.session_id AND fork.state = 'starting'
           AND sessions.archived_at IS NULL",
    )?;
    Ok(query
        .query_map([NATIVE_FORK_PREFIX], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn has_unconfirmed_claude_fork(db: &Connection, session_id: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS (SELECT 1 FROM session_fork_intents
         WHERE session_id = ? AND operation_id = ? || session_id AND state <> 'succeeded')",
        params![session_id, NATIVE_FORK_PREFIX],
        |row| row.get(0),
    )?)
}

pub fn fail_claude_fork(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    error: &str,
    now: i64,
) -> Result<DomainCommit> {
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, session_id)?;
        if session.archived_at.is_some()
            || tx.execute(
                "UPDATE session_fork_intents
             SET state = 'failed', stage = 'failed', error_message = ?,
                 completed_at = ?, updated_at = ?, lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL
             WHERE session_id = ? AND operation_id = ? || session_id
               AND state IN ('pending', 'starting')",
                params![error, now, now, session_id, NATIVE_FORK_PREFIX],
            )? == 0
        {
            return Ok(Value::Null);
        }
        tx.execute(
            "UPDATE sessions SET status = 'interrupted', work_status = 'error',
                 updated_at = ?, version = version + 1 WHERE id = ?",
            params![now, session_id],
        )?;
        let payload = json!({
            "result": "fork-failed", "sessionId": session_id,
            "provider": session.kind, "forkError": error,
        });
        events.push(session_event(
            command,
            "session.fork-failed",
            &session,
            now,
            payload.clone(),
        ));
        Ok(payload)
    })
}

pub fn claude_binding(db: &Connection, session_id: &str) -> Result<Option<ClaudeBinding>> {
    Ok(db
        .query_row(
            "SELECT id, provider_session_id, metadata_json, restore_state,
                (resume_state = 'available' AND (validated_at IS NOT NULL
                    OR CASE WHEN json_valid(metadata_json)
                       THEN json_extract(metadata_json, '$.loadedFromCatalog') = 1
                       ELSE 0 END IS TRUE)
                    AND invalidated_at IS NULL AND user_exited_at IS NULL
                    AND restore_state <> 'failed' AND provider_session_id <> '') AS resumable
         FROM provider_bindings WHERE session_id = ? AND provider = 'claude-code'
           AND EXISTS (SELECT 1 FROM sessions WHERE id = session_id AND kind = 'claude-code')
         ORDER BY updated_at DESC, (invalidated_at IS NULL) DESC, id DESC LIMIT 1",
            [session_id],
            |row| {
                let metadata: String = row.get("metadata_json")?;
                Ok(ClaudeBinding {
                    id: row.get("id")?,
                    provider_session_id: row.get("provider_session_id")?,
                    metadata: serde_json::from_str(&metadata).unwrap_or(Value::Null),
                    resumable: row.get("resumable")?,
                    restore_state: row.get("restore_state")?,
                })
            },
        )
        .optional()?)
}

pub fn codex_binding(db: &Connection, session_id: &str) -> Result<Option<ProviderBinding>> {
    runtime_provider_binding(db, session_id, "codex")
}

pub fn antigravity_binding(db: &Connection, session_id: &str) -> Result<Option<ProviderBinding>> {
    runtime_provider_binding(db, session_id, "antigravity")
}

pub fn dsh_binding(db: &Connection, session_id: &str) -> Result<Option<ProviderBinding>> {
    runtime_provider_binding(db, session_id, "dsh")
}

fn runtime_provider_binding(
    db: &Connection,
    session_id: &str,
    provider: &str,
) -> Result<Option<ProviderBinding>> {
    Ok(db
        .query_row(
            "SELECT id, provider_session_id, metadata_json, restore_state,
                (resume_state IN ('available', 'resumed') AND validated_at IS NOT NULL
                    AND invalidated_at IS NULL AND user_exited_at IS NULL
                    AND restore_state <> 'failed') AS resumable
             FROM provider_bindings WHERE session_id = ?1 AND provider = ?2
               AND EXISTS (SELECT 1 FROM sessions WHERE id = ?1 AND kind = ?2)
             ORDER BY CASE WHEN restore_state IN ('restoring', 'failed') THEN 0
                           WHEN resume_state IN ('available', 'resumed')
                             AND validated_at IS NOT NULL AND invalidated_at IS NULL
                             AND user_exited_at IS NULL THEN 1 ELSE 2 END,
                      CASE WHEN resume_state IN ('available', 'resumed')
                             AND invalidated_at IS NULL THEN validated_at END DESC,
                      updated_at DESC, id DESC LIMIT 1",
            params![session_id, provider],
            |row| {
                let metadata: String = row.get("metadata_json")?;
                Ok(ProviderBinding {
                    id: row.get("id")?,
                    provider_session_id: row.get("provider_session_id")?,
                    metadata: serde_json::from_str(&metadata).unwrap_or(Value::Null),
                    resumable: row.get("resumable")?,
                    restore_state: row.get("restore_state")?,
                })
            },
        )
        .optional()?)
}

// Legacy cards can have a run but no binding. Their existing domain event
// records the failure without manufacturing a provider conversation identity.
pub fn unbound_codex_restore(db: &Connection, session_id: &str) -> Result<Option<Value>> {
    let payload: Option<String> = db
        .query_row(
            "SELECT payload_json FROM domain_events WHERE session_id = ?
           AND event_type = 'session.restore-state-changed'
           AND json_extract(payload_json, '$.provider') = 'codex'
         ORDER BY seq DESC LIMIT 1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(payload
        .and_then(|payload| serde_json::from_str::<Value>(&payload).ok())
        .filter(|payload| payload["bindingId"].is_null()))
}

/// A missing permission is distinct from an explicit default on rollback.
pub fn write_provider_permission(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    binding_id: &str,
    permission: Option<&Value>,
    now: i64,
) -> Result<DomainCommit> {
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, session_id)?;
        let provider = session.kind.as_str();
        ensure!(
            matches!(provider, "claude-code" | "codex" | "dsh"),
            "Unsupported permission provider"
        );
        let raw: String = tx.query_row(
            "SELECT metadata_json FROM provider_bindings
             WHERE id = ? AND session_id = ? AND provider = ?",
            params![binding_id, session_id, provider],
            |row| row.get(0),
        )?;
        let mut metadata: serde_json::Map<String, Value> = serde_json::from_str(&raw)?;
        if let Some(permission) = permission {
            metadata.insert("permissionMode".into(), permission.clone());
        } else {
            metadata.remove("permissionMode");
        }
        tx.execute(
            "UPDATE provider_bindings SET metadata_json = ? WHERE id = ?",
            params![serde_json::to_string(&metadata)?, binding_id],
        )?;
        let payload = json!({
            "sessionId": session_id, "bindingId": binding_id, "provider": provider,
            "permissionMode": permission,
        });
        events.push(session_event(
            command,
            "provider-binding.permission-mode-updated",
            &session,
            now,
            payload.clone(),
        ));
        Ok(payload)
    })
}

#[derive(Clone, Copy)]
pub enum ClaudeRestore<'a> {
    Begin(&'a str),
    Fail { binding_id: &'a str, error: &'a str },
    Retry,
    Fresh,
    PermissionFresh,
}

pub struct ClaudeCatalogLoad<'a> {
    pub binding_id: &'a str,
    pub session_id: &'a str,
    pub scene_id: &'a str,
    pub cwd: &'a str,
    pub provider_session_id: &'a str,
    pub title: &'a str,
    pub model: Option<&'a str>,
    pub permission_mode: &'a str,
}

pub fn load_claude_catalog_binding(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ClaudeCatalogLoad<'_>,
    now: i64,
) -> Result<DomainCommit> {
    load_provider_catalog_binding(db, command, input, "claude-code", false, now)
}

pub fn catalog_binding_owners(
    db: &Connection,
    provider: &str,
    provider_ids: &[&str],
) -> Result<Vec<(String, String, String)>> {
    let mut query = db.prepare(
        "SELECT binding.provider_session_id, session.id, session.title
         FROM provider_bindings AS binding
         JOIN sessions AS session ON session.id = binding.session_id
         JOIN tasks ON tasks.id = session.task_id
         JOIN workspaces ON workspaces.id = tasks.workspace_id
         WHERE binding.provider = ?1
           AND (binding.resume_state = 'available'
                OR (?1 = 'codex' AND binding.resume_state = 'resumed'))
           AND binding.invalidated_at IS NULL AND binding.user_exited_at IS NULL
           AND session.kind = ?1 AND session.archived_at IS NULL
           AND tasks.archived_at IS NULL AND workspaces.archived_at IS NULL
           AND binding.provider_session_id IN (SELECT value FROM json_each(?2))
         ORDER BY binding.updated_at DESC, session.id",
    )?;
    let rows = query.query_map(
        params![provider, serde_json::to_string(provider_ids)?],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn ensure_codex_catalog_available(
    db: &Connection,
    session_id: &str,
    provider_session_id: &str,
) -> Result<()> {
    if let Some((_, owner, title)) =
        catalog_binding_owners(db, "codex", &[provider_session_id])?.first()
    {
        ensure!(owner != session_id, "已载入当前卡片");
        bail!("已载入“{title}”");
    }
    Ok(())
}

pub fn load_provider_catalog_binding(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ClaudeCatalogLoad<'_>,
    provider: &str,
    completed_turn: bool,
    now: i64,
) -> Result<DomainCommit> {
    ensure!(
        matches!(provider, "claude-code" | "codex" | "dsh" | "antigravity"),
        "Unsupported catalog provider"
    );
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, input.session_id)?;
        ensure!(session.archived_at.is_none(), "Session does not exist");
        let in_scene: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM session_canvas_memberships AS member
                JOIN scenes ON scenes.id = member.scene_id AND scenes.archived_at IS NULL
                WHERE member.session_id = ? AND member.scene_id = ?)",
            params![input.session_id, input.scene_id],
            |row| row.get(0),
        )?;
        ensure!(
            in_scene
                && session.cwd == input.cwd
                && matches!(
                    session.kind.as_str(),
                    "shell" | "claude-code" | "codex" | "dsh" | "antigravity"
                ),
            "Card is no longer available in this scene and working directory"
        );
        let workspace_id: String = tx
            .query_row(
                "SELECT workspace_id FROM tasks WHERE id = ? AND archived_at IS NULL",
                [&session.task_id],
                |row| row.get(0),
            )
            .optional()?
            .context("Session does not exist")?;
        let running: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM session_runs
                WHERE session_id = ? AND status IN ('starting', 'running'))",
            [input.session_id],
            |row| row.get(0),
        )?;
        ensure!(!running, "Card already has a running process");
        ensure!(
            !has_unconfirmed_claude_fork(tx, input.session_id)?,
            "Card has an unconfirmed fork"
        );

        if provider == "codex" {
            ensure_codex_catalog_available(tx, input.session_id, input.provider_session_id)?;
        }
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT id, metadata_json FROM provider_bindings
                 WHERE session_id = ? AND provider = ? AND provider_session_id = ?",
                params![input.session_id, provider, input.provider_session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let binding_id = existing.as_ref().map_or(input.binding_id, |(id, _)| id);
        let mut metadata: serde_json::Map<String, Value> = existing
            .as_ref()
            .and_then(|(_, metadata)| serde_json::from_str(metadata).ok())
            .unwrap_or_default();
        metadata.remove("invalidationReason");
        metadata.remove("provisional");
        metadata.remove("lastHookEvent");
        if let Some(model) = input.model {
            metadata.insert("model".into(), json!(model));
        } else {
            metadata.remove("model");
        }
        metadata.extend([
            ("permissionMode".into(), json!(input.permission_mode)),
            ("loadedFromCatalog".into(), json!(true)),
            ("spawnRevision".into(), json!(now)),
            (
                "observedUserPrompt".into(),
                json!(provider == "codex" && completed_turn),
            ),
            (
                "observedNormalStop".into(),
                json!(provider == "codex" && completed_turn),
            ),
            (
                "canFork".into(),
                json!(
                    provider == "claude-code"
                        || (provider == "codex" && completed_turn)
                        || (provider == "antigravity"
                            && metadata.get("canFork") == Some(&Value::Bool(true)))
                ),
            ),
        ]);
        tx.execute(
            "UPDATE provider_bindings SET resume_state = 'expired', restore_state = 'none',
                restore_error = NULL, invalidated_at = ?, updated_at = ?
             WHERE session_id = ? AND provider = ? AND id <> ?",
            params![now, now, input.session_id, provider, binding_id],
        )?;
        tx.execute(
            "INSERT INTO provider_bindings (id, session_id, provider, provider_session_id,
                resume_state, restore_state, metadata_json, created_at, updated_at, validated_at)
             VALUES (?, ?, ?, ?, 'available', 'restoring', ?, ?, ?, ?)
             ON CONFLICT(session_id, provider, provider_session_id) DO UPDATE SET
                resume_state = excluded.resume_state, restore_state = excluded.restore_state,
                restore_error = NULL, invalidated_at = NULL, user_exited_at = NULL,
                validated_at = excluded.validated_at,
                metadata_json = excluded.metadata_json, updated_at = excluded.updated_at",
            params![
                binding_id,
                input.session_id,
                provider,
                input.provider_session_id,
                serde_json::to_string(&metadata)?,
                now,
                now,
                now
            ],
        )?;
        tx.execute(
            "UPDATE sessions SET kind = ?,
                 title = CASE WHEN ? = 'antigravity' AND title_source = 'manual' THEN title ELSE ? END,
                 status = 'starting',
                 work_status = 'starting', updated_at = ?, last_activity_at = ?, version = version + 1
             WHERE id = ?",
            params![provider, provider, input.title, now, now, input.session_id],
        )?;
        let mut payload = json!({
            "sessionId": input.session_id, "provider": provider, "bindingId": binding_id,
            "providerSessionId": input.provider_session_id, "restoreState": "restoring",
            "restoreError": null, "spawnRevision": now,
        });
        let mut mode_changed = session_event(
            command,
            "session.mode-changed",
            &session,
            now,
            payload.clone(),
        );
        mode_changed.event_id = format!("{}:session.mode-changed", command.command_id);
        mode_changed.workspace_id = Some(workspace_id.clone());
        events.push(mode_changed);

        let dismissal = json!({
            "operation": "dismiss",
            "replacementKey": format!("provider-restore:{}", session.id),
        });
        let mut notification = session_event(
            command,
            "agent.notification",
            &session,
            now,
            json!({
                "targetSessionId": session.id,
                "provider": provider,
                "runId": format!("restore:{binding_id}"),
                "event": dismissal,
            }),
        );
        notification.event_id = format!("{}:agent.notification", command.command_id);
        notification.workspace_id = Some(workspace_id);
        payload["agentNotification"] = notification_projection(&notification);
        events.push(notification);
        Ok(payload)
    })
}

pub fn change_claude_restore(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    change: ClaudeRestore<'_>,
    now: i64,
) -> Result<Option<DomainCommit>> {
    let Some(binding) = claude_binding(db, session_id)? else {
        return Ok(None);
    };
    let mut metadata = binding.metadata.as_object().cloned().unwrap_or_default();
    let (resume, restore, error, update_session) = match change {
        ClaudeRestore::Begin(id)
            if binding.id == id && binding.resumable && binding.restore_state != "restoring" =>
        {
            ("available", "restoring", None, None)
        }
        ClaudeRestore::Fail { binding_id, error }
            if binding.id == binding_id && binding.resumable =>
        {
            ("failed", "failed", Some(error), Some("error"))
        }
        ClaudeRestore::Retry | ClaudeRestore::Fresh if binding.restore_state == "failed" => {
            let revision = metadata
                .get("spawnRevision")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            metadata.insert("spawnRevision".into(), json!(revision.saturating_add(1)));
            match change {
                ClaudeRestore::Retry => ("available", "restoring", None, Some("starting")),
                _ => ("expired", "none", None, Some("starting")),
            }
        }
        ClaudeRestore::PermissionFresh if !binding.resumable => {
            ("expired", "none", None, Some("starting"))
        }
        _ => return Ok(None),
    };
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, session_id)?;
        ensure!(
            session.archived_at.is_none() && session.kind == "claude-code",
            "Claude restore requires an active Claude Code card"
        );
        tx.execute(
            "UPDATE provider_bindings SET resume_state = ?, restore_state = ?, restore_error = ?,
                 user_exited_at = NULL, metadata_json = ?, updated_at = ?,
                 invalidated_at = CASE WHEN ? = 'available' THEN NULL
                   WHEN ? = 'expired' THEN COALESCE(invalidated_at, ?) ELSE ? END
             WHERE id = ?",
            params![
                resume,
                restore,
                error,
                serde_json::to_string(&metadata)?,
                now,
                resume,
                resume,
                now,
                now,
                binding.id
            ],
        )?;
        if let Some(work_status) = update_session {
            tx.execute(
                "UPDATE sessions SET
                     status = CASE WHEN ? = 'starting' THEN 'starting' ELSE status END,
                     work_status = ?, updated_at = ?, version = version + 1 WHERE id = ?",
                params![work_status, work_status, now, session_id],
            )?;
        }
        let payload = json!({
            "sessionId": session_id, "provider": "claude-code",
            "bindingId": binding.id, "providerSessionId": binding.provider_session_id,
            "restoreState": restore, "restoreError": error,
            "spawnRevision": metadata.get("spawnRevision"),
        });
        events.push(session_event(
            command,
            "session.restore-state-changed",
            &session,
            now,
            payload.clone(),
        ));
        Ok(payload)
    })
    .map(Some)
}

pub fn change_codex_restore(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    change: ClaudeRestore<'_>,
    failure_title: &str,
    now: i64,
) -> Result<Option<DomainCommit>> {
    change_runtime_provider_restore(db, command, session_id, "codex", change, failure_title, now)
}

pub fn change_dsh_restore(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    change: ClaudeRestore<'_>,
    now: i64,
) -> Result<Option<DomainCommit>> {
    change_runtime_provider_restore(db, command, session_id, "dsh", change, "DSH", now)
}

pub fn change_antigravity_restore(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    change: ClaudeRestore<'_>,
    now: i64,
) -> Result<Option<DomainCommit>> {
    change_runtime_provider_restore(
        db,
        command,
        session_id,
        "antigravity",
        change,
        "Antigravity",
        now,
    )
}

fn change_runtime_provider_restore(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    provider: &str,
    change: ClaudeRestore<'_>,
    failure_title: &str,
    now: i64,
) -> Result<Option<DomainCommit>> {
    let binding = runtime_provider_binding(db, session_id, provider)?;
    let target = binding.as_ref().map(|binding| binding.id.as_str());
    let unbound = if binding.is_none() && provider == "codex" {
        unbound_codex_restore(db, session_id)?
    } else {
        None
    };
    let previous = binding
        .as_ref()
        .map(|binding| binding.restore_state.as_str())
        .or_else(|| {
            unbound
                .as_ref()
                .and_then(|event| event["restoreState"].as_str())
        });
    let mut metadata = binding
        .as_ref()
        .and_then(|binding| binding.metadata.as_object())
        .cloned()
        .unwrap_or_default();
    let (resume, restore, error, work) = match change {
        ClaudeRestore::Begin(id)
            if target == Some(id)
                && binding.as_ref().is_some_and(|binding| binding.resumable)
                && previous != Some("restoring") =>
        {
            ("available", "restoring", None, "starting")
        }
        ClaudeRestore::Fail { binding_id, error }
            if target.unwrap_or_default() == binding_id && previous != Some("failed") =>
        {
            ("failed", "failed", Some(error), "error")
        }
        ClaudeRestore::Retry if previous == Some("failed") => {
            ("available", "restoring", None, "starting")
        }
        ClaudeRestore::Fresh if previous == Some("failed") => ("expired", "none", None, "starting"),
        _ => return Ok(None),
    };
    let revision = match change {
        ClaudeRestore::Retry | ClaudeRestore::Fresh => {
            let revision = metadata
                .get("spawnRevision")
                .and_then(Value::as_i64)
                .or_else(|| unbound.as_ref().and_then(|e| e["spawnRevision"].as_i64()))
                .unwrap_or(0)
                .saturating_add(1);
            metadata.insert("spawnRevision".into(), json!(revision));
            Some(revision)
        }
        _ => metadata
            .get("spawnRevision")
            .and_then(Value::as_i64)
            .or_else(|| {
                unbound
                    .as_ref()
                    .and_then(|event| event["spawnRevision"].as_i64())
            }),
    };
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, session_id)?;
        ensure!(
            session.archived_at.is_none() && session.kind == provider,
            if provider == "codex" {
                "Codex restore requires an active Codex card"
            } else if provider == "antigravity" {
                "Antigravity restore requires an active Antigravity card"
            } else {
                "DSH restore requires an active DSH card"
            }
        );
        if matches!(change, ClaudeRestore::Fresh) {
            tx.execute(
                "UPDATE provider_bindings SET resume_state = 'expired', restore_state = 'none',
                     restore_error = NULL, invalidated_at = COALESCE(invalidated_at, ?)
                 WHERE session_id = ? AND provider = ? AND id <> ?",
                params![now, session_id, provider, target.unwrap_or_default()],
            )?;
        }
        if let Some(id) = target {
            tx.execute(
                "UPDATE provider_bindings SET resume_state = ?, restore_state = ?, restore_error = ?,
                    metadata_json = ?, updated_at = ?, user_exited_at = NULL,
                    invalidated_at = CASE WHEN ? = 'available' THEN NULL ELSE ? END
                 WHERE id = ?",
                params![resume, restore, error, serde_json::to_string(&metadata)?, now, resume, now, id],
            )?;
        }
        tx.execute(
            "UPDATE sessions SET work_status = ?, updated_at = ?, version = version + 1
             WHERE id = ?",
            params![work, now, session_id],
        )?;
        let mut payload = json!({
            "sessionId": session_id, "provider": provider, "bindingId": target,
            "providerSessionId": binding.as_ref().map(|binding| &binding.provider_session_id),
            "restoreState": restore, "restoreError": error, "spawnRevision": revision,
        });
        events.push(session_event(
            command, "session.restore-state-changed", &session, now, payload.clone(),
        ));
        let replacement_key = format!("provider-restore:{session_id}");
        let notification = match change {
            ClaudeRestore::Fail { error, .. } => Some(json!({
                "operation": "upsert", "replacementKey": replacement_key, "eventType": "error",
                "title": failure_title, "body": error, "sound": true,
            })),
            ClaudeRestore::Retry | ClaudeRestore::Fresh => Some(json!({
                "operation": "dismiss", "replacementKey": replacement_key,
            })),
            _ => None,
        };
        if let Some(notification) = notification {
            let mut event = session_event(command, "agent.notification", &session, now, json!({
                "targetSessionId": session_id, "provider": provider,
                "runId": format!("restore:{}", target.unwrap_or(session_id)), "event": notification,
            }));
            event.event_id.push_str(":agent.notification");
            event.workspace_id = Some(tx.query_row(
                "SELECT workspace_id FROM tasks WHERE id = ?", [&session.task_id], |row| row.get(0),
            )?);
            payload["agentNotification"] = notification_projection(&event);
            events.push(event);
        }
        Ok(payload)
    }).map(Some)
}

pub struct ProviderIdentity<'a> {
    pub binding_id: &'a str,
    pub session_id: &'a str,
    pub run_id: &'a str,
    pub generation: &'a str,
    pub provider_session_id: &'a str,
    pub expected_id: Option<&'a str>,
    pub launch_metadata: &'a Value,
    pub observed_metadata: &'a Value,
    pub event_name: &'a str,
    pub first_for_run: bool,
}

pub type ClaudeIdentity<'a> = ProviderIdentity<'a>;

pub fn record_claude_identity(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ClaudeIdentity<'_>,
    now: i64,
) -> Result<DomainCommit> {
    record_provider_identity(db, command, input, "claude-code", None, now)
}

pub fn record_codex_identity(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ProviderIdentity<'_>,
    now: i64,
) -> Result<DomainCommit> {
    ensure!(
        input.event_name == "SessionStart" && input.first_for_run,
        "Codex identity requires SessionStart on its current run"
    );
    record_provider_identity(db, command, input, "codex", None, now)
}

pub fn record_codex_fork_readiness(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ProviderIdentity<'_>,
    now: i64,
) -> Result<DomainCommit> {
    ensure!(
        !input.first_for_run
            && input.expected_id.is_some()
            && matches!(input.event_name, "UserPromptSubmit" | "Stop"),
        "Codex Fork readiness requires a normal turn on the confirmed run"
    );
    record_provider_identity(db, command, input, "codex", None, now)
}

pub fn project_dsh_status(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    run_id: &str,
    status: &str,
    title: Option<&str>,
    now: i64,
) -> Result<DomainCommit> {
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, session_id)?;
        ensure!(
            session.kind == "dsh" && session.archived_at.is_none(),
            "DSH status requires an active DSH session"
        );
        let run = require_run(tx, run_id)?;
        ensure!(
            run.session_id == session_id && run.status == "running",
            "DSH run is no longer active"
        );
        tx.execute(
            "UPDATE sessions SET work_status = ?, provider_title = COALESCE(?, provider_title),
                 title = CASE WHEN ? IS NOT NULL AND title_source <> 'manual' THEN ? ELSE title END,
                 title_source = CASE WHEN ? IS NOT NULL AND title_source <> 'manual' THEN 'auto' ELSE title_source END,
                 updated_at = ?, version = version + 1 WHERE id = ?",
            params![status, title, title, title, title, now, session_id],
        )?;
        let payload = json!({"sessionId": session_id, "runId": run_id, "workStatus": status});
        events.push(session_event(
            command,
            "session.work-status-changed",
            &session,
            now,
            payload.clone(),
        ));
        Ok(payload)
    })
}

pub fn record_dsh_identity(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ProviderIdentity<'_>,
    now: i64,
) -> Result<DomainCommit> {
    ensure!(
        input.event_name == "ready",
        "DSH identity requires a persistent ready observation"
    );
    record_provider_identity(db, command, input, "dsh", None, now)
}

pub fn record_antigravity_identity(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ProviderIdentity<'_>,
    now: i64,
) -> Result<DomainCommit> {
    record_provider_identity(db, command, input, "antigravity", None, now)
}

pub fn record_codex_fork_identity(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ProviderIdentity<'_>,
    lease: &crate::hierarchy::canvas::NativeForkLease<'_>,
    now: i64,
) -> Result<DomainCommit> {
    ensure!(
        input.event_name == "SessionStart" && input.first_for_run,
        "Codex Fork requires SessionStart on its current run"
    );
    record_provider_identity(db, command, input, "codex", Some(lease), now)
}

fn record_provider_identity(
    db: &mut Connection,
    command: &CommandMetadata,
    input: &ProviderIdentity<'_>,
    provider: &str,
    lease: Option<&crate::hierarchy::canvas::NativeForkLease<'_>>,
    now: i64,
) -> Result<DomainCommit> {
    let provider_session_id = if let Some(expected) = input.expected_id {
        ensure!(
            expected == input.provider_session_id,
            "Provider session identity mismatch"
        );
        ensure!(
            !matches!(input.event_name, "unknown" | "SessionEnd"),
            "Provider restore requires a lifecycle event that confirms the conversation"
        );
        input.provider_session_id
    } else {
        trim(input.provider_session_id)
    };
    ensure!(
        !provider_session_id.is_empty(),
        "Provider session identity must not be empty"
    );
    execute(db, command, now, |tx, events| {
        let session = require_session(tx, input.session_id)?;
        let run = tx
            .query_row(
                "SELECT id, runtime_generation, status, profile FROM session_runs
             WHERE session_id = ? ORDER BY ordinal DESC LIMIT 1",
                [input.session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        ensure!(
            session.archived_at.is_none()
                && session.kind == provider
                && run
                    .as_ref()
                    .is_some_and(|(id, generation, status, profile)| {
                        id == input.run_id
                            && generation == input.generation
                            && status == "running"
                            && profile == provider
                    }),
            "Provider hook no longer owns its SessionRun"
        );
        let fork: Option<(String, String)> =
            if matches!(provider, "claude-code" | "codex" | "antigravity") {
                tx.query_row(
                    "SELECT state, source_provider_session_id FROM session_fork_intents
             WHERE session_id = ? AND operation_id = ? || session_id AND source_provider = ?
               AND state <> 'succeeded'",
                    params![input.session_id, NATIVE_FORK_PREFIX, provider],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
            } else {
                None
            };
        if provider == "codex" && fork.is_some() {
            let lease = lease.context("Codex Fork identity requires the current lease")?;
            ensure!(
                lease.owner == input.generation
                    && tx.query_row(
                        "SELECT EXISTS (SELECT 1 FROM session_fork_intents WHERE session_id = ?
                   AND operation_id = ? AND lease_owner = ? AND lease_token = ? AND lease_fence = ?
                   AND state = 'starting' AND stage = 'starting-window')",
                        params![
                            input.session_id,
                            lease.operation_id,
                            lease.owner,
                            lease.token,
                            lease.fence
                        ],
                        |row| row.get::<_, bool>(0),
                    )?,
                "Codex Fork lease is stale"
            );
        } else {
            ensure!(
                lease.is_none(),
                "Codex Fork intent no longer owns the child"
            );
        }
        if let Some((state, source_id)) = &fork {
            ensure!(
                state == "starting"
                    && input.first_for_run
                    && input.expected_id.is_none()
                    && !matches!(input.event_name, "unknown" | "SessionEnd"),
                "Fork requires a lifecycle identity from its active launch"
            );
            ensure!(
                provider_session_id != trim(source_id),
                "Fork returned the source conversation"
            );
        }
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT id, metadata_json FROM provider_bindings
             WHERE session_id = ? AND provider = ? AND provider_session_id = ?",
                params![input.session_id, provider, provider_session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if matches!(provider, "codex" | "dsh" | "antigravity") && input.expected_id.is_none() {
            let expired: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM provider_bindings
                   WHERE session_id = ? AND provider = ? AND resume_state = 'expired'
                     AND CASE WHEN provider = 'codex' THEN lower(trim(provider_session_id))
                              ELSE provider_session_id END = ?)",
                params![input.session_id, provider, provider_session_id],
                |row| row.get(0),
            )?;
            ensure!(
                !expired,
                if provider == "codex" {
                    "Fresh Codex run returned an expired conversation identity"
                } else if provider == "antigravity" {
                    "Fresh Antigravity run returned an expired conversation identity"
                } else {
                    "Fresh DSH run returned an expired conversation identity"
                }
            );
        }
        if input.expected_id.is_some() {
            ensure!(
                (if provider == "claude-code" {
                    claude_binding(tx, input.session_id)?
                } else {
                    runtime_provider_binding(tx, input.session_id, provider)?
                })
                .is_some_and(|binding| binding.resumable
                    && binding.provider_session_id == provider_session_id),
                "Provider hook no longer owns its restore binding"
            );
        }
        let binding_id = existing.as_ref().map_or(input.binding_id, |(id, _)| id);
        let mut metadata: serde_json::Map<String, Value> = existing
            .as_ref()
            .and_then(|(_, metadata)| serde_json::from_str(metadata).ok())
            .unwrap_or_default();
        for (key, value) in input.launch_metadata.as_object().into_iter().flatten() {
            if key == "providerConfigId" || (provider == "codex" && key == "permissionMode") {
                metadata.insert(key.clone(), value.clone());
            } else {
                metadata.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        metadata.extend(
            input
                .observed_metadata
                .as_object()
                .into_iter()
                .flatten()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        metadata.remove("invalidationReason");
        metadata.remove("provisional");
        metadata.insert("lastHookEvent".into(), json!(input.event_name));
        if matches!(provider, "claude-code" | "codex") {
            for (key, observed) in [
                ("observedUserPrompt", input.event_name == "UserPromptSubmit"),
                ("observedNormalStop", input.event_name == "Stop"),
            ] {
                metadata.insert(
                    key.into(),
                    Value::Bool(
                        observed || metadata.get(key).and_then(Value::as_bool) == Some(true),
                    ),
                );
            }
            let can_fork =
                metadata["observedUserPrompt"] == true && metadata["observedNormalStop"] == true;
            metadata.insert("canFork".into(), json!(can_fork));
        }
        tx.execute(
            "INSERT INTO provider_bindings (
                 id, session_id, provider, provider_session_id, resume_state,
                 metadata_json, created_at, updated_at, validated_at
             ) VALUES (?, ?, ?, ?, 'available', ?, ?, ?, ?)
             ON CONFLICT(session_id, provider, provider_session_id) DO UPDATE SET
                 resume_state = 'available', metadata_json = excluded.metadata_json,
                 updated_at = excluded.updated_at, validated_at = excluded.validated_at,
                 invalidated_at = NULL, restore_state = 'none', restore_error = NULL",
            params![
                binding_id,
                input.session_id,
                provider,
                provider_session_id,
                serde_json::to_string(&metadata)?,
                now,
                now,
                now
            ],
        )?;
        if matches!(provider, "codex" | "dsh" | "antigravity") {
            tx.execute(
                "UPDATE provider_bindings SET resume_state = 'expired', restore_state = 'none',
                     restore_error = NULL, invalidated_at = COALESCE(invalidated_at, ?)
                 WHERE session_id = ? AND provider = ? AND id <> ?",
                params![now, input.session_id, provider, binding_id],
            )?;
        }
        if fork.is_some() {
            tx.execute(
                "UPDATE session_fork_intents SET state = 'succeeded', stage = 'succeeded',
                     completed_steps = total_steps, completed_at = ?, updated_at = ?,
                     lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL
                 WHERE session_id = ?",
                params![now, now, input.session_id],
            )?;
            tx.execute(
                "UPDATE sessions SET work_status = 'idle', updated_at = ?, version = version + 1
                 WHERE id = ? AND work_status = 'starting'",
                params![now, input.session_id],
            )?;
        }
        if matches!(provider, "codex" | "dsh" | "antigravity") && input.first_for_run {
            tx.execute(
                "UPDATE sessions SET work_status = 'idle', updated_at = ?, version = version + 1
                 WHERE id = ?",
                params![now, input.session_id],
            )?;
        }
        // Publish only the fields owned by this observation. Existing binding
        // metadata can contain fields from other versions of the runtime.
        let mut public_metadata = serde_json::Map::new();
        for key in [
            "providerConfigId",
            "model",
            "permissionMode",
            "loadedFromCatalog",
            "cwd",
            "lastHookEvent",
            "observedUserPrompt",
            "observedNormalStop",
            "canFork",
        ] {
            if let Some(value) = metadata.get(key) {
                public_metadata.insert(key.into(), value.clone());
            }
        }
        let payload = json!({
            "sessionId": input.session_id, "runId": input.run_id,
            "provider": provider, "providerSessionId": provider_session_id,
            "bindingId": binding_id, "metadata": public_metadata,
            "restoreState": "none", "restoreError": null,
            "spawnRevision": metadata.get("spawnRevision")
                .or_else(|| input.launch_metadata.get("spawnRevision")),
        });
        events.push(session_event(
            command,
            "provider-binding.recorded",
            &session,
            now,
            payload.clone(),
        ));
        if input.expected_id.is_some() && input.first_for_run {
            let mut event = session_event(
                command,
                "session.restore-state-changed",
                &session,
                now,
                payload.clone(),
            );
            event.event_id.push_str(":restore");
            events.push(event);
        }
        let cwd = input.observed_metadata["cwd"]
            .as_str()
            .unwrap_or(&session.cwd);
        if cwd != session.cwd {
            tx.execute(
                "UPDATE sessions SET cwd = ?, updated_at = ?, version = version + 1
                 WHERE id = ?",
                params![cwd, now, input.session_id],
            )?;
        }
        if cwd != session.cwd || input.first_for_run {
            let work_status: String = tx.query_row(
                "SELECT work_status FROM sessions WHERE id = ?",
                [input.session_id],
                |row| row.get(0),
            )?;
            let mut event = session_event(
                command,
                "session.mode-changed",
                &session,
                now,
                json!({"runId": input.run_id, "provider": provider, "workStatus": work_status}),
            );
            event.event_id.push_str(":mode");
            events.push(event);
        }
        Ok(payload)
    })
}

/// Publish after the owner has accepted the hook's conversation identity.
pub fn publish_claude_notification(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    run_id: &str,
    generation: &str,
    event: Value,
    now: i64,
) -> Result<DomainCommit> {
    publish_provider_notification(
        db,
        command,
        session_id,
        run_id,
        generation,
        "claude-code",
        event,
        now,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn publish_provider_notification(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    run_id: &str,
    generation: &str,
    provider: &str,
    event: Value,
    now: i64,
) -> Result<DomainCommit> {
    execute(db, command, now, |tx, events| {
        let owner: Option<(String, String)> = tx
            .query_row(
                "SELECT tasks.workspace_id, sessions.task_id
                 FROM sessions JOIN tasks ON tasks.id = sessions.task_id
                 JOIN session_runs ON session_runs.id = (
                     SELECT id FROM session_runs WHERE session_id = sessions.id
                     ORDER BY ordinal DESC LIMIT 1
                 )
                 WHERE sessions.id = ? AND sessions.archived_at IS NULL
                   AND sessions.kind = ? AND session_runs.id = ?
                   AND session_runs.runtime_generation = ?
                   AND session_runs.status = 'running' AND session_runs.profile = ?",
                params![session_id, provider, run_id, generation, provider],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((workspace_id, task_id)) = owner else {
            return Ok(json!({"published": false}));
        };
        ensure!(event.is_object(), "Expected a notification object");
        let notification = DomainEventInput {
            event_id: command.command_id.clone(),
            event_type: "agent.notification".into(),
            aggregate_type: "session".into(),
            aggregate_id: session_id.into(),
            workspace_id: Some(workspace_id),
            task_id: Some(task_id),
            session_id: Some(session_id.into()),
            payload: json!({
                "targetSessionId": session_id,
                "runId": run_id,
                "provider": provider,
                "event": event,
            }),
            schema_version: Some(1),
            required_terminal_sequence: None,
            occurred_at: now,
        };
        let projection = notification_projection(&notification);
        events.push(notification);
        Ok(json!({"published": true, "agentNotification": projection}))
    })
}

fn notification_projection(notification: &DomainEventInput) -> Value {
    let mut event = notification.payload["event"].clone();
    event["eventId"] = json!(notification.event_id);
    event["workspaceId"] = json!(notification.workspace_id);
    event["taskId"] = json!(notification.task_id);
    event["sessionId"] = json!(notification.session_id);
    event["occurredAt"] = json!(notification.occurred_at);
    event
}

pub fn interrupt_run(
    db: &mut Connection,
    command: &CommandMetadata,
    run_id: &str,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    execute(db, command, committed_at, |tx, events| {
        let (run, event) = interrupt_run_in_transaction(tx, command, run_id, now)?;
        events.push(event);
        Ok(run)
    })
}

pub(crate) fn interrupt_run_in_transaction(
    tx: &Connection,
    command: &CommandMetadata,
    run_id: &str,
    now: i64,
) -> Result<(Value, DomainEventInput)> {
    let before = require_run(tx, run_id)?;
    let session = require_session(tx, &before.session_id)?;
    ensure!(
        matches!(before.status.as_str(), "starting" | "running"),
        "only an active SessionRun can be interrupted"
    );
    tx.execute(
        "UPDATE session_runs SET status = 'interrupted', ended_at = ? WHERE id = ?",
        params![now, run_id],
    )?;
    tx.execute(
        "UPDATE sessions SET
               status = CASE WHEN archived_at IS NULL THEN 'interrupted' ELSE status END,
               work_status = CASE WHEN archived_at IS NULL THEN 'interrupted' ELSE work_status END,
               updated_at = ?, last_activity_at = ?, version = version + 1
             WHERE id = ?",
        params![now, now, before.session_id],
    )?;
    let run = serde_json::to_value(require_run(tx, run_id)?)?;
    let event = session_event(
        command,
        "session.run-interrupted",
        &session,
        now,
        json!({"run": run}),
    );
    Ok((run, event))
}

pub fn rename_session(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    title: &str,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    let title = trim(title);
    ensure!(!title.is_empty(), "Session title must not be empty");
    execute(db, command, committed_at, |tx, events| {
        let before = require_session(tx, session_id)?;
        ensure!(
            before.archived_at.is_none(),
            "archived Session cannot be renamed"
        );
        tx.execute(
            "UPDATE sessions SET title = ?, title_source = 'manual', updated_at = ?,
               version = version + 1 WHERE id = ?",
            params![title, now, session_id],
        )?;
        let session = serde_json::to_value(require_session(tx, session_id)?)?;
        events.push(session_event(
            command,
            "session.updated",
            &before,
            now,
            json!({"session": session}),
        ));
        Ok(session)
    })
}

pub fn observe_codex_title(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    run_id: &str,
    generation: &str,
    provider_session_id: &str,
    title: &str,
    now: i64,
) -> Result<Option<DomainCommit>> {
    observe_provider_title(
        db,
        command,
        session_id,
        run_id,
        generation,
        "codex",
        provider_session_id,
        title,
        now,
    )
}

pub fn observe_provider_title(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    run_id: &str,
    generation: &str,
    provider: &str,
    provider_session_id: &str,
    title: &str,
    now: i64,
) -> Result<Option<DomainCommit>> {
    let title = trim(title);
    let before = require_session(db, session_id)?;
    let manual = before.title_source.as_deref() == Some("manual");
    // The serialized owner is the sole writer. Skip unchanged observations
    // before opening the command transaction, including its replay receipt.
    if title.is_empty()
        || (before.provider_title.as_deref() == Some(title)
            && (manual
                || (before.title_source.as_deref() == Some("auto") && before.title == title)))
    {
        return Ok(None);
    }
    execute(db, command, now, |tx, events| {
        let owns_run: bool = tx.query_row(
            "SELECT EXISTS (
               SELECT 1 FROM session_runs
               WHERE session_id = ?1 AND id = ?2 AND runtime_generation = ?3
                 AND profile = ?4 AND status = 'running'
                 AND ordinal = (SELECT max(ordinal) FROM session_runs WHERE session_id = ?1)
             )",
            params![session_id, run_id, generation, provider],
            |row| row.get(0),
        )?;
        ensure!(
            before.archived_at.is_none()
                && owns_run
                && runtime_provider_binding(tx, session_id, provider)?.is_some_and(|binding| {
                    binding.resumable && binding.provider_session_id == provider_session_id
                }),
            "{provider} title no longer belongs to the active SessionRun"
        );
        write_provider_title(tx, command, events, &before, title, manual, now)
    })
    .map(Some)
}

pub fn restore_codex_title(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    now: i64,
) -> Result<Option<DomainCommit>> {
    let before = require_session(db, session_id)?;
    ensure!(
        before.kind == "codex" && before.archived_at.is_none(),
        "automatic title can only be restored for a current Codex card"
    );
    let Some(title) = before
        .provider_title
        .as_deref()
        .map(trim)
        .filter(|title| !title.is_empty() && before.title_source.as_deref() == Some("manual"))
    else {
        return Ok(None);
    };
    execute(db, command, now, |tx, events| {
        write_provider_title(tx, command, events, &before, title, false, now)
    })
    .map(Some)
}

fn write_provider_title(
    tx: &Connection,
    command: &CommandMetadata,
    events: &mut Vec<DomainEventInput>,
    before: &Session,
    provider_title: &str,
    manual: bool,
    now: i64,
) -> Result<Value> {
    let (title, source) = if manual {
        (before.title.as_str(), "manual")
    } else {
        (provider_title, "auto")
    };
    tx.execute(
        "UPDATE sessions SET title = ?, title_source = ?, provider_title = ?,
             updated_at = ?, version = version + 1 WHERE id = ?",
        params![title, source, provider_title, now, before.id],
    )?;
    let session = serde_json::to_value(require_session(tx, &before.id)?)?;
    events.push(session_event(
        command,
        "session.updated",
        before,
        now,
        json!({"session": session}),
    ));
    Ok(session)
}

pub fn update_cwd(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    cwd: &str,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    let cwd = trim(cwd);
    ensure!(!cwd.is_empty(), "Session cwd must not be empty");
    execute(db, command, committed_at, |tx, events| {
        let before = require_session(tx, session_id)?;
        ensure!(
            before.archived_at.is_none(),
            "archived Session cwd cannot be modified"
        );
        tx.execute(
            "UPDATE sessions SET cwd = ?, updated_at = ?, last_activity_at = ?,
               version = version + 1 WHERE id = ?",
            params![cwd, now, now, session_id],
        )?;
        let session = serde_json::to_value(require_session(tx, session_id)?)?;
        events.push(session_event(
            command,
            "session.cwd-updated",
            &before,
            now,
            json!({"cwd": cwd}),
        ));
        Ok(session)
    })
}

pub fn archive_session(
    db: &mut Connection,
    command: &CommandMetadata,
    session_id: &str,
    now: i64,
    committed_at: i64,
) -> Result<DomainCommit> {
    execute(db, command, committed_at, |tx, events| {
        let before = require_session(tx, session_id)?;
        require_no_active_run(tx, session_id)?;
        tx.execute(
            "UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?,
               version = version + 1 WHERE id = ?",
            params![now, now, session_id],
        )?;
        let session = serde_json::to_value(require_session(tx, session_id)?)?;
        events.push(session_event(
            command,
            "session.archived",
            &before,
            now,
            json!({"archivedAt": now}),
        ));
        Ok(session)
    })
}

pub(crate) fn require_no_active_run(db: &Connection, session_id: &str) -> Result<()> {
    let active = db
        .prepare(
            "SELECT 1 FROM session_runs WHERE session_id = ? AND status IN ('starting', 'running')",
        )?
        .exists([session_id])?;
    ensure!(
        !active,
        "active SessionRuns must be stopped before archiving a Session"
    );
    Ok(())
}

pub fn get_session(db: &Connection, id: &str) -> Result<Option<Session>> {
    Ok(db
        .query_row(
            "SELECT * FROM sessions WHERE id = ?",
            [id],
            Session::from_row,
        )
        .optional()?)
}

pub fn list_sessions(db: &Connection) -> Result<Vec<Session>> {
    let mut query = db.prepare("SELECT * FROM sessions ORDER BY created_at, id")?;
    Ok(query
        .query_map([], Session::from_row)?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn get_run(db: &Connection, id: &str) -> Result<Option<SessionRun>> {
    Ok(db
        .query_row(
            "SELECT * FROM session_runs WHERE id = ?",
            [id],
            SessionRun::from_row,
        )
        .optional()?)
}

pub fn list_runs(db: &Connection, session_id: &str) -> Result<Vec<SessionRun>> {
    let mut query =
        db.prepare("SELECT * FROM session_runs WHERE session_id = ? ORDER BY ordinal")?;
    Ok(query
        .query_map([session_id], SessionRun::from_row)?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn list_all_runs(db: &Connection) -> Result<Vec<SessionRun>> {
    let mut query = db.prepare("SELECT * FROM session_runs ORDER BY session_id, ordinal")?;
    Ok(query
        .query_map([], SessionRun::from_row)?
        .collect::<rusqlite::Result<_>>()?)
}

fn require_session(db: &Connection, id: &str) -> Result<Session> {
    get_session(db, id)?.context("Session does not exist")
}

fn require_run(db: &Connection, id: &str) -> Result<SessionRun> {
    get_run(db, id)?.context("SessionRun does not exist")
}

fn session_event(
    command: &CommandMetadata,
    event_type: &str,
    session: &Session,
    occurred_at: i64,
    payload: Value,
) -> DomainEventInput {
    DomainEventInput {
        event_id: command.command_id.clone(),
        event_type: event_type.to_owned(),
        aggregate_type: "session".into(),
        aggregate_id: session.id.clone(),
        workspace_id: None,
        task_id: Some(session.task_id.clone()),
        session_id: Some(session.id.clone()),
        payload,
        schema_version: None,
        required_terminal_sequence: None,
        occurred_at,
    }
}

fn trim(text: &str) -> &str {
    // ECMAScript String.trim includes U+FEFF and does not remove U+0085.
    text.trim_matches(|character| {
        matches!(character,
            '\u{0009}'..='\u{000D}' | ' ' | '\u{00A0}' | '\u{1680}'
            | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}'
            | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
        )
    })
}
