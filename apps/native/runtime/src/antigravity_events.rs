//! Reduce authenticated Antigravity hook and statusline observations for one run.

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{
    antigravity_launch::{LaunchMode, PermissionMode, valid_conversation_id},
    dsh_events::js_whitespace,
};

pub const FORK_COMMAND: &str = "/fork\r";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkStatus {
    Starting,
    Idle,
    Running,
    NeedsInput,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Hook,
    Statusline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryFailure {
    IdentityMismatch,
    ForkTimeout,
    Exited,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryState {
    Fresh,
    AwaitingIdentity,
    Confirmed,
    Failed(RecoveryFailure),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Identity<'a> {
    pub conversation_id: &'a str,
    pub cwd: Option<&'a str>,
    pub model: Option<&'a str>,
    pub permission: Option<PermissionMode>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub event_type: &'static str,
    pub title: &'static str,
    pub subtitle: String,
    pub body: String,
    pub sound: bool,
    pub cooldown_key: &'static str,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticEvent {
    pub kind: &'static str,
    pub provider_event_id: Option<String>,
    pub occurred_at: f64,
    pub payload: Value,
}

#[derive(Debug, PartialEq)]
pub struct Update<'a> {
    pub event_name: Option<&'a str>,
    pub source: Source,
    /// Forward accepted HUD data to the existing HUD projection.
    pub payload: &'a Map<String, Value>,
    pub identity: Option<Identity<'a>>,
    pub work_status: Option<WorkStatus>,
    pub permission_changed: Option<PermissionMode>,
    pub notification: Option<Notification>,
    pub semantic_events: Vec<SemanticEvent>,
    pub transcript_path: Option<&'a str>,
    pub title_candidate: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome<'a> {
    Ignored,
    SendForkCommand,
    IdentityMismatch { expected: String, actual: &'a str },
    Observed(Update<'a>),
}

pub struct Run {
    run_id: String,
    mode: LaunchMode,
    recovery: RecoveryState,
    retired: bool,
    conversation: Option<String>,
    bound_cwd: Option<String>,
    bound_model: Option<String>,
    bound_permission: Option<PermissionMode>,
    slash_sent: bool,
    work_status: WorkStatus,
    last_agent_state: Option<String>,
    error_latched: bool,
    permission: PermissionMode,
    observed_cycle_mode: bool,
    observed_prompt: bool,
    observed_stop: bool,
    inherited_fork: bool,
}

impl Run {
    pub fn new(run_id: impl Into<String>, mode: LaunchMode, permission: PermissionMode) -> Self {
        let recovery = if mode == LaunchMode::Fresh {
            RecoveryState::Fresh
        } else {
            RecoveryState::AwaitingIdentity
        };
        Self {
            run_id: run_id.into(),
            mode,
            recovery,
            retired: false,
            conversation: None,
            bound_cwd: None,
            bound_model: None,
            bound_permission: None,
            slash_sent: false,
            work_status: WorkStatus::Starting,
            last_agent_state: None,
            error_latched: false,
            permission,
            observed_cycle_mode: false,
            observed_prompt: false,
            observed_stop: false,
            inherited_fork: false,
        }
    }

    pub fn conversation_id(&self) -> Option<&str> {
        self.conversation.as_deref()
    }

    pub fn work_status(&self) -> WorkStatus {
        self.work_status
    }

    pub fn recovery_state(&self) -> RecoveryState {
        self.recovery
    }

    pub fn can_fork(&self) -> bool {
        self.conversation.is_some()
            && (self.inherited_fork || self.observed_prompt && self.observed_stop)
    }

    /// Ordinary restore remains pending until a definitive event arrives. A
    /// fork has a bounded identity handshake and can be retried in a new run.
    pub fn deadline_elapsed(&mut self) -> RecoveryState {
        if self.recovery == RecoveryState::AwaitingIdentity
            && matches!(self.mode, LaunchMode::Fork(_))
        {
            self.fail(RecoveryFailure::ForkTimeout);
        }
        self.recovery
    }

    pub fn retire(&mut self) {
        if self.recovery == RecoveryState::AwaitingIdentity {
            self.fail(RecoveryFailure::Exited);
        }
        self.retired = true;
    }

    fn fail(&mut self, failure: RecoveryFailure) {
        self.recovery = RecoveryState::Failed(failure);
        self.retired = true;
    }

    /// The caller authenticates the observation and passes the owning run ID.
    /// `now_ms` supplies the fallback timestamp for hooks without one.
    pub fn ingest<'a>(
        &mut self,
        run_id: &str,
        source: Source,
        raw: &'a Value,
        now_ms: i64,
    ) -> Outcome<'a> {
        if self.retired || run_id != self.run_id {
            return Outcome::Ignored;
        }
        let Some(envelope) = raw.as_object() else {
            return Outcome::Ignored;
        };
        let payload = envelope
            .get("data")
            .and_then(Value::as_object)
            .unwrap_or(envelope);
        let event_name = text(envelope.get("event"))
            .or_else(|| first_text(payload, &["hook_event_name", "hookEventName"]));
        let is_statusline = source == Source::Statusline || event_name.is_none();
        let source = if is_statusline {
            Source::Statusline
        } else {
            Source::Hook
        };
        let id = first_text(
            payload,
            &[
                "session_id",
                "sessionId",
                "conversationId",
                "conversation_id",
            ],
        );
        if id.is_some_and(|id| !valid_conversation_id(id)) {
            return Outcome::Ignored;
        }
        let agent_state = first_text(payload, &["agent_state"]);
        let confirms = event_name != Some("SessionEnd");
        if self.recovery == RecoveryState::AwaitingIdentity {
            let Some(id) = id.filter(|_| confirms) else {
                return Outcome::Ignored;
            };
            match &self.mode {
                LaunchMode::Resume(expected) if expected != id => {
                    let expected = expected.clone();
                    self.fail(RecoveryFailure::IdentityMismatch);
                    return Outcome::IdentityMismatch {
                        expected,
                        actual: id,
                    };
                }
                LaunchMode::Fork(parent) if parent == id => {
                    if is_statusline && agent_state == Some("idle") && !self.slash_sent {
                        self.slash_sent = true;
                        return Outcome::SendForkCommand;
                    }
                    return Outcome::Ignored;
                }
                LaunchMode::Fork(_) if !self.slash_sent => return Outcome::Ignored,
                _ => {}
            }
            self.recovery = RecoveryState::Confirmed;
        }
        // The child run can still receive late observations from the parent.
        if matches!(&self.mode, LaunchMode::Fork(parent) if Some(parent.as_str()) == id) {
            return Outcome::Ignored;
        }
        if !confirms && id.is_some() && id != self.conversation.as_deref() {
            return Outcome::Ignored;
        }

        let reported_permission = match first_text(
            payload,
            &[
                "cycle_mode",
                "cycleMode",
                "permission_mode",
                "permissionMode",
            ],
        ) {
            Some("auto") if self.permission == PermissionMode::Auto => Some(PermissionMode::Auto),
            Some("auto" | "default") => Some(PermissionMode::Default),
            Some("ask") => Some(PermissionMode::Ask),
            Some("bypassPermissions" | "bypass-permissions") => {
                Some(PermissionMode::BypassPermissions)
            }
            Some("accept-edits" | "acceptEdits") => Some(PermissionMode::AcceptEdits),
            Some("plan") => Some(PermissionMode::Plan),
            None if is_statusline
                && (payload.contains_key("cycle_mode")
                    || payload.contains_key("cycleMode")
                    || self.observed_cycle_mode
                        && ["model", "context_window", "agent_state"]
                            .iter()
                            .any(|key| payload.contains_key(*key))) =>
            {
                Some(PermissionMode::Ask)
            }
            _ => None,
        };
        if is_statusline && reported_permission.is_some() {
            self.observed_cycle_mode = true;
        }
        let permission_changed =
            reported_permission.filter(|permission| *permission != self.permission);
        if let Some(permission) = permission_changed {
            self.permission = permission;
        }

        let identity = id.filter(|_| confirms).and_then(|conversation_id| {
            let model = payload.get("model");
            let identity = Identity {
                conversation_id,
                cwd: first_text(payload, &["cwd"]),
                model: text(model).or_else(|| text(model.and_then(|model| model.get("id")))),
                permission: reported_permission,
            };
            let lifecycle = !is_statusline
                && match event_name {
                    Some("PostToolUse" | "PostToolUseFailure") => false,
                    Some("PreToolUse") => self.work_status != WorkStatus::Running,
                    _ => true,
                };
            let changed = self.bind_identity(&identity);
            (changed || lifecycle).then_some(identity)
        });

        let previous_state = self.last_agent_state.as_deref();
        let notification = notification(payload, event_name, previous_state);
        let stopped_with_error = stop_error(payload);
        let work_status = if is_statusline {
            let work_status = match agent_state.filter(|state| Some(*state) != previous_state) {
                Some("initializing" | "authenticating") if previous_state.is_none() => {
                    Some(WorkStatus::Starting)
                }
                Some("working") => {
                    self.error_latched = false;
                    Some(WorkStatus::Running)
                }
                Some("tool_use") => Some(WorkStatus::NeedsInput),
                Some("idle") if !self.error_latched => Some(WorkStatus::Idle),
                _ => None,
            };
            if let Some(state) = agent_state {
                self.last_agent_state = Some(state.to_owned());
            }
            work_status
        } else {
            match event_name {
                Some("PreInvocation" | "PreToolUse") => {
                    self.error_latched = false;
                    Some(WorkStatus::Running)
                }
                Some("Stop") => {
                    self.error_latched = stopped_with_error;
                    Some(if stopped_with_error {
                        WorkStatus::Error
                    } else {
                        WorkStatus::Idle
                    })
                }
                _ => None,
            }
        };
        if let Some(status) = work_status {
            self.work_status = status;
        }
        if !is_statusline && id.is_some() {
            self.observed_prompt |=
                matches!(event_name, Some("PreInvocation" | "UserPromptSubmit"));
            self.observed_stop |= event_name == Some("Stop");
        }
        let occurred_at = envelope
            .get("timestamp")
            .and_then(Value::as_f64)
            .or_else(|| payload.get("timestamp").and_then(Value::as_f64))
            .unwrap_or(now_ms as f64);
        Outcome::Observed(Update {
            event_name,
            source,
            payload,
            identity,
            work_status,
            permission_changed,
            notification,
            semantic_events: if is_statusline {
                Vec::new()
            } else {
                semantic_events(payload, event_name, occurred_at)
            },
            transcript_path: (!is_statusline)
                .then(|| first_text(payload, &["transcriptPath"]))
                .flatten(),
            title_candidate: (!is_statusline && event_name == Some("Stop"))
                .then(|| self.conversation.clone())
                .flatten(),
        })
    }

    fn bind_identity(&mut self, identity: &Identity<'_>) -> bool {
        let mut changed = self.conversation.as_deref() != Some(identity.conversation_id);
        if changed {
            self.inherited_fork =
                self.conversation.is_none() && matches!(self.mode, LaunchMode::Fork(_));
            self.conversation = Some(identity.conversation_id.to_owned());
            self.observed_prompt = false;
            self.observed_stop = false;
            self.bound_cwd = None;
            self.bound_model = None;
            self.bound_permission = None;
        }
        for (reported, bound) in [
            (identity.cwd, &mut self.bound_cwd),
            (identity.model, &mut self.bound_model),
        ] {
            if let Some(reported) = reported.filter(|value| Some(*value) != bound.as_deref()) {
                *bound = Some(reported.to_owned());
                changed = true;
            }
        }
        if let Some(permission) = identity
            .permission
            .filter(|permission| Some(*permission) != self.bound_permission)
        {
            self.bound_permission = Some(permission);
            changed = true;
        }
        changed
    }
}

fn text(value: Option<&Value>) -> Option<&str> {
    let value = value?.as_str()?.trim_matches(js_whitespace);
    (!value.is_empty()).then_some(value)
}

fn first_text<'a>(payload: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| text(payload.get(*key)))
}

fn stop_error(payload: &Map<String, Value>) -> bool {
    first_text(payload, &["terminationReason"]) == Some("ERROR")
        || text(payload.get("error")).is_some()
}

fn semantic_events(
    payload: &Map<String, Value>,
    event_name: Option<&str>,
    occurred_at: f64,
) -> Vec<SemanticEvent> {
    let event = |kind, provider_event_id, payload| SemanticEvent {
        kind,
        provider_event_id,
        occurred_at,
        payload,
    };
    match event_name {
        Some("PreToolUse" | "PostToolUse") => {
            let call = payload.get("toolCall");
            let Some(id) = text(call.and_then(|call| call.get("id")))
                .or_else(|| text(payload.get("toolUseId")))
            else {
                return Vec::new();
            };
            let Some(name) = text(call.and_then(|call| call.get("name")))
                .or_else(|| text(payload.get("toolName")))
            else {
                return Vec::new();
            };
            let args = call
                .and_then(|call| call.get("args"))
                .and_then(Value::as_object)
                .or_else(|| payload.get("toolInput").and_then(Value::as_object));
            let finished = event_name == Some("PostToolUse");
            let kind = if finished {
                "agent.tool-finished"
            } else {
                "agent.tool-started"
            };
            let mut tool = json!({"toolName": name, "input": args.cloned().unwrap_or_default()});
            if let Some(error) = payload.get("error").filter(|_| finished) {
                tool["error"] = error.clone();
            }
            let mut events = vec![event(kind, Some(id.to_owned()), tool)];
            if finished
                && matches!(
                    name,
                    "write_file"
                        | "edit_file"
                        | "replace_file_content"
                        | "multi_replace_file_content"
                )
                && let Some(path) = args
                    .and_then(|args| first_text(args, &["Path", "TargetFile", "file_path", "path"]))
            {
                events.push(event(
                    "file.changed",
                    Some(format!("{id}:file:{path}")),
                    json!({"path": path, "toolName": name}),
                ));
            }
            events
        }
        Some("Stop") => {
            let terminal = if stop_error(payload) {
                "error"
            } else {
                "waiting"
            };
            let mut details = json!({"terminalState": terminal});
            for key in ["terminationReason", "error"] {
                if let Some(value) = text(payload.get(key)) {
                    details[key] = json!(value);
                }
            }
            let id = text(payload.get("conversationId"))
                .map(str::to_owned)
                .or_else(|| fallback_id(payload));
            vec![event(
                "agent.message",
                id.map(|id| format!("stop:{id}")),
                details,
            )]
        }
        _ => Vec::new(),
    }
}

#[cfg(unix)]
fn fallback_id(payload: &Map<String, Value>) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut canonical = Value::Object(payload.clone());
    canonical.sort_all_objects();
    Some(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical).ok()?)
    ))
}

#[cfg(not(unix))]
fn fallback_id(_payload: &Map<String, Value>) -> Option<String> {
    None
}

fn notification(
    payload: &Map<String, Value>,
    event_name: Option<&str>,
    previous_state: Option<&str>,
) -> Option<Notification> {
    let path = first_text(payload, &["cwd"]).unwrap_or_default();
    let project = path.rsplit(['/', '\\']).find(|part| !part.is_empty());
    let notice = |event_type, subtitle, body: &str, limit, cooldown_key| Notification {
        event_type,
        title: "Antigravity",
        subtitle,
        body: compact(body, limit),
        sound: true,
        cooldown_key,
    };
    match event_name {
        Some("Stop") if stop_error(payload) => Some(notice(
            "error",
            project
                .map(|name| format!("Error in {name}"))
                .unwrap_or_else(|| "Error".into()),
            text(payload.get("error")).unwrap_or("Antigravity session encountered an error"),
            200,
            "Stop",
        )),
        Some("Stop") if first_text(payload, &["terminationReason"]) == Some("USER_CANCELED") => {
            None
        }
        Some("Stop") => {
            let fallback = project
                .map(|name| format!("Antigravity session completed in {name}"))
                .unwrap_or_else(|| "Antigravity session completed".into());
            Some(notice(
                "completed",
                project
                    .map(|name| format!("Completed in {name}"))
                    .unwrap_or_else(|| "Completed".into()),
                first_text(
                    payload,
                    &["last_assistant_message", "lastAssistantMessage", "result"],
                )
                .unwrap_or(&fallback),
                200,
                "Stop",
            ))
        }
        Some("PermissionRequest") => {
            let tool = first_text(payload, &["tool_name", "toolName"]).unwrap_or("operation");
            let detail = first_text(payload, &["message", "description"]).or_else(|| {
                [
                    ("tool_input", "command"),
                    ("tool_input", "file_path"),
                    ("toolInput", "command"),
                    ("toolInput", "filePath"),
                ]
                .into_iter()
                .find_map(|(outer, inner)| {
                    text(payload.get(outer).and_then(|input| input.get(inner)))
                })
            });
            let body = detail
                .map(|detail| format!("{tool}: {detail}"))
                .unwrap_or_else(|| format!("{tool} requires approval"));
            Some(notice(
                "permission",
                "Permission".into(),
                &body,
                180,
                "Notification",
            ))
        }
        Some("Notification") => {
            let body = first_text(
                payload,
                &["message", "body", "text", "prompt", "error", "description"],
            )
            .or_else(|| {
                ["notification", "data"]
                    .into_iter()
                    .find_map(|key| text(payload.get(key).and_then(|data| data.get("message"))))
            })
            .unwrap_or("Antigravity needs your attention");
            let lower = body.to_lowercase();
            let (kind, subtitle) = if ["permission", "approve", "approval"]
                .iter()
                .any(|word| lower.contains(word))
            {
                ("permission", "Permission")
            } else if ["complet", "finish", "done", "success"]
                .iter()
                .any(|word| lower.contains(word))
            {
                ("completed", "Completed")
            } else if ["idle", "wait", "input"]
                .iter()
                .any(|word| lower.contains(word))
            {
                ("waiting", "Waiting")
            } else {
                ("attention", "Attention")
            };
            Some(notice(kind, subtitle.into(), body, 180, "Notification"))
        }
        None if previous_state != Some("tool_use")
            && first_text(payload, &["agent_state", "agentState"]) == Some("tool_use") =>
        {
            Some(notice(
                "permission",
                "Permission".into(),
                "Antigravity requires approval",
                180,
                "Notification",
            ))
        }
        _ => None,
    }
}

pub(crate) fn compact(value: &str, limit: usize) -> String {
    let flat = value
        .split(js_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if flat.encode_utf16().count() <= limit {
        return flat;
    }
    let mut units = 0;
    let mut result: String = flat
        .chars()
        .take_while(|character| {
            units += character.len_utf16();
            units < limit
        })
        .collect();
    result.push('…');
    result
}
