//! Reduce authenticated hook payloads for one run. PTY bytes and history
//! records have no entry point into the live identity handshake.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::{Map, Value};

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DshWorkStatus {
    #[default]
    Idle,
    Running,
    NeedsInput,
    Error,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DshIdentity<'a> {
    pub session_id: &'a str,
    pub persisted: bool,
    pub cwd: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshNotification {
    pub event_type: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub body: String,
    pub sound: bool,
    pub cooldown_key: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DshUpdate<'a> {
    pub session_id: &'a str,
    pub work_status: DshWorkStatus,
    pub identity: Option<DshIdentity<'a>>,
    pub hud: Option<&'a Map<String, Value>>,
    pub title: Option<&'a str>,
    pub notification: Option<DshNotification>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DshEventOutcome<'a> {
    Ignored,
    IdentityMismatch {
        expected_session_id: String,
        actual_session_id: &'a str,
    },
    Observed(DshUpdate<'a>),
}

pub struct DshEvents {
    run_id: String,
    expected_session_id: Option<String>,
    identity_rejected: bool,
    session_id: Option<String>,
    persisted: Option<bool>,
    generation: i64,
    sequence: i64,
    pending: HashSet<String>,
    status: DshWorkStatus,
    turn_error: bool,
    turn: Option<i64>,
    title: Option<String>,
}

impl DshEvents {
    pub fn new(run_id: impl Into<String>, expected_session_id: Option<&str>) -> Self {
        Self {
            run_id: run_id.into(),
            expected_session_id: expected_session_id.map(str::to_owned),
            identity_rejected: false,
            session_id: None,
            persisted: None,
            generation: 0,
            sequence: 0,
            pending: HashSet::new(),
            status: DshWorkStatus::Idle,
            turn_error: false,
            turn: None,
            title: None,
        }
    }

    /// Call only after authenticating the hook and checking that this run still
    /// owns the card. A mismatched restore retires the reducer for this run.
    pub fn ingest<'a>(&mut self, payload: &'a Value) -> DshEventOutcome<'a> {
        if self.identity_rejected {
            return DshEventOutcome::Ignored;
        }
        let Some(event) = Observation::parse(payload, &self.run_id) else {
            return DshEventOutcome::Ignored;
        };
        if event.sequence <= self.sequence
            || event.generation < self.generation
            || (event.generation > self.generation && !event.ready)
            || (event.generation == self.generation
                && self.session_id.as_deref() != Some(event.session_id))
        {
            return DshEventOutcome::Ignored;
        }

        let identity_changed = self.session_id.as_deref() != Some(event.session_id);
        let identity = if event.ready {
            if let Some(expected) = &self.expected_session_id {
                if event.session_id != expected {
                    self.identity_rejected = true;
                    return DshEventOutcome::IdentityMismatch {
                        expected_session_id: expected.clone(),
                        actual_session_id: event.session_id,
                    };
                }
                if !event.persisted {
                    return DshEventOutcome::Ignored;
                }
            }
            self.expected_session_id = None;
            if identity_changed {
                self.title = None;
            }
            if identity_changed || self.persisted != Some(event.persisted) {
                self.persisted = Some(event.persisted);
                Some(DshIdentity {
                    session_id: event.session_id,
                    persisted: event.persisted,
                    cwd: event.cwd,
                })
            } else {
                None
            }
        } else {
            None
        };

        if event.generation > self.generation {
            self.generation = event.generation;
            self.session_id = Some(event.session_id.to_owned());
            self.pending.clear();
            self.turn = None;
            self.turn_error = false;
            self.status = DshWorkStatus::Idle;
        }
        self.sequence = event.sequence;
        let notification = self.apply(&event);
        let work_status = if self.turn_error {
            DshWorkStatus::Error
        } else if !self.pending.is_empty() {
            DshWorkStatus::NeedsInput
        } else {
            self.status
        };
        let hud = payload["hud"].as_object();
        let title = hud
            .and_then(|hud| hud.get("title"))
            .and_then(crate::provider_hook::nonempty)
            .filter(|title| self.title.as_deref() != Some(*title));
        if let Some(title) = title {
            self.title = Some(title.to_owned());
        }
        DshEventOutcome::Observed(DshUpdate {
            session_id: event.session_id,
            work_status,
            identity,
            hud,
            title,
            notification,
        })
    }

    fn apply(&mut self, event: &Observation<'_>) -> Option<DshNotification> {
        if event.ready {
            self.pending.clear();
            if let Some(error) = event.error {
                self.turn_error = error;
            }
            self.status = if event.pending > 0 {
                DshWorkStatus::NeedsInput
            } else {
                event.status.unwrap_or(self.status)
            };
            return None;
        }

        match event.kind {
            "turn/start" if event.turn.is_some() => {
                self.turn = event.turn;
                self.turn_error = false;
                self.pending.clear();
                self.status = DshWorkStatus::Running;
            }
            "agent/status" => self.status = event.status.unwrap_or(self.status),
            "approval/pending" | "question/pending" => {
                let id = text(&event.data["id"])?;
                if self.pending.insert(id.to_owned()) {
                    return Some(if event.kind == "approval/pending" {
                        notice(
                            "permission",
                            "Permission",
                            format!(
                                "{} requires approval",
                                text(&event.data["toolName"]).unwrap_or("DSH")
                            ),
                        )
                    } else {
                        notice("waiting", "Waiting", "DSH needs your input")
                    });
                }
            }
            "approval/resolved" | "question/resolved" => {
                if let Some(id) = text(&event.data["id"]) {
                    self.pending.remove(id);
                }
                self.status = event.status.unwrap_or(self.status);
            }
            "turn/end" if self.turn.is_some() && self.turn == event.turn => {
                self.turn = None;
                self.pending.clear();
                self.status = DshWorkStatus::Idle;
                let reason = &event.data["reason"];
                return match reason["kind"].as_str() {
                    Some("completed") => {
                        Some(notice("completed", "Completed", "DSH session completed"))
                    }
                    Some("error") => {
                        self.turn_error = true;
                        let error = &reason["error"];
                        let body = text(&error["message"])
                            .or_else(|| text(&error["code"]))
                            .unwrap_or("DSH session encountered an error");
                        Some(notice("error", "Error", error_body(body)))
                    }
                    Some("blocked" | "max-tokens") => {
                        Some(notice("attention", "Attention", "DSH needs your attention"))
                    }
                    _ => None,
                };
            }
            _ => {}
        }
        None
    }
}

struct Observation<'a> {
    session_id: &'a str,
    kind: &'a str,
    data: &'a Value,
    generation: i64,
    sequence: i64,
    turn: Option<i64>,
    ready: bool,
    persisted: bool,
    cwd: Option<&'a str>,
    status: Option<DshWorkStatus>,
    pending: i64,
    error: Option<bool>,
}

impl<'a> Observation<'a> {
    fn parse(payload: &'a Value, run_id: &str) -> Option<Self> {
        if run_id.is_empty()
            || payload["dsh_observation"].as_f64() != Some(1.0)
            || payload["run_id"].as_str() != Some(run_id)
            || payload["foreground"].as_bool() != Some(true)
        {
            return None;
        }
        let generation = safe_integer(&payload["generation"]).filter(|&value| value > 0)?;
        let sequence = safe_integer(&payload["sequence"]).filter(|&value| value > 0)?;
        let session_id = text(&payload["session_id"]).filter(|id| valid_session_id(id))?;
        let kind = text(&payload["event_type"])?;
        let data = &payload["event_data"];
        let status = match text(&payload["agent_status"]) {
            Some("idle") => Some(DshWorkStatus::Idle),
            Some("running") => Some(DshWorkStatus::Running),
            _ => None,
        };
        Some(Self {
            session_id,
            kind,
            data,
            generation,
            sequence,
            turn: safe_integer(&data["turn"]),
            ready: kind == "ready" && payload["ready"].as_bool() == Some(true),
            persisted: payload["persisted"].as_bool() == Some(true),
            cwd: text(&payload["cwd"]),
            status,
            pending: safe_integer(&payload["pending_count"]).unwrap_or(0).max(0),
            error: payload["turn_error"].as_bool(),
        })
    }
}

pub(crate) fn js_whitespace(character: char) -> bool {
    character == '\u{feff}' || (character != '\u{85}' && character.is_whitespace())
}

pub(crate) fn valid_session_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn text(value: &Value) -> Option<&str> {
    let value = value.as_str()?;
    (!value.trim_matches(js_whitespace).is_empty() && value.encode_utf16().count() <= 4096)
        .then_some(value)
}

fn safe_integer(value: &Value) -> Option<i64> {
    let number = value.as_f64()?;
    (number.abs() <= MAX_SAFE_INTEGER && number.fract() == 0.0).then_some(number as i64)
}

fn notice(
    event_type: &'static str,
    subtitle: &'static str,
    body: impl Into<String>,
) -> DshNotification {
    DshNotification {
        event_type,
        title: "DSH",
        subtitle,
        body: body.into(),
        sound: true,
        cooldown_key: if matches!(event_type, "completed" | "error") {
            "Stop"
        } else {
            "Notification"
        },
    }
}

fn error_body(body: &str) -> String {
    let mut compact = String::new();
    let mut in_whitespace = false;
    for character in body.chars() {
        let whitespace = js_whitespace(character);
        if !whitespace || !in_whitespace {
            compact.push(if whitespace { ' ' } else { character });
        }
        in_whitespace = whitespace;
    }
    String::from_utf16_lossy(&compact.encode_utf16().take(200).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dsh_json_numbers_follow_javascript_safe_integer_limits() {
        assert_eq!(safe_integer(&json!(1.0)), Some(1));
        assert_eq!(
            safe_integer(&json!(9_007_199_254_740_991_u64)),
            Some(9_007_199_254_740_991)
        );
        assert_eq!(safe_integer(&json!(9_007_199_254_740_992_u64)), None);
        assert_eq!(safe_integer(&json!(1.5)), None);
        assert_eq!(safe_integer(&json!("1")), None);
    }

    #[test]
    fn dsh_error_body_keeps_edge_spaces_and_uses_utf16_limit() {
        assert_eq!(
            error_body("\u{feff} transport\n\tfailed "),
            " transport failed "
        );
        let source = format!("{}tail", "🎈".repeat(100));
        assert_eq!(error_body(&source), "🎈".repeat(100));
    }
}
