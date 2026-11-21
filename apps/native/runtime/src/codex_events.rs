//! Run-scoped Codex lifecycle. The owner admits hooks before applying effects.
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsStr,
    fs::{self, File},
    hash::{DefaultHasher, Hash, Hasher},
    io::{self, BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant, SystemTime},
};

use matou_native_core::pty::{ShellLaunch, get_var};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    agent_hud_projection::{
        AgentActivity, AgentState, Todo, ToolStatus, agent_tool, new_agent, target_for_tool,
        timestamp_ms, tool_arguments, update_mcp_error, update_todos,
    },
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
    invalid_transcript_field: bool,
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
            invalid_transcript_field: paths
                .iter()
                .any(|key| payload.get(*key).is_some_and(|value| !value.is_string())),
            turn,
            fingerprint: fingerprint.clone(),
            hud_tool: matches!(event, "PreToolUse" | "PostToolUse")
                .then(|| HookTool::parse(payload, fingerprint.as_deref()))
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
    next_rollout_poll: Instant,
    hud: Hud,
    pub title: Option<title::Watch>,
    turns_seen: HashSet<String>,
    turn: Option<Turn>,
    ended: bool,
    interruption: CodexInterruption,
}

impl Run {
    pub fn new(launch: &ShellLaunch) -> Self {
        let home = crate::codex_launch::config_root(|key| {
            get_var(&launch.env, key).or_else(|| get_var(&launch.inherited_env, key))
        });
        Self {
            home: home.map(|home| launch.cwd.join(home)),
            rollout: None,
            next_rollout_poll: Instant::now(),
            hud: Hud::default(),
            title: None,
            turns_seen: HashSet::new(),
            turn: None,
            ended: false,
            interruption: CodexInterruption::default(),
        }
    }

    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    pub fn hud_agents(&self, now_ms: f64) -> Vec<AgentActivity> {
        if self.ended {
            return Vec::new();
        }
        self.hud
            .agents
            .values()
            .filter(|(_, failed_at)| failed_at.is_none_or(|at| now_ms - at < 300_000.))
            .map(|(activity, _)| activity.clone())
            .collect()
    }

    pub fn start_rollout(
        &self,
        hook: &Hook,
        identity: Uuid,
        resuming: bool,
    ) -> Result<Option<(PathBuf, File)>, &'static str> {
        if hook.invalid_transcript_field {
            return Err("invalid transcript field");
        }
        hook.transcript.as_ref().map_or(Ok(None), |path| {
            self.open_transcript(path, identity, resuming)
        })
    }

    fn open_transcript(
        &self,
        path: &Path,
        identity: Uuid,
        allow_pending: bool,
    ) -> Result<Option<(PathBuf, File)>, &'static str> {
        let home = self
            .home()
            .and_then(|home| home.canonicalize().ok())
            .ok_or("unavailable Codex home")?;
        let valid_name = |path: &Path| {
            path.extension().and_then(OsStr::to_str) == Some("jsonl")
                && rollout_identity(path).is_none_or(|id| id == identity)
        };
        if !path.is_absolute()
            || path.components().any(|part| part == Component::ParentDir)
            || !valid_name(path)
        {
            return Err("invalid transcript path");
        }
        let resolved = match path.canonicalize() {
            Ok(path) => path,
            Err(error)
                if allow_pending
                    && error.kind() == io::ErrorKind::NotFound
                    && rollout_identity(path) == Some(identity) =>
            {
                // A file the CLI has yet to create is safe only if its
                // nearest existing ancestor resolves inside this Codex home.
                for ancestor in path.ancestors() {
                    match fs::symlink_metadata(ancestor) {
                        Ok(_) => {
                            if ancestor
                                .canonicalize()
                                .is_ok_and(|path| path.starts_with(&home))
                            {
                                return Ok(None);
                            }
                            break;
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(_) => break,
                    }
                }
                return Err("transcript path leaves Codex home");
            }
            Err(_) => return Err("unavailable transcript path"),
        };
        if !resolved.starts_with(home) || !valid_name(&resolved) || !resolved.is_file() {
            return Err("invalid transcript path");
        }
        let Ok(file) = File::open(&resolved) else {
            return Ok(None);
        };
        if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
            return Err("invalid transcript file");
        }
        Ok(Some((resolved, file)))
    }

    pub fn find_rollout(&self, identity: Uuid) -> Option<(PathBuf, File)> {
        let home = self.home()?.canonicalize().ok()?;
        let sessions = home.join("sessions").canonicalize().ok()?;
        if !sessions.starts_with(&home) {
            return None;
        }
        let mut directories = vec![sessions.clone()];
        while let Some(directory) = directories.pop() {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if kind.is_dir() {
                    directories.push(path);
                } else if kind.is_file()
                    && entry.file_name().to_str().is_some_and(|name| {
                        name.starts_with("rollout-")
                            && name.ends_with(".jsonl")
                            && rollout_identity(&path) == Some(identity)
                    })
                    && let Ok(Some(rollout)) = self.open_transcript(&path, identity, false)
                    && rollout.0.starts_with(&sessions)
                {
                    return Some(rollout);
                }
            }
        }
        None
    }

    pub fn confirm(&mut self, rollout: Option<(PathBuf, File)>, identity: String) {
        self.title = self
            .home
            .as_ref()
            .map(|home| title::Watch::new(home.clone(), identity));
        self.rollout = rollout.map(|(path, file)| (path, Arc::new(file)));
        self.hud = Hud::default();
    }

    pub fn admits_hook(&mut self, hook: &Hook, identity: Uuid) -> bool {
        if self.ended || hook.invalid_transcript_field {
            return false;
        }
        if !hook.has_transcript_field {
            return true;
        }
        if let Some((rollout, _)) = &self.rollout {
            return hook
                .transcript
                .as_ref()
                .and_then(|path| path.canonicalize().ok())
                .as_ref()
                == Some(rollout);
        }
        let Some(path) = &hook.transcript else {
            return true;
        };
        match self.open_transcript(path, identity, true) {
            Ok(rollout) => {
                self.rollout = rollout.map(|(path, file)| (path, Arc::new(file)));
                true
            }
            Err(_) => false,
        }
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
        let now = Instant::now();
        if self.rollout.is_none()
            && !self.ended
            && (flush || now >= self.next_rollout_poll)
            && let Some(watch) = self.title.as_ref()
            && (flush || watch.ready(now))
            && let Ok(identity) = watch.identity.parse()
        {
            self.next_rollout_poll = now + Duration::from_secs(1);
            self.rollout = self
                .find_rollout(identity)
                .map(|(path, file)| (path, Arc::new(file)));
        }
        let turn = self.turn.as_mut()?;
        let id = turn.id.as_deref()?;
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

fn rollout_identity(path: &Path) -> Option<Uuid> {
    transcript_uuid(path.file_name()?.to_str()?)?.parse().ok()
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
    input: Value,
    result: Value,
}

impl HookTool {
    fn parse(payload: &Map<String, Value>, fingerprint: Option<&str>) -> Option<Self> {
        let text = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| payload.get(*key).and_then(nonempty))
        };
        let call = payload.get("toolCall").unwrap_or(&Value::Null);
        let name = text(&["tool_name", "toolName"]).or_else(|| nonempty(&call["name"]))?;
        let id = text(&["tool_use_id", "toolUseId"])
            .or_else(|| nonempty(&call["id"]))
            .or_else(|| (name == "update_plan").then_some(fingerprint).flatten())?
            .to_owned();
        let input = ["tool_input", "toolInput"]
            .iter()
            .find_map(|key| payload.get(*key))
            .unwrap_or(&call["args"]);
        let input = tool_arguments(input);
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
                status: if payload.get("error").is_some_and(|error| !error.is_null()) {
                    "error"
                } else {
                    "completed"
                },
            },
            input,
            result: payload
                .get("tool_response")
                .or_else(|| payload.get("toolResponse"))
                .or_else(|| payload.get("toolResult"))
                .cloned()
                .unwrap_or_default(),
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
    rollout_only: BTreeMap<String, String>,
    rollout_calls: BTreeMap<String, Tool>,
    rollout_todos: Option<Vec<Todo>>,
    hook_todos: Option<(String, Vec<Todo>)>,
    agents: BTreeMap<String, (AgentActivity, Option<f64>)>,
    agent_ids: BTreeMap<String, String>,
    mcp_errors: Vec<String>,
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
            rollout_only: BTreeMap::new(),
            rollout_calls: BTreeMap::new(),
            rollout_todos: None,
            hook_todos: None,
            agents: BTreeMap::new(),
            agent_ids: BTreeMap::new(),
            mcp_errors: Vec::new(),
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

    fn observe_hook(
        &mut self,
        event: &str,
        HookTool {
            id,
            mut tool,
            input,
            result,
        }: HookTool,
    ) {
        if event == "PreToolUse" && !self.rollout_completed.contains(&id) {
            if tool.name == "update_plan" && input["plan"].is_array() {
                let mut todos = Vec::new();
                update_todos(&mut todos, &tool.name, &input);
                self.hook_todos = Some((id.clone(), todos));
            }
            self.observe_agent_call(&id, &tool.name, &input, None);
        } else if event == "PostToolUse" {
            self.observe_agent_result(&id, &result, tool.status == "error", None);
            update_mcp_error(
                &mut self.mcp_errors,
                &tool.name,
                if tool.status == "error" {
                    ToolStatus::Error
                } else {
                    ToolStatus::Completed
                },
            );
        }
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
                self.rollout_only.remove(&id);
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
            self.rollout_only.clear();
            self.rollout_calls.clear();
            self.rollout_todos = None;
            self.agents.clear();
            self.agent_ids.clear();
            self.mcp_errors.clear();
            self.dirty = true;
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
                HudUpdate::Command { id, tool } => self.complete_tool(id, tool),
                HudUpdate::Call {
                    id,
                    name,
                    input,
                    time,
                } => {
                    if name == "update_plan" && input["plan"].is_array() {
                        let mut todos = Vec::new();
                        update_todos(&mut todos, &name, &input);
                        self.rollout_todos = Some(todos);
                        if self
                            .hook_todos
                            .as_ref()
                            .is_some_and(|(call, _)| call == &id || call.starts_with("payload:"))
                        {
                            self.hook_todos = None;
                        }
                    }
                    self.observe_agent_call(&id, &name, &input, time);
                    if !self.rollout_completed.contains(&id) {
                        self.rollout_calls.insert(
                            id,
                            Tool {
                                target: target_for_tool(&name, &input),
                                name,
                                status: "running",
                            },
                        );
                    }
                    self.dirty = true;
                }
                HudUpdate::Result {
                    id,
                    result,
                    failed,
                    time,
                } => {
                    self.observe_agent_result(&id, &result, failed, time);
                    if let Some(mut tool) = self.rollout_calls.remove(&id) {
                        tool.status = if failed { "error" } else { "completed" };
                        self.complete_tool(id, tool);
                    }
                    self.dirty = true;
                }
                HudUpdate::AgentStates { statuses, time } => {
                    for (id, status) in statuses {
                        match status {
                            AgentOutcome::Started { call_id, nickname } => {
                                let key = call_id.unwrap_or_else(|| id.clone());
                                if let Some((activity, _)) = self.agents.get_mut(&key) {
                                    if let Some(nickname) = nickname {
                                        activity.label = nickname
                                    }
                                } else if let Some(nickname) = nickname {
                                    let input = json!({"nickname": nickname});
                                    self.agents.insert(
                                        key.clone(),
                                        (new_agent(&key, &input, 1, time), None),
                                    );
                                }
                                self.agent_ids.insert(id, key);
                            }
                            AgentOutcome::Finished { failed } => {
                                self.finish_agent(&id, failed, time)
                            }
                        }
                    }
                    self.dirty = true;
                }
            }
        }
        if caught_up_now {
            self.caught_up = true;
            self.dirty |= self.model.is_some()
                || self.effort.is_some()
                || self.context.is_some()
                || !self.rollout_only.is_empty()
                || self.rollout_todos.is_some();
        }
    }

    fn complete_tool(&mut self, id: String, tool: Tool) {
        if self.rollout_completed.insert(id.clone()) {
            self.running.remove(&id);
            if !self.hook_completed.contains(&id) {
                self.rollout_only.insert(id, tool.name.clone());
                update_mcp_error(
                    &mut self.mcp_errors,
                    &tool.name,
                    if tool.status == "error" {
                        ToolStatus::Error
                    } else {
                        ToolStatus::Completed
                    },
                );
            }
            self.last_is_rollout = self.running.is_empty();
            self.last_rollout_tool = Some(tool);
            self.dirty = true;
        }
    }

    fn observe_agent_call(&mut self, id: &str, name: &str, input: &Value, time: Option<f64>) {
        if agent_tool(name)
            && !self.rollout_completed.contains(id)
            && !self.hook_completed.contains(id)
            && !self.agents.contains_key(id)
        {
            self.agents.insert(
                id.to_owned(),
                (new_agent(id, input, self.agents.len() + 1, time), None),
            );
        }
    }

    fn observe_agent_result(&mut self, id: &str, result: &Value, failed: bool, time: Option<f64>) {
        if !self.agents.contains_key(id) {
            return;
        }
        if failed {
            self.finish_agent(id, true, time);
        } else if let Some(child) =
            nonempty(&result["agent_id"]).or_else(|| nonempty(&result["agentId"]))
        {
            self.agent_ids.insert(child.to_owned(), id.to_owned());
            if let Some(nickname) = nonempty(&result["nickname"])
                && let Some((agent, _)) = self.agents.get_mut(id)
            {
                agent.label = nickname.to_owned();
            }
        } else if matches!(
            result["status"].as_str(),
            Some("completed" | "shutdown" | "cancelled")
        ) {
            self.finish_agent(id, false, time);
        }
    }

    fn finish_agent(&mut self, id: &str, failed: bool, time: Option<f64>) {
        let key = self.agent_ids.get(id).map(String::as_str).unwrap_or(id);
        if failed {
            if let Some((agent, failed_at)) = self.agents.get_mut(key) {
                agent.state = AgentState::Error;
                *failed_at = time;
            }
        } else {
            self.agents.remove(key);
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
        if self.caught_up {
            for name in self.rollout_only.values().filter(|name| safe(name)) {
                *counts.entry(name.as_str()).or_default() += 1;
            }
        }
        if let Some(todos) = self
            .hook_todos
            .as_ref()
            .map(|(_, todos)| todos)
            .or_else(|| {
                self.caught_up
                    .then_some(self.rollout_todos.as_ref())
                    .flatten()
            })
        {
            value["todos"] = json!(todos);
        }
        if !self.mcp_errors.is_empty() {
            value["mcpErrors"] = json!(self.mcp_errors);
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
                .chain(
                    self.rollout_calls
                        .iter()
                        .filter(|(id, _)| {
                            self.caught_up
                                && !self.running.contains_key(*id)
                                && !self.hook_completed.contains(*id)
                        })
                        .map(|(_, tool)| tool)
                )
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

enum AgentOutcome {
    Started {
        call_id: Option<String>,
        nickname: Option<String>,
    },
    Finished {
        failed: bool,
    },
}

enum HudUpdate {
    Call {
        id: String,
        name: String,
        input: Value,
        time: Option<f64>,
    },
    Result {
        id: String,
        result: Value,
        failed: bool,
        time: Option<f64>,
    },
    AgentStates {
        statuses: Vec<(String, AgentOutcome)>,
        time: Option<f64>,
    },
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
    let time = timestamp_ms(&row["timestamp"]);
    let text = |key: &str| nonempty(&payload[key]).map(str::to_owned);
    let update = match (row["type"].as_str()?, payload["type"].as_str()) {
        ("response_item", Some("function_call" | "custom_tool_call")) => {
            let name = text("name")?;
            // Shell command completion already comes from item_completed on
            // supported CLIs, where there is a real exit code.
            if matches!(
                name.as_str(),
                "exec_command" | "local_shell" | "shell_command"
            ) {
                return None;
            }
            HudUpdate::Call {
                id: text("call_id")?,
                name,
                input: tool_arguments(payload.get("arguments").unwrap_or(&payload["input"])),
                time,
            }
        }
        ("response_item", Some("function_call_output" | "custom_tool_call_output")) => {
            let output = &payload["output"];
            let result = tool_arguments(output);
            let failed = payload["is_error"] == true
                || output["success"] == false
                || !result["error"].is_null()
                || matches!(result["status"].as_str(), Some("failed" | "error"));
            HudUpdate::Result {
                id: text("call_id")?,
                result,
                failed,
                time,
            }
        }
        ("event_msg", Some("collab_spawn_end")) => {
            let id = text("new_thread_id").or_else(|| text("receiver_thread_id"))?;
            HudUpdate::AgentStates {
                statuses: vec![(
                    id,
                    AgentOutcome::Started {
                        call_id: text("call_id"),
                        nickname: text("new_agent_nickname").or_else(|| text("nickname")),
                    },
                )],
                time,
            }
        }
        ("event_msg", Some("collab_waiting_end" | "collab_close_end" | "agent_status")) => {
            let finished = |status: &Value| {
                let label = nonempty(status)
                    .or_else(|| status.as_object()?.keys().next().map(String::as_str))?;
                match label.to_ascii_lowercase().as_str() {
                    "completed" | "shutdown" | "closed" | "cancelled" => {
                        Some(AgentOutcome::Finished { failed: false })
                    }
                    "failed" | "errored" | "error" => Some(AgentOutcome::Finished { failed: true }),
                    _ => None,
                }
            };
            let mut statuses = Vec::new();
            if let Some(entries) = payload["statuses"].as_object() {
                statuses.extend(
                    entries
                        .iter()
                        .filter_map(|(id, status)| Some((id.clone(), finished(status)?))),
                );
            }
            if let Some(id) = text("receiver_thread_id").or_else(|| text("agent_id"))
                && let Some(status) = finished(&payload["status"]).or_else(|| {
                    (payload["type"] == "collab_close_end")
                        .then_some(AgentOutcome::Finished { failed: false })
                })
            {
                statuses.push((id, status));
            }
            HudUpdate::AgentStates { statuses, time }
        }
        ("event_msg", Some("mcp_tool_call_end")) => {
            let invocation = &payload["invocation"];
            let name = format!(
                "mcp__{}__{}",
                nonempty(&invocation["server"])?,
                nonempty(&invocation["tool"])?
            );
            let result = &payload["result"];
            let failed = !result["Err"].is_null()
                || result["Ok"]["isError"] == true
                || result["is_error"] == true
                || result["isError"] == true;
            HudUpdate::Command {
                id: text("call_id")?,
                tool: Tool {
                    name,
                    target: None,
                    status: if failed { "error" } else { "completed" },
                },
            }
        }
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

#[cfg(test)]
mod hud_tests {
    use super::*;

    fn replay(hud: &mut Hud, rows: &[Value]) {
        let updates = rows
            .iter()
            .map(|row| serde_json::to_vec(row).unwrap())
            .filter_map(|line| hud_update(&line))
            .collect();
        hud.accept(HudReply {
            session: "session".into(),
            run: "run".into(),
            cursor: HudCursor::default(),
            reset: false,
            more: false,
            updates,
        });
    }

    #[test]
    fn codex_hud_uses_hook_and_rollout_todos_subagents_and_mcp_outcomes() {
        let mut hud = Hud::default();
        let hook = json!({"tool_use_id":"plan","tool_name":"update_plan","tool_input":{"plan":[
            {"step":"Read history","status":"completed"},{"step":"Recheck","status":"in_progress"}
        ]}});
        hud.observe_hook(
            "PreToolUse",
            HookTool::parse(hook.as_object().unwrap(), None).unwrap(),
        );
        assert_eq!(
            hud.project(|_| true)["todos"][1],
            json!({"content":"Recheck","status":"in_progress"})
        );

        replay(
            &mut hud,
            &[
                json!({"type":"response_item","payload":{"type":"function_call","call_id":"plan","name":"update_plan",
                "arguments":hook["tool_input"].to_string()}}),
                json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"plan","output":"Plan updated"}}),
                json!({"type":"response_item","payload":{"type":"function_call","call_id":"spawn","name":"spawn_agent",
                "arguments":"{\"agent_type\":\"explorer\"}"}}),
                json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"spawn",
                "output":"{\"agent_id\":\"child\",\"nickname\":\"Researcher\"}"}}),
                json!({"type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"mcp",
                "invocation":{"server":"search","tool":"read"},"result":{"Err":"unavailable"}}}),
            ],
        );
        assert_eq!(hud.project(|_| true)["todos"][0]["content"], "Read history");
        assert_eq!(hud.project(|_| true)["mcpErrors"], json!(["search"]));
        assert_eq!(hud.agents["spawn"].0.label, "Researcher");
        assert_eq!(hud.agents["spawn"].0.state, AgentState::Running);
        replay(
            &mut hud,
            &[
                json!({"type":"event_msg","payload":{"type":"collab_waiting_end","statuses":{"child":{"completed":"done"}}}}),
                json!({"type":"event_msg","payload":{"type":"mcp_tool_call_end","call_id":"mcp-retry",
                "invocation":{"server":"search","tool":"read"},"result":{"Ok":{"isError":false}}}}),
            ],
        );
        assert!(hud.agents.is_empty());
        assert!(hud.project(|_| true)["mcpErrors"].is_null());
    }
}
