//! Run-scoped Codex lifecycle. The owner admits hooks before applying effects.
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsStr,
    fs::{self, File},
    hash::{DefaultHasher, Hash, Hasher},
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant, SystemTime},
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
    hud_tool: Option<HookTool>,
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
            hud_tool: matches!(event, "PreToolUse" | "PostToolUse")
                .then(|| HookTool::parse(payload))
                .flatten(),
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
    rollout: Option<(PathBuf, Arc<File>)>,
    hud: Hud,
    pub title: Option<title::Watch>,
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
            hud: Hud::default(),
            title: None,
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

    pub fn confirm(&mut self, rollout: (PathBuf, File), identity: String) {
        self.title = self
            .home
            .as_ref()
            .map(|home| title::Watch::new(home.clone(), identity));
        self.rollout = Some((rollout.0, Arc::new(rollout.1)));
        self.hud = Hud::default();
    }

    pub fn admits_hook(&self, hook: &Hook) -> bool {
        !self.ended
            && self.rollout.as_ref().is_some_and(|(rollout, _)| {
                !hook.has_transcript_field
                    || hook
                        .transcript
                        .as_ref()
                        .and_then(|path| path.canonicalize().ok())
                        .as_ref()
                        == Some(rollout)
            })
    }

    pub fn apply_admitted(&mut self, event: &'static str, hook: Hook, cwd: &str) -> Option<Effect> {
        if let Some(title) = &mut self.title {
            title.refresh(event == "SessionEnd");
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
            self.hud.clear_running();
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
                self.hud.clear_running();
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
                if let Some(tool) = hook.hud_tool {
                    self.hud.observe_hook(event, tool);
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
            self.hud.clear_running();
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
        let (_, file) = self.rollout.as_ref()?;
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
        self.hud.clear_running();
        Some(Effect {
            error: Some(error),
            ..Effect::status("error")
        })
    }

    pub fn hud_projection(&self, safe: impl Fn(&str) -> bool) -> Value {
        self.hud.project(safe)
    }

    pub fn take_hud_change(&mut self) -> bool {
        std::mem::take(&mut self.hud.dirty)
    }

    pub fn hud_due(&self) -> Option<Instant> {
        (!self.ended && self.rollout.is_some() && self.hud.cursor.is_some()).then_some(self.hud.due)
    }

    pub fn read_hud(&mut self, session: String, run: String) -> Option<HudRequest> {
        let (path, file) = self.rollout.as_ref()?;
        Some(HudRequest {
            session,
            run,
            path: path.clone(),
            file: Arc::clone(file),
            cursor: self.hud.cursor.take()?,
        })
    }

    pub fn accept_hud(&mut self, reply: HudReply) {
        if !self.ended {
            self.hud.accept(reply);
        }
    }
}

fn read_error(file: &File, size: u64, turn: &str) -> Option<String> {
    let start = size.saturating_sub(ROLLOUT_TAIL_BYTES);
    let tail = RolloutRead {
        file,
        offset: start.saturating_sub(1),
    };
    let mut reader = BufReader::new(tail.take(ROLLOUT_TAIL_BYTES + 1));
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

// The same admitted handle feeds the error tail and the background HUD.
// Explicit offsets keep the two readers independent.
struct RolloutRead<'a> {
    file: &'a File,
    offset: u64,
}

impl Read for RolloutRead<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        let size = std::os::unix::fs::FileExt::read_at(self.file, buffer, self.offset)?;
        #[cfg(windows)]
        let size = std::os::windows::fs::FileExt::seek_read(self.file, buffer, self.offset)?;
        self.offset += size as u64;
        Ok(size)
    }
}

const HUD_INTERVAL: Duration = Duration::from_millis(250);
const HUD_READ_BYTES: u64 = 256 * 1024;
const HUD_MAX_LINE: usize = 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
struct Tool {
    name: String,
    target: Option<String>,
    status: &'static str,
}

impl Tool {
    fn project(&self, safe: &impl Fn(&str) -> bool) -> Option<Value> {
        if !safe(&self.name) {
            return None;
        }
        let mut tool = json!({"name": self.name, "status": self.status});
        if let Some(target) = self.target.as_deref().filter(|target| safe(target)) {
            let limit = if self.status == "running"
                && matches!(self.name.as_str(), "Bash" | "run_command")
            {
                30
            } else {
                120
            };
            let mut text = target.chars();
            let mut clipped: String = text.by_ref().take(limit).collect();
            if limit == 30 && text.next().is_some() {
                clipped = target.chars().take(27).collect::<String>() + "...";
            }
            tool["target"] = json!(clipped);
        }
        Some(tool)
    }
}

#[derive(PartialEq, Eq)]
struct HookTool {
    id: String,
    tool: Tool,
}

impl HookTool {
    fn parse(payload: &Map<String, Value>) -> Option<Self> {
        let text = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| payload.get(*key).and_then(nonempty))
        };
        let id = text(&["tool_use_id", "toolUseId"])?.to_owned();
        let call = payload.get("toolCall").unwrap_or(&Value::Null);
        let name = text(&["tool_name", "toolName"]).or_else(|| nonempty(&call["name"]))?;
        let input = ["tool_input", "toolInput"]
            .iter()
            .find_map(|key| payload.get(*key).filter(|value| value.is_object()))
            .unwrap_or(&call["args"]);
        let fields: &[_] = match name {
            "Bash" | "run_command" => &["command", "CommandLine"],
            "Glob" | "Grep" | "find_by_name" | "grep_search" => &["pattern", "Pattern", "Query"],
            "Read"
            | "Write"
            | "Edit"
            | "MultiEdit"
            | "NotebookEdit"
            | "read_file"
            | "write_file"
            | "edit_file"
            | "view_file"
            | "replace_file_content"
            | "multi_replace_file_content" => {
                &["file_path", "path", "Path", "TargetFile", "AbsolutePath"]
            }
            _ => &[],
        };
        Some(Self {
            id,
            tool: Tool {
                name: name.to_owned(),
                target: fields
                    .iter()
                    .find_map(|key| nonempty(&input[*key]))
                    .map(str::to_owned),
                status: if text(&["error"]).is_some() {
                    "error"
                } else {
                    "completed"
                },
            },
        })
    }
}

struct Hud {
    cursor: Option<HudCursor>,
    due: Instant,
    dirty: bool,
    model: Option<String>,
    effort: Option<String>,
    context: Option<(f64, f64)>,
    caught_up: bool,
    running: BTreeMap<String, Tool>,
    hook_completed: HashSet<String>,
    hook_counts: BTreeMap<String, usize>,
    rollout_completed: HashSet<String>,
    rollout_only: usize,
    last_rollout_tool: Option<Tool>,
    last_hook_tool: Option<Tool>,
    last_is_rollout: bool,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            cursor: Some(HudCursor::default()),
            due: Instant::now(),
            dirty: false,
            model: None,
            effort: None,
            context: None,
            caught_up: false,
            running: BTreeMap::new(),
            hook_completed: HashSet::new(),
            hook_counts: BTreeMap::new(),
            rollout_completed: HashSet::new(),
            rollout_only: 0,
            last_rollout_tool: None,
            last_hook_tool: None,
            last_is_rollout: false,
        }
    }
}

impl Hud {
    fn clear_running(&mut self) {
        self.dirty |= !self.running.is_empty();
        self.running.clear();
    }

    fn observe_hook(&mut self, event: &str, HookTool { id, mut tool }: HookTool) {
        if event == "PreToolUse" {
            if !self.hook_completed.contains(&id) && !self.rollout_completed.contains(&id) {
                tool.status = "running";
                self.running.insert(id, tool.clone());
                self.last_hook_tool = Some(tool);
                self.last_is_rollout = false;
                self.dirty = true;
            }
        } else if self.hook_completed.insert(id.clone()) {
            if let Some(active) = self.running.remove(&id) {
                tool.target = active.target.or(tool.target);
            }
            *self.hook_counts.entry(tool.name.clone()).or_default() += 1;
            if self.rollout_completed.contains(&id) {
                self.rollout_only -= 1;
            } else {
                self.last_hook_tool = Some(tool);
                self.last_is_rollout = false;
            }
            self.dirty = true;
        }
    }

    fn accept(&mut self, reply: HudReply) {
        self.cursor = Some(reply.cursor);
        self.due = Instant::now()
            + if reply.more {
                Duration::ZERO
            } else {
                HUD_INTERVAL
            };
        if reply.reset {
            self.dirty |= self.model.take().is_some()
                | self.effort.take().is_some()
                | self.context.take().is_some()
                | self.last_rollout_tool.take().is_some();
            self.rollout_completed.clear();
            self.rollout_only = 0;
            self.last_is_rollout = false;
            self.caught_up = false;
        }
        let caught_up_now = !self.caught_up && !reply.more;
        for update in reply.updates {
            match update {
                HudUpdate::Turn { model, effort } => {
                    if let Some(model) = model {
                        self.dirty |= self.model.as_ref() != Some(&model);
                        self.model = Some(model);
                    }
                    self.dirty |= self.effort != effort;
                    self.effort = effort;
                }
                HudUpdate::Context(context) => {
                    self.dirty |= self.context != context;
                    self.context = context;
                }
                HudUpdate::Command { id, tool } => {
                    if self.rollout_completed.insert(id.clone()) {
                        self.running.remove(&id);
                        self.rollout_only += usize::from(!self.hook_completed.contains(&id));
                        self.last_is_rollout = self.running.is_empty();
                        self.last_rollout_tool = Some(tool);
                        self.dirty = true;
                    }
                }
            }
        }
        if caught_up_now {
            self.caught_up = true;
            self.dirty |= self.model.is_some()
                || self.effort.is_some()
                || self.context.is_some()
                || self.rollout_only > 0;
        }
    }

    fn project(&self, safe: impl Fn(&str) -> bool) -> Value {
        let mut value = json!({});
        if self.caught_up {
            if let Some(model) = self.model.as_deref().filter(|value| safe(value)) {
                value["model"] = json!(model);
            }
            if let Some(effort) = self.effort.as_deref().filter(|value| safe(value)) {
                value["effort"] = json!(effort);
            }
            if let Some((percent, window)) = self.context {
                value["contextPercent"] = json!(percent);
                value["contextWindowSize"] = json!(window);
            }
        }
        let mut counts: BTreeMap<_, _> = self
            .hook_counts
            .iter()
            .filter(|(name, _)| safe(name))
            .map(|(name, count)| (name.as_str(), *count))
            .collect();
        if self.caught_up && self.rollout_only > 0 {
            *counts.entry("Bash").or_default() += self.rollout_only;
        }
        value["toolCounts"] = json!(
            counts
                .into_iter()
                .map(|(name, count)| json!({"name": name, "count": count}))
                .collect::<Vec<_>>()
        );
        value["runningTools"] = json!(
            self.running
                .values()
                .filter_map(|tool| tool.project(&safe))
                .collect::<Vec<_>>()
        );
        let last = if self.caught_up && self.last_is_rollout {
            self.last_rollout_tool
                .as_ref()
                .or(self.last_hook_tool.as_ref())
        } else {
            self.last_hook_tool.as_ref()
        };
        if let Some(tool) = last
            .filter(|tool| tool.status != "running" || !self.running.is_empty())
            .and_then(|tool| tool.project(&safe))
        {
            value["lastTool"] = tool;
        }
        value
    }
}

enum HudUpdate {
    Turn {
        model: Option<String>,
        effort: Option<String>,
    },
    Context(Option<(f64, f64)>),
    Command {
        id: String,
        tool: Tool,
    },
}

fn hud_update(line: &[u8]) -> Option<HudUpdate> {
    let row: Value = serde_json::from_slice(line).ok()?;
    let payload = &row["payload"];
    let update = match (row["type"].as_str()?, payload["type"].as_str()) {
        ("turn_context", _) => HudUpdate::Turn {
            model: nonempty(&payload["model"]).map(str::to_owned),
            effort: nonempty(&payload["effort"])
                .or_else(|| {
                    nonempty(&payload["collaboration_mode"]["settings"]["reasoning_effort"])
                })
                .map(str::to_owned),
        },
        ("compacted", _) | ("event_msg", Some("context_compacted")) => HudUpdate::Context(None),
        ("event_msg", Some("token_count")) => {
            let info = &payload["info"];
            let context = info["last_token_usage"]["total_tokens"]
                .as_f64()
                .zip(info["model_context_window"].as_f64())
                .filter(|(used, window)| *used >= 0. && *window > 0.)
                .map(|(used, window)| ((100. * (used / window)).round(), window))
                .filter(|(percent, _)| percent.is_finite());
            HudUpdate::Context(context)
        }
        ("event_msg", Some("item_completed")) => {
            let item = &payload["item"];
            let status = item["status"].as_str()?;
            if item["type"] != "CommandExecution" || !matches!(status, "completed" | "failed") {
                return None;
            }
            let command = &item["command"];
            let target = if let Some(parts) = command.as_array() {
                let command = parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ");
                (!command.is_empty()).then_some(command)
            } else {
                nonempty(command).map(str::to_owned)
            };
            HudUpdate::Command {
                id: nonempty(&item["id"])?.to_owned(),
                tool: Tool {
                    name: "Bash".into(),
                    target,
                    status: if status == "failed"
                        || item["exit_code"].as_f64().is_some_and(|code| code != 0.)
                    {
                        "error"
                    } else {
                        "completed"
                    },
                },
            }
        }
        _ => return None,
    };
    Some(update)
}

#[derive(Default)]
struct HudCursor {
    offset: u64,
    stamp: Option<(u64, Option<SystemTime>)>,
    pending: Vec<u8>,
    skipping_line: bool,
}

pub(crate) struct HudRequest {
    session: String,
    run: String,
    path: PathBuf,
    file: Arc<File>,
    cursor: HudCursor,
}

pub(crate) struct HudReply {
    pub session: String,
    pub run: String,
    cursor: HudCursor,
    reset: bool,
    more: bool,
    updates: Vec<HudUpdate>,
}

impl HudRequest {
    fn reply(self) -> HudReply {
        HudReply {
            session: self.session,
            run: self.run,
            cursor: self.cursor,
            reset: false,
            more: false,
            updates: Vec::new(),
        }
    }

    fn read(self) -> HudReply {
        let path = self.path.clone();
        let file = Arc::clone(&self.file);
        let mut reply = self.reply();
        if reply.append(&path, &file).is_err() {
            reply.reset = reply.cursor.stamp.is_some();
            reply.more = false;
            reply.cursor = HudCursor::default();
            reply.updates.clear();
        }
        reply
    }
}

impl HudReply {
    fn append(&mut self, path: &Path, file: &File) -> io::Result<()> {
        let opened = file.metadata()?;
        let on_disk = fs::symlink_metadata(path)?;
        if !on_disk.is_file() || !same_rollout(&opened, &on_disk) {
            return Err(io::ErrorKind::NotFound.into());
        }
        let stamp = (opened.len(), opened.modified().ok());
        if let Some(previous) = self.cursor.stamp {
            if stamp.0 < previous.0 || (stamp.0 == previous.0 && stamp.1 != previous.1) {
                self.cursor = HudCursor::default();
                self.reset = true;
            }
        }
        let cursor = &mut self.cursor;
        cursor.stamp = Some(stamp);
        let budget = stamp.0.saturating_sub(cursor.offset).min(HUD_READ_BYTES);
        let mut bytes = Vec::new();
        RolloutRead {
            file,
            offset: cursor.offset,
        }
        .take(budget)
        .read_to_end(&mut bytes)?;
        cursor.offset += bytes.len() as u64;
        self.more = cursor.offset < stamp.0 && !bytes.is_empty();
        for segment in bytes.split_inclusive(|byte| *byte == b'\n') {
            if !cursor.skipping_line && cursor.pending.len() + segment.len() <= HUD_MAX_LINE {
                cursor.pending.extend_from_slice(segment);
            } else {
                cursor.pending.clear();
                cursor.skipping_line = true;
            }
            if segment.ends_with(b"\n") {
                self.updates.extend(hud_update(&cursor.pending));
                cursor.pending.clear();
                cursor.skipping_line = false;
            }
        }
        // Accept a complete final record without a newline. Keep incomplete
        // UTF-8 and JSON bytes for the next append.
        if !self.more
            && !bytes.is_empty()
            && let Some(update) = hud_update(&cursor.pending)
        {
            self.updates.push(update);
            cursor.pending.clear();
        }
        Ok(())
    }
}

fn same_rollout(opened: &fs::Metadata, on_disk: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        opened.dev() == on_disk.dev() && opened.ino() == on_disk.ino()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        opened.creation_time() == on_disk.creation_time()
    }
}

#[derive(Default)]
pub(crate) struct HudReader {
    worker: Option<(mpsc::SyncSender<HudRequest>, mpsc::Receiver<HudReply>)>,
    busy: bool,
}

impl HudReader {
    pub fn idle(&self) -> bool {
        !self.busy
    }

    pub fn submit(&mut self, request: HudRequest) -> Option<HudReply> {
        if self.worker.is_none() {
            self.worker = Self::start().ok();
        }
        let Some((send, _)) = &self.worker else {
            return Some(request.reply());
        };
        match send.try_send(request) {
            Ok(()) => {
                self.busy = true;
                None
            }
            Err(error) => {
                self.worker = None;
                let request = match error {
                    mpsc::TrySendError::Full(request)
                    | mpsc::TrySendError::Disconnected(request) => request,
                };
                Some(request.reply())
            }
        }
    }

    pub fn poll(&mut self) -> Option<HudReply> {
        let (_, replies) = self.worker.as_ref()?;
        match replies.try_recv() {
            Ok(reply) => {
                self.busy = false;
                Some(reply)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.worker = None;
                self.busy = false;
                None
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }

    fn start() -> io::Result<(mpsc::SyncSender<HudRequest>, mpsc::Receiver<HudReply>)> {
        let (send, requests) = mpsc::sync_channel::<HudRequest>(1);
        let (replies, receive) = mpsc::channel();
        thread::Builder::new()
            .name("codex-hud".into())
            .spawn(move || {
                for request in requests {
                    if replies.send(request.read()).is_err() {
                        break;
                    }
                }
            })?;
        Ok((send, receive))
    }
}

pub(crate) mod title {
    //! Observe the index of an admitted Codex run without blocking the owner on disk reads.
    use std::{
        fs::{self, Metadata, OpenOptions},
        io::{self, BufRead, BufReader, Read},
        path::PathBuf,
        sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
        thread,
        time::{Duration, Instant, SystemTime},
    };

    use rusqlite::Connection;
    use serde_json::Value;

    const POLL_INTERVAL: Duration = Duration::from_millis(250);
    const WATCH_WINDOW: Duration = Duration::from_secs(30);
    const MAX_LINE: u64 = 8 * 1024 * 1024;

    pub(crate) struct Watch {
        home: PathBuf,
        pub identity: String,
        pub due: Instant,
        deadline: Instant,
        final_read: bool,
        stamp: Option<Stamp>,
    }

    impl Watch {
        pub fn new(home: PathBuf, identity: String) -> Self {
            let now = Instant::now();
            Self {
                home,
                identity,
                due: now,
                deadline: now + WATCH_WINDOW,
                final_read: false,
                stamp: None,
            }
        }

        pub fn refresh(&mut self, ended: bool) {
            let now = Instant::now();
            if ended || now >= self.deadline {
                self.due = now;
            }
            self.deadline = now + WATCH_WINDOW;
            self.final_read = ended;
        }

        pub fn ready(&self, now: Instant) -> bool {
            now >= self.due && (now < self.deadline || self.final_read)
        }

        pub fn request(&mut self, session: String, run: String, now: Instant) -> Request {
            self.due = now + POLL_INTERVAL;
            if self.final_read {
                self.deadline = now;
                self.final_read = false;
            }
            Request {
                session,
                run,
                identity: self.identity.clone(),
                home: self.home.clone(),
                previous: self.stamp,
            }
        }

        pub fn accept(&mut self, read: ReadIndex) -> Option<String> {
            match read {
                ReadIndex::Unchanged => None,
                ReadIndex::Missing => {
                    self.stamp = None;
                    None
                }
                ReadIndex::Read(stamp, title) => {
                    self.stamp = Some(stamp);
                    title
                }
            }
        }

        pub fn retry(&mut self) {
            self.stamp = None;
        }
    }

    pub(crate) struct Request {
        session: String,
        run: String,
        identity: String,
        home: PathBuf,
        previous: Option<Stamp>,
    }

    pub(crate) struct Observation {
        pub session: String,
        pub run: String,
        pub identity: String,
        pub read: ReadIndex,
    }

    #[derive(Default)]
    pub(crate) struct Reader {
        worker: Option<(SyncSender<Request>, Receiver<Observation>)>,
        busy: bool,
    }

    impl Reader {
        pub fn idle(&self) -> bool {
            !self.busy
        }

        pub fn submit(&mut self, request: Request) {
            if self.worker.is_none() {
                self.worker = start_reader().ok();
            }
            let Some((requests, _)) = &self.worker else {
                return;
            };
            if requests.try_send(request).is_ok() {
                self.busy = true;
            } else {
                self.worker = None;
            }
        }

        pub fn poll(&mut self) -> Option<Observation> {
            if !self.busy {
                return None;
            }
            match self.worker.as_ref()?.1.try_recv() {
                Ok(result) => {
                    self.busy = false;
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.busy = false;
                    None
                }
            }
        }
    }

    fn start_reader() -> io::Result<(SyncSender<Request>, Receiver<Observation>)> {
        let (requests, incoming) = mpsc::sync_channel::<Request>(1);
        let (outgoing, replies) = mpsc::channel();
        thread::Builder::new()
            .name("codex-title".into())
            .spawn(move || {
                let Ok(dates) = Connection::open_in_memory() else {
                    return;
                };
                for request in incoming {
                    let read = read_index(&request, &dates).unwrap_or(ReadIndex::Unchanged);
                    if outgoing
                        .send(Observation {
                            session: request.session,
                            run: request.run,
                            identity: request.identity,
                            read,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok((requests, replies))
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Stamp {
        size: u64,
        modified: SystemTime,
        #[cfg(unix)]
        identity: (u64, u64, i64, i64),
    }

    impl Stamp {
        fn new(meta: &Metadata) -> io::Result<Self> {
            #[cfg(unix)]
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                size: meta.len(),
                modified: meta.modified()?,
                #[cfg(unix)]
                identity: (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec()),
            })
        }
    }

    pub(crate) enum ReadIndex {
        Unchanged,
        Missing,
        Read(Stamp, Option<String>),
    }

    fn read_index(request: &Request, dates: &Connection) -> io::Result<ReadIndex> {
        let path = request.home.canonicalize()?.join("session_index.jsonl");
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) => return Ok(ReadIndex::Missing),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ReadIndex::Missing),
            Err(error) => return Err(error),
        };
        let stamp = Stamp::new(&meta)?;
        if request.previous == Some(stamp) {
            return Ok(ReadIndex::Unchanged);
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        let opened = file.metadata()?;
        if !opened.is_file() || Stamp::new(&opened)? != stamp {
            // An append or replacement raced the open. Leave the stamp unconsumed.
            return Ok(ReadIndex::Unchanged);
        }
        let mut reader = BufReader::new(file.take(stamp.size));
        let title = latest_title(&mut reader, &request.identity, dates)?;
        Ok(ReadIndex::Read(stamp, title))
    }

    fn latest_title(
        reader: &mut impl BufRead,
        identity: &str,
        dates: &Connection,
    ) -> io::Result<Option<String>> {
        let mut newest: Option<(i64, String)> = None;
        let mut line = Vec::new();
        let mut timestamp = dates
            .prepare_cached("SELECT CAST(ROUND(unixepoch(?1, 'subsec') * 1000) AS INTEGER)")
            .map_err(io::Error::other)?;
        loop {
            line.clear();
            let size = reader.take(MAX_LINE + 1).read_until(b'\n', &mut line)?;
            if size == 0 {
                break;
            }
            if size as u64 > MAX_LINE {
                if !line.ends_with(b"\n") {
                    reader.skip_until(b'\n')?;
                }
                continue;
            }
            let Ok(row) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if row["id"].as_str() != Some(identity) {
                continue;
            }
            let Some(title) = row["thread_name"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let Some(date) = row["updated_at"]
                .as_str()
                .filter(|date| date.as_bytes().get(4) == Some(&b'-'))
            else {
                continue;
            };
            if let Ok(Some(at)) = timestamp.query_row([date], |row| row.get(0))
                && newest.as_ref().is_none_or(|(previous, _)| at >= *previous)
            {
                newest = Some((at, title.to_owned()));
            }
        }
        Ok(newest.map(|(_, title)| title))
    }
}
