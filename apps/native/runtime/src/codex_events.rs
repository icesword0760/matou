//! Run-scoped Codex lifecycle. The owner admits hooks before applying effects.
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs::File,
    hash::{DefaultHasher, Hash, Hasher},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use matou_native_core::pty::ShellLaunch;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    derived::CodexInterruption,
    provider_hook::{nonempty, one_line, transcript_uuid},
};

pub(crate) const EVENTS: [(&str, u8); 7] = [
    ("SessionStart", 5),
    ("UserPromptSubmit", 5),
    ("PreToolUse", 10),
    ("PermissionRequest", 10),
    ("PostToolUse", 10),
    ("Stop", 5),
    ("SessionEnd", 3),
];

const ERROR_POLL_INTERVAL: Duration = Duration::from_millis(100);
const ROLLOUT_TAIL_BYTES: u64 = 64 * 1024;

#[derive(PartialEq, Eq)]
pub(crate) struct Hook {
    transcript: Option<PathBuf>,
    has_transcript_field: bool,
    turn: Option<String>,
    fingerprint: Option<String>,
}

impl Hook {
    pub fn parse(payload: &Map<String, Value>, event: &str) -> Self {
        let text = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| payload.get(*key).and_then(nonempty))
        };
        let paths = ["transcript_path", "transcriptPath", "agent_transcript_path"];
        let turn = text(&["turn_id", "turnId"]).map(str::to_owned);
        let fingerprint = match event {
            "PreToolUse" | "PostToolUse" | "PermissionRequest" => Some(
                text(&["tool_use_id", "toolUseId"])
                    .map(|id| format!("id:{id}"))
                    .unwrap_or_else(|| {
                        // Some approval hooks omit a call ID. Deduplicate the
                        // payload in this turn, independent of JSON key order.
                        let mut hash = DefaultHasher::new();
                        for (field, value) in payload {
                            field.hash(&mut hash);
                            value.to_string().hash(&mut hash);
                        }
                        format!("payload:{}", hash.finish())
                    }),
            ),
            _ => None,
        };
        Self {
            transcript: text(&paths).map(PathBuf::from),
            has_transcript_field: paths.iter().any(|key| payload.contains_key(*key)),
            turn,
            fingerprint,
        }
    }
}

pub(crate) struct Effect {
    pub status: &'static str,
    pub error: Option<Value>,
    pub notify: bool,
}

impl Effect {
    fn status(status: &'static str) -> Self {
        Self {
            status,
            error: None,
            notify: false,
        }
    }
}

struct Turn {
    id: Option<String>,
    seen_tools: HashSet<(&'static str, String)>,
    next_poll: Instant,
    scanned_size: Option<u64>,
}

pub(crate) struct Run {
    home: Option<PathBuf>,
    rollout: Option<(PathBuf, File)>,
    turns_seen: HashSet<String>,
    turn: Option<Turn>,
    ended: bool,
    interruption: CodexInterruption,
}

impl Run {
    pub fn new(launch: &ShellLaunch) -> Self {
        let env = |key: &str| {
            launch
                .env
                .get(OsStr::new(key))
                .or_else(|| launch.inherited_env.get(OsStr::new(key)))
                .filter(|value| !value.is_empty())
        };
        let home = env("CODEX_HOME").map(PathBuf::from).or_else(|| {
            let home_key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
            env(home_key).map(|home| Path::new(home).join(".codex"))
        });
        Self {
            home: home.map(|home| launch.cwd.join(home)),
            rollout: None,
            turns_seen: HashSet::new(),
            turn: None,
            ended: false,
            interruption: CodexInterruption::default(),
        }
    }

    pub fn start_rollout(&self, hook: &Hook, identity: Uuid) -> Option<(PathBuf, File)> {
        let home = self.home.as_ref()?.canonicalize().ok()?;
        let path = hook.transcript.as_ref()?;
        if !path.is_absolute() {
            return None;
        }
        let path = path.canonicalize().ok()?;
        if !path.starts_with(home)
            || path.extension().and_then(OsStr::to_str) != Some("jsonl")
            || transcript_uuid(path.file_name()?.to_str()?)
                .and_then(|id| Uuid::parse_str(id).ok())
                .is_some_and(|id| id != identity)
            || !path.is_file()
        {
            return None;
        }
        let file = File::open(&path).ok()?;
        file.metadata().ok()?.is_file().then_some((path, file))
    }

    pub fn confirm(&mut self, rollout: (PathBuf, File)) {
        self.rollout = Some(rollout);
    }

    pub fn apply(&mut self, event: &'static str, hook: Hook, cwd: &str) -> Option<Effect> {
        let (rollout_path, _) = self.rollout.as_ref()?;
        if self.ended
            || (hook.has_transcript_field
                && hook
                    .transcript
                    .as_ref()
                    .and_then(|path| path.canonicalize().ok())
                    .as_ref()
                    != Some(rollout_path))
        {
            return None;
        }
        if event == "UserPromptSubmit" {
            if let Some(id) = &hook.turn {
                if !self.turns_seen.insert(id.clone()) {
                    return None;
                }
            } else if self.turn.is_some() {
                return None;
            }
            self.interruption = CodexInterruption::default();
            self.turn = Some(Turn {
                id: hook.turn,
                seen_tools: HashSet::new(),
                next_poll: Instant::now(),
                scanned_size: None,
            });
            return Some(Effect::status("running"));
        }
        if event == "SessionEnd" {
            self.ended = true;
            let error = self.poll_error(cwd, true).and_then(|effect| effect.error);
            self.turn = None;
            return Some(Effect {
                error,
                ..Effect::status("exited")
            });
        }
        let turn = self.turn.as_mut()?;
        if hook.turn.is_some() && hook.turn != turn.id {
            return None;
        }
        match event {
            "Stop" => {
                if let Some(error) = self.poll_error(cwd, true) {
                    return Some(error);
                }
                self.turn = None;
                Some(Effect {
                    notify: true,
                    ..Effect::status("idle")
                })
            }
            "PreToolUse" | "PostToolUse" | "PermissionRequest" => {
                if !turn.seen_tools.insert((event, hook.fingerprint?)) {
                    return None;
                }
                Some(if event == "PermissionRequest" {
                    Effect {
                        notify: true,
                        ..Effect::status("needs-input")
                    }
                } else {
                    Effect::status("running")
                })
            }
            _ => None,
        }
    }

    pub fn output(&mut self, chunk: &str) -> Option<Effect> {
        if self.turn.is_some() && self.interruption.ingest(chunk) {
            self.turn = None;
            return Some(Effect::status("interrupted"));
        }
        None
    }

    pub fn poll_error(&mut self, cwd: &str, flush: bool) -> Option<Effect> {
        let turn = self.turn.as_mut()?;
        let id = turn.id.as_deref()?;
        let now = Instant::now();
        if !flush && now < turn.next_poll {
            return None;
        }
        turn.next_poll = now + ERROR_POLL_INTERVAL;
        let (_, file) = self.rollout.as_mut()?;
        let size = file.metadata().ok()?.len();
        if !flush && turn.scanned_size == Some(size) {
            return None;
        }
        turn.scanned_size = Some(size);
        let message = read_error(file, size, id)?;
        let project = Path::new(cwd).file_name().and_then(OsStr::to_str);
        let subtitle = project.map_or_else(|| "Error".into(), |name| format!("Error in {name}"));
        let body: Vec<_> = one_line(&message).encode_utf16().take(200).collect();
        let error = json!({
            "eventType": "error",
            "title": "Codex",
            "subtitle": subtitle,
            "body": String::from_utf16_lossy(&body),
            "sound": true,
            "cooldownKey": "Stop",
            "replacementKey": format!("codex-turn-error:{id}"),
        });
        // A terminal turn is consumed. A duplicate prompt, Stop or error
        // cannot reopen it, and the next prompt creates a fresh turn.
        self.turn = None;
        Some(Effect {
            error: Some(error),
            ..Effect::status("error")
        })
    }
}

fn read_error(file: &mut File, size: u64, turn: &str) -> Option<String> {
    let start = size.saturating_sub(ROLLOUT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start.saturating_sub(1))).ok()?;
    let mut reader = BufReader::new(file.take(ROLLOUT_TAIL_BYTES + 1));
    let mut line = Vec::new();
    if start != 0 {
        // Begin at a record boundary, including when the tail starts on one.
        reader.read_until(b'\n', &mut line).ok()?;
        line.clear();
    }
    while reader.read_until(b'\n', &mut line).ok()? != 0 {
        if let Ok(row) = serde_json::from_slice::<Value>(&line) {
            let payload = &row["payload"];
            if row["type"] == "event_msg"
                && payload["type"] == "task_complete"
                && payload["turn_id"] == turn
                && let Some(message) = nonempty(&payload["error"]["message"])
            {
                return Some(message.to_owned());
            }
        }
        line.clear();
    }
    None
}
