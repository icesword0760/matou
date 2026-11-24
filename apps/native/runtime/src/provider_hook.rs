//! Private HTTP ingress for a provider run. SQLite remains owned by the host thread.
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use matou_native_core::pty::{LaunchProfile, ShellLaunch};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::codex_events;
use crate::context_guard::interceptor::HookInterceptor;

pub(crate) const IDENTITY_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const HOOK_QUEUE_CAPACITY: usize = 128;
const RETIRE_GRACE: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_millis(1500);
const SHUTDOWN_DRAIN: Duration = Duration::from_millis(100);
const MAX_HOOK_BYTES: usize = 1024 * 1024;
const MAX_HEADERS: usize = 16 * 1024;
const MAX_CONNECTIONS: usize = 8;
const EVENTS: [(&str, u8); 9] = [
    ("SessionStart", 5),
    ("SessionEnd", 3),
    ("UserPromptSubmit", 5),
    ("PreToolUse", 10),
    ("PostToolUse", 10),
    ("PostToolUseFailure", 5),
    ("PermissionRequest", 10),
    ("Stop", 5),
    ("Notification", 5),
];

#[derive(Debug)]
pub(crate) struct HookSetup;

impl std::fmt::Display for HookSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("prepare provider hook")
    }
}

#[derive(Clone)]
pub(crate) struct Scope {
    pub session_id: String,
    pub run_id: String,
    pub generation: String,
    pub profile: LaunchProfile,
    pub expected_id: Option<String>,
    pub fork_source_id: Option<String>,
}

#[derive(PartialEq, Eq)]
pub(crate) struct Observation {
    pub event: &'static str,
    pub identity: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub cwd: Option<String>,
    pub notification: Option<(Notification, Instant)>,
    pub codex: Option<codex_events::Hook>,
    pub hud_payload: Option<Value>,
    pub statusline: bool,
}

impl Observation {
    #[cfg(test)]
    pub(super) fn parse(payload: &Map<String, Value>) -> Self {
        Self::parse_for(payload, LaunchProfile::ClaudeCode)
    }

    fn parse_for(payload: &Map<String, Value>, profile: LaunchProfile) -> Self {
        let events: &[_] = if profile == LaunchProfile::Codex {
            &codex_events::EVENTS
        } else {
            &EVENTS
        };
        let title = if profile == LaunchProfile::Codex {
            "Codex"
        } else {
            "Claude Code"
        };
        let text = |names: &[&str]| {
            names
                .iter()
                .find_map(|&name| payload.get(name).and_then(nonempty))
        };
        let event = text(&["hook_event_name", "hookEventName"])
            .and_then(|name| events.iter().find(|(event, _)| *event == name))
            .map_or("unknown", |&(event, _)| event);
        let identity = [
            "session_id",
            "sessionId",
            "conversationId",
            "conversation_id",
        ]
        .iter()
        .find_map(|name| {
            let value = payload.get(*name)?;
            nonempty(value)?;
            value.as_str()
        })
        .or_else(|| {
            let path = text(&["transcript_path", "transcriptPath", "agent_transcript_path"])?;
            transcript_uuid(path)
        });
        Self {
            event,
            identity: identity.map(str::to_owned),
            model: text(&["model"])
                .or_else(|| payload.get("model")?.get("id").and_then(nonempty))
                .map(str::to_owned),
            permission_mode: text(&["permission_mode", "permissionMode"]).map(str::to_owned),
            cwd: text(&["cwd"]).map(str::to_owned),
            notification: Notification::parse(event, payload, title)
                .map(|event| (event, Instant::now())),
            codex: (profile == LaunchProfile::Codex)
                .then(|| codex_events::Hook::parse(payload, event)),
            hud_payload: None,
            statusline: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotificationKind {
    Completed,
    Permission,
    Waiting,
    Attention,
}

#[derive(PartialEq, Eq)]
pub(crate) struct Notification {
    pub kind: NotificationKind,
    title: &'static str,
    subtitle: String,
    body: String,
    cooldown_key: &'static str,
}

impl Notification {
    fn parse(event: &str, payload: &Map<String, Value>, title: &'static str) -> Option<Self> {
        let text = |key: &str| payload.get(key).and_then(nonempty);
        let nested = |key: &str, field: &str| payload.get(key)?.get(field).and_then(nonempty);
        let (kind, subtitle, body, limit, cooldown_key) = match event {
            "Stop" => {
                let project = text("cwd")
                    .and_then(|cwd| cwd.rsplit(['/', '\\']).find(|part| !part.is_empty()));
                let subtitle = project.map_or_else(
                    || "Completed".into(),
                    |project| format!("Completed in {project}"),
                );
                let body = text("last_assistant_message")
                    .or_else(|| text("lastAssistantMessage"))
                    .or_else(|| text("result"))
                    .map_or_else(
                        || {
                            let provider = if title == "Codex" { title } else { "Claude" };
                            project.map_or_else(
                                || format!("{provider} session completed"),
                                |project| format!("{provider} session completed in {project}"),
                            )
                        },
                        str::to_owned,
                    );
                (NotificationKind::Completed, subtitle, body, 200, "Stop")
            }
            "PermissionRequest" => {
                let tool = text("tool_name")
                    .or_else(|| text("toolName"))
                    .unwrap_or("operation");
                let detail = [
                    text("message"),
                    text("description"),
                    nested("tool_input", "command"),
                    nested("tool_input", "file_path"),
                    nested("toolInput", "command"),
                    nested("toolInput", "filePath"),
                ]
                .into_iter()
                .flatten()
                .next();
                let body = detail.map_or_else(
                    || format!("{tool} requires approval"),
                    |detail| format!("{tool}: {detail}"),
                );
                (
                    NotificationKind::Permission,
                    "Permission".into(),
                    body,
                    180,
                    "Notification",
                )
            }
            "Notification" => {
                let raw = [
                    text("message"),
                    text("body"),
                    text("text"),
                    text("prompt"),
                    text("error"),
                    text("description"),
                    nested("notification", "message"),
                    nested("data", "message"),
                ]
                .into_iter()
                .flatten()
                .next();
                let kind = NotificationKind::classify(raw.unwrap_or_default());
                (
                    kind,
                    kind.labels().1.into(),
                    raw.unwrap_or("Claude needs your attention").into(),
                    180,
                    "Notification",
                )
            }
            _ => return None,
        };
        Some(Self {
            kind,
            title,
            subtitle,
            body: compact(&body, limit),
            cooldown_key,
        })
    }

    pub fn payload(&self) -> Value {
        json!({
            "eventType": self.kind.labels().0,
            "title": self.title,
            "subtitle": self.subtitle,
            "body": self.body,
            "sound": true,
            "cooldownKey": self.cooldown_key,
        })
    }
}

impl NotificationKind {
    fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Completed => ("completed", "Completed"),
            Self::Permission => ("permission", "Permission"),
            Self::Waiting => ("waiting", "Waiting"),
            Self::Attention => ("attention", "Attention"),
        }
    }

    fn classify(raw: &str) -> Self {
        let lower = raw.to_lowercase();
        if ["permission", "approve", "approval"]
            .iter()
            .any(|word| lower.contains(word))
        {
            Self::Permission
        } else if ["complet", "finish", "done", "success"]
            .iter()
            .any(|word| lower.contains(word))
        {
            Self::Completed
        } else if ["idle", "wait", "input"]
            .iter()
            .any(|word| lower.contains(word))
        {
            Self::Waiting
        } else {
            Self::Attention
        }
    }

    pub fn work_status(self, current: &str) -> &'static str {
        match (self, current) {
            (_, "error") => "error",
            (Self::Completed, _) => "idle",
            _ => "needs-input",
        }
    }
}

pub(crate) fn nonempty(value: &Value) -> Option<&str> {
    let value = value.as_str()?.trim_matches(js_whitespace);
    (!value.is_empty()).then_some(value)
}

fn js_whitespace(c: char) -> bool {
    c == '\u{feff}' || (c != '\u{85}' && c.is_whitespace())
}

pub(crate) fn one_line(value: &str) -> String {
    value
        .split(js_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact(value: &str, limit: usize) -> String {
    let line = one_line(value);
    let mut units: Vec<_> = line.encode_utf16().take(limit + 1).collect();
    if units.len() <= limit {
        return line;
    }
    units.truncate(limit - 1);
    // A JavaScript slice can leave half of a surrogate pair. JSON text in Rust
    // must be valid UTF-8, so render the surviving half as a replacement character.
    let mut shortened = String::from_utf16_lossy(&units);
    shortened.push('…');
    shortened
}

pub(crate) fn transcript_uuid(path: &str) -> Option<&str> {
    path.as_bytes()
        .windows(6)
        .enumerate()
        .filter(|(_, ext)| ext.eq_ignore_ascii_case(b".jsonl"))
        .find_map(|(end, _)| {
            let id = path.get(end.checked_sub(36)?..end)?;
            Uuid::parse_str(id).ok().map(|_| id)
        })
}

pub(crate) enum HookMessage {
    Observed(Scope, Observation, Instant),
    DshObserved(Scope, Value, Instant),
    AntigravityObserved(Scope, crate::antigravity_events::Source, Value, Instant),
    Retired(Scope),
}

type Registry = Arc<Mutex<HashMap<String, Record>>>;

struct Record {
    scope: Scope,
    data_root: PathBuf,
    _files: HookFiles,
    retired: Option<Instant>,
    reported_retirement: bool,
}

struct HookFiles {
    settings: PathBuf,
    script: PathBuf,
    temporary: PathBuf,
}

impl Drop for HookFiles {
    fn drop(&mut self) {
        for path in [&self.temporary, &self.settings, &self.script] {
            let _ = fs::remove_file(path);
        }
    }
}

pub(crate) struct Registration {
    token: String,
    registry: Registry,
    pub settings_path: PathBuf,
    script_path: PathBuf,
    url: String,
    dsh_assets: Option<tempfile::TempDir>,
}

impl Registration {
    pub fn attach_dsh(&mut self, dump: &str, run_id: &str) -> Result<PathBuf> {
        let assets = tempfile::Builder::new()
            .prefix("dsh-")
            .tempdir_in(self.settings_path.parent().context("DSH hook directory")?)?;
        let directory = assets.path().join("dsh");
        fs::create_dir(&directory)?;
        for (name, source) in [
            (
                "bridge.mjs",
                include_str!("../../../runtime/control-assets/providers/dsh/bridge.mjs"),
            ),
            (
                "hud.mjs",
                include_str!("../../../runtime/control-assets/providers/dsh/hud.mjs"),
            ),
            (
                "instructions.mjs",
                include_str!("../../../runtime/control-assets/providers/dsh/instructions.mjs"),
            ),
        ] {
            write_private(&directory.join(name), source.as_bytes(), 0o600)?;
        }
        write_private(
            &assets.path().join("dsh-system-message.md"),
            include_bytes!("../../../runtime/control-assets/providers/dsh-system-message.md"),
            0o600,
        )?;
        let bridge = directory.join("bridge.mjs");
        let patch = crate::dsh_launch::observation_patch(
            dump,
            bridge.to_str().context("DSH observer path is not UTF-8")?,
            &self.url,
            run_id,
        )?;
        let path = assets.path().join("observation.yml");
        write_private(&path, patch.as_bytes(), 0o600)?;
        self.dsh_assets = Some(assets);
        Ok(path)
    }

    pub fn antigravity_environment(
        &self,
    ) -> std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> {
        [
            ("MATOU_PROVIDER_HOOK_URL".into(), self.url.clone().into()),
            (
                "MATOU_PROVIDER_HOOK_TOKEN".into(),
                self.token.clone().into(),
            ),
        ]
        .into()
    }

    pub fn attach_codex(&self, launch: &mut ShellLaunch) -> Result<()> {
        let command = json!(callback_command(&self.url, &self.script_path)?);
        let status = if launch
            .env
            .get(std::ffi::OsStr::new("MATOU_LOCALE"))
            .is_some_and(|locale| locale == "en")
        {
            "Confirming the session"
        } else {
            "正在确认会话"
        };
        let status = json!(status);
        let hooks = codex_events::EVENTS.map(|(event, timeout)| {
            format!(r#"{event} = [{{ hooks = [{{ type = "command", command = {command}, timeout = {timeout}, statusMessage = {status} }}] }}]"#)
        });
        launch.argv.extend([
            "-c".into(),
            format!("hooks={{ {} }}", hooks.join(", ")),
            "--dangerously-bypass-hook-trust".into(),
        ]);
        Ok(())
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(record) = self.registry.lock().unwrap().get_mut(&self.token) {
            record
                .retired
                .get_or_insert_with(|| Instant::now() + RETIRE_GRACE);
        }
    }
}

pub(crate) struct ProviderHookServer {
    port: u16,
    registry: Registry,
    interceptor: Arc<HookInterceptor>,
    messages: Receiver<HookMessage>,
    stop: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
}

impl ProviderHookServer {
    pub fn start() -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .context("bind provider hook to local loopback")?;
        listener
            .set_nonblocking(true)
            .context("configure provider hook listener")?;
        let port = listener.local_addr()?.port();
        let registry = Registry::default();
        let interceptor = Arc::new(HookInterceptor::new());
        let stop = Arc::new(AtomicBool::new(false));
        let (send, messages) = mpsc::sync_channel(HOOK_QUEUE_CAPACITY);
        let handle = {
            let registry = Arc::clone(&registry);
            let interceptor = Arc::clone(&interceptor);
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("provider-hook".into())
                .spawn(move || listen(listener, &registry, &interceptor, &send, &stop))
                .context("start provider hook listener")?
        };
        Ok(Self {
            port,
            registry,
            interceptor,
            messages,
            stop,
            listener: Some(handle),
        })
    }

    pub fn saved_noise_bytes(&self, session_id: &str) -> u64 {
        self.interceptor.saved_noise_bytes(session_id)
    }

    pub fn register(&self, data_root: &Path, scope: Scope) -> Result<Registration> {
        let token = new_token();
        let url = format!("http://127.0.0.1:{}/hooks/{token}", self.port);
        let directory = data_root.join("provider-hooks");
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .context("create provider hook directory")?;
        ensure!(
            fs::symlink_metadata(&directory)?.is_dir(),
            "provider hook directory must be a real directory"
        );
        set_mode(&directory, 0o700).context("make provider hook directory private")?;
        let files = HookFiles {
            settings: directory.join(format!("{token}.settings.json")),
            script: directory.join(format!("{token}.statusline.sh")),
            temporary: directory.join(format!("{token}.settings.json.tmp")),
        };
        if scope.profile == LaunchProfile::Antigravity {
            let settings = json!({"statusLine": {
                "type": "command", "command": callback_command(&format!("{url}?kind=statusline"), &files.script)?,
                "stack_with_default": true,
            }});
            write_private(
                &files.temporary,
                &serde_json::to_vec_pretty(&settings)?,
                0o600,
            )?;
            fs::rename(&files.temporary, &files.settings)?;
        } else if !matches!(scope.profile, LaunchProfile::Dsh | LaunchProfile::Codex) {
            let settings = settings(&url, &files.script)?;
            write_private(
                &files.temporary,
                &serde_json::to_vec_pretty(&settings)?,
                0o600,
            )
            .context("write provider settings")?;
            fs::rename(&files.temporary, &files.settings).context("publish provider settings")?;
            File::open(&files.settings).context("read provider settings")?;
        }
        #[cfg(not(windows))]
        {
            let script = match scope.profile {
                LaunchProfile::Dsh => String::new(),
                LaunchProfile::Antigravity => format!(
                    "#!/bin/sh\npayload=$(cat)\n(printf '%s' \"$payload\" | /usr/bin/curl --noproxy '*' --silent --max-time 2 --request POST --header 'content-type: application/json' --data-binary @- '{url}?kind=statusline') </dev/null >/dev/null 2>&1 &\n"
                ),
                LaunchProfile::Codex => format!(
                    "#!/bin/sh\n/usr/bin/curl --noproxy '*' --silent --show-error --max-time 2 --request POST --header 'content-type: application/json' --data-binary @- '{url}' >/dev/null 2>&1 || true\n"
                ),
                _ => status_script(&url),
            };
            write_private(&files.script, script.as_bytes(), 0o700)
                .context("write provider hook script")?;
        }
        let settings_path = files.settings.clone();
        let script_path = files.script.clone();
        self.registry.lock().unwrap().insert(
            token.clone(),
            Record {
                scope,
                data_root: data_root.to_path_buf(),
                _files: files,
                retired: None,
                reported_retirement: false,
            },
        );
        Ok(Registration {
            token,
            registry: Arc::clone(&self.registry),
            settings_path,
            script_path,
            url,
            dsh_assets: None,
        })
    }

    pub fn messages(&self) -> impl Iterator<Item = HookMessage> + '_ {
        self.messages.try_iter()
    }

    pub(crate) fn finish_requests(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

impl Drop for ProviderHookServer {
    fn drop(&mut self) {
        self.finish_requests();
        self.registry.lock().unwrap().clear();
    }
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

fn write_private(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    set_mode(path, mode)
}

fn callback_command(url: &str, script: &Path) -> Result<String> {
    callback_command_for_platform(url, script, cfg!(windows))
}

fn callback_command_for_platform(url: &str, script: &Path, windows: bool) -> Result<String> {
    if windows {
        return Ok(windows_callback_command(url));
    }
    let script = script
        .to_str()
        .context("provider statusline path is not UTF-8")?;
    let command = if script
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
    {
        script.to_owned()
    } else {
        format!("'{}'", script.replace('\'', "'\\''"))
    };
    Ok(command)
}

fn windows_callback_command(url: &str) -> String {
    // The URL is generated from the loopback port, token and optional callback kind.
    // Double quotes work in both cmd and PowerShell. curl reads the original stdin.
    format!(
        r#"curl.exe --disable --silent --noproxy "*" --max-time 2 --output NUL --request POST --header "content-type: application/json" --data-binary "@-" "{url}""#
    )
}

pub(crate) fn bundled_claude_plugin(
    executable: Option<&Path>,
    control_asset_root: Option<&Path>,
) -> Option<PathBuf> {
    let valid_plugin = |root: &Path| {
        let plugin = root.join("providers/claude-plugin");
        plugin
            .join(".claude-plugin/plugin.json")
            .is_file()
            .then_some(plugin)
    };
    if let Some(plugin) = control_asset_root
        .filter(|root| !root.as_os_str().is_empty())
        .and_then(valid_plugin)
    {
        return Some(plugin);
    }
    let executable = executable?;
    let executable_dir = executable.parent()?;
    let resources = if executable
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        executable_dir.join("resources")
    } else {
        let contents = executable_dir.parent()?;
        if executable_dir.file_name()? != "MacOS" || contents.file_name()? != "Contents" {
            return None;
        }
        contents.join("Resources")
    };
    valid_plugin(&resources.join("control-assets"))
}

pub(crate) fn claude_config_counts(cwd: &Path, config: &Path) -> Value {
    use std::collections::BTreeSet;
    let project = cwd.join(".claude");
    let project_instruction = cwd.join("CLAUDE.md");
    let instruction_files = [
        config.join("CLAUDE.md"),
        project_instruction.clone(),
        cwd.join("CLAUDE.local.md"),
        project.join("CLAUDE.md"),
        project.join("CLAUDE.local.md"),
    ]
    .iter()
    .filter(|path| path.is_file())
    .count();
    fn rules(path: &Path) -> usize {
        let Ok(entries) = fs::read_dir(path) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| {
                let Ok(kind) = entry.file_type() else {
                    return 0;
                };
                if kind.is_dir() {
                    rules(&entry.path())
                } else {
                    usize::from(
                        kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "md"),
                    )
                }
            })
            .sum()
    }
    let mut hooks = BTreeSet::new();
    let read = |path: PathBuf| -> Value {
        fs::metadata(&path)
            .ok()
            .filter(|metadata| metadata.is_file() && metadata.len() <= 1024 * 1024)
            .and_then(|_| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    };
    let mut user_mcp = BTreeSet::new();
    let mut project_mcp = BTreeSet::new();
    let mut add_config =
        |mcp: &mut BTreeSet<String>, settings: Value, disabled: &str, include_hooks: bool| {
            if let Some(servers) = settings["mcpServers"].as_object() {
                mcp.extend(servers.keys().cloned());
            }
            for name in settings[disabled]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                mcp.remove(name);
            }
            if include_hooks && let Some(names) = settings["hooks"].as_object() {
                hooks.extend(names.keys().cloned());
            }
        };
    for name in ["settings.json", "settings.local.json"] {
        add_config(&mut user_mcp, read(config.join(name)), "", true);
    }
    if let Some(parent) = config.parent() {
        add_config(
            &mut user_mcp,
            read(parent.join(".claude.json")),
            "disabledMcpServers",
            false,
        );
    }
    add_config(&mut project_mcp, read(cwd.join(".mcp.json")), "", false);
    for name in ["settings.json", "settings.local.json"] {
        add_config(
            &mut project_mcp,
            read(project.join(name)),
            "disabledMcpjsonServers",
            true,
        );
    }
    json!({
        "instructionFiles": instruction_files,
        "claudeMd": instruction_files,
        "rules": rules(&config.join("rules")) + rules(&project.join("rules")),
        "mcps": user_mcp.union(&project_mcp).count(),
        "projectInstructionFileExists": project_instruction.is_file(),
        "mcpServers": user_mcp.len() + project_mcp.len(),
        "mcpServerNames": user_mcp.into_iter().chain(project_mcp).collect::<Vec<_>>(),
        "hooks": hooks.len(),
        "hookNames": hooks,
    })
}

fn settings(url: &str, script: &Path) -> Result<Value> {
    let command = callback_command(&format!("{url}?kind=statusline"), script)?;
    let mut hooks: Map<String, Value> = EVENTS
        .into_iter()
        .map(|(event, timeout)| {
            (
                event.to_owned(),
                json!([{ "hooks": [{ "type": "http", "url": url, "timeout": timeout }] }]),
            )
        })
        .collect();
    // Claude Code dispatches SessionStart to command hooks only.
    let session_start = if cfg!(windows) {
        windows_callback_command(url)
    } else {
        format!(
            "/usr/bin/curl --disable --silent --noproxy '*' --max-time 2 --output /dev/null --header 'content-type: application/json' --data-binary @- '{url}'"
        )
    };
    hooks.insert(
        "SessionStart".into(),
        json!([{
            "hooks": [{"type": "command", "command": session_start, "timeout": 5}]
        }]),
    );
    Ok(json!({
        "hooks": hooks,
        "statusLine": { "type": "command", "command": command, "padding": 0 },
    }))
}

#[cfg(not(windows))]
fn status_script(url: &str) -> String {
    format!(
        "#!/bin/sh\n/usr/bin/curl --silent --show-error --max-time 2 --request POST --header 'content-type: application/json' --data-binary @- '{url}?kind=statusline' >/dev/null 2>&1 || true\n"
    )
}

fn new_token() -> String {
    // UUID v4 uses the platform CSPRNG. Copy only bytes without version or
    // variant bits, giving the token 32 independent random bytes.
    let mut random = [0; 32];
    for chunk in random.chunks_mut(14) {
        let uuid = Uuid::new_v4();
        let bytes = uuid.as_bytes();
        for (output, input) in chunk
            .iter_mut()
            .zip(bytes[..6].iter().chain(&bytes[7..8]).chain(&bytes[9..]))
        {
            *output = *input;
        }
    }
    const BASE64URL: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut token = String::with_capacity(43);
    for chunk in random.chunks(3) {
        let a = chunk[0];
        let b = chunk[1];
        token.push(BASE64URL[(a >> 2) as usize] as char);
        token.push(BASE64URL[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        token.push(
            BASE64URL[(((b & 15) << 2) | (chunk.get(2).copied().unwrap_or(0) >> 6)) as usize]
                as char,
        );
        if let Some(c) = chunk.get(2) {
            token.push(BASE64URL[(c & 63) as usize] as char);
        }
    }
    token
}

fn listen(
    listener: TcpListener,
    registry: &Registry,
    interceptor: &Arc<HookInterceptor>,
    send: &SyncSender<HookMessage>,
    stop: &AtomicBool,
) {
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    let mut drain_deadline = None;
    loop {
        let now = Instant::now();
        if stop.load(Ordering::Relaxed) {
            let deadline = drain_deadline.get_or_insert(now + SHUTDOWN_DRAIN);
            if now >= *deadline {
                break;
            }
        }
        registry
            .lock()
            .unwrap()
            .retain(|_, record| record.retired.is_none_or(|deadline| now < deadline));
        let mut index = 0;
        while index < workers.len() {
            if workers[index].is_finished() {
                let _ = workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
        if workers.len() < MAX_CONNECTIONS {
            match listener.accept() {
                Ok((socket, _)) => {
                    let registry = Arc::clone(registry);
                    let interceptor = Arc::clone(interceptor);
                    let send = send.clone();
                    if let Ok(worker) = thread::Builder::new()
                        .name("provider-hook-request".into())
                        .spawn(move || {
                            handle_request(socket, &registry, &interceptor, &send, drain_deadline)
                        })
                    {
                        workers.push(worker);
                    }
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                // A nonblocking accept has drained the backlog. Already
                // accepted requests can still finish on their own sockets.
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && drain_deadline.is_some() =>
                {
                    break;
                }
                Err(_) => {}
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    for worker in workers {
        let _ = worker.join();
    }
}

const HOOK_DECISION_BUDGET: Duration = Duration::from_millis(150);

fn evaluate_hook_fail_open(
    interceptor: &Arc<HookInterceptor>,
    data_root: &Path,
    profile: LaunchProfile,
    session_id: &str,
    payload: &Value,
) -> String {
    let (tx, rx) = mpsc::sync_channel(1);
    let interceptor = Arc::clone(interceptor);
    let data_root = data_root.to_path_buf();
    let session_id = session_id.to_owned();
    let payload = payload.clone();
    let spawned = thread::Builder::new()
        .name("context-guard-eval".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                interceptor
                    .evaluate_hook(&data_root, profile, &session_id, &payload)
                    .response_json
            }))
            .unwrap_or_else(|_| "{}".to_owned());
            let _ = tx.try_send(result);
        });
    if spawned.is_err() {
        return "{}".to_owned();
    }
    rx.recv_timeout(HOOK_DECISION_BUDGET)
        .unwrap_or_else(|_| "{}".to_owned())
}

fn handle_request(
    mut socket: TcpStream,
    registry: &Registry,
    interceptor: &Arc<HookInterceptor>,
    send: &SyncSender<HookMessage>,
    drain_deadline: Option<Instant>,
) {
    // BSD can inherit non-blocking mode from the listening socket.
    if socket.set_nonblocking(false).is_err() {
        return;
    }
    let deadline = drain_deadline.unwrap_or_else(|| Instant::now() + REQUEST_TIMEOUT);
    let result = read_request(&mut socket, registry, interceptor, send, deadline);
    let (status, body) = match result {
        Ok(decision_json) => ("200 OK", decision_json),
        Err(status) => (status, String::new()),
    };
    let _ = socket.set_write_timeout(Some(Duration::from_millis(100)));
    let _ = write!(
        socket,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

type HttpResult<T> = std::result::Result<T, &'static str>;
const BAD_REQUEST: &str = "400 Bad Request";
const TOO_LARGE: &str = "413 Payload Too Large";
const NOT_FOUND: &str = "404 Not Found";

fn read_request(
    socket: &mut TcpStream,
    registry: &Registry,
    interceptor: &Arc<HookInterceptor>,
    send: &SyncSender<HookMessage>,
    deadline: Instant,
) -> HttpResult<String> {
    let mut reader = BufReader::new(DeadlineRead { socket, deadline });
    let mut header_budget = MAX_HEADERS;
    let first = read_line(&mut reader, &mut header_budget)?;
    let mut fields = first.split_ascii_whitespace();
    let method = fields.next().ok_or(BAD_REQUEST)?;
    if method != "POST" {
        return Err("405 Method Not Allowed");
    }
    let path = fields
        .next()
        .and_then(|path| path.strip_prefix("/hooks/"))
        .ok_or(NOT_FOUND)?;
    let (token, kind) = path.split_once('?').unwrap_or((path, ""));
    let statusline = kind == "kind=statusline";
    let pre_tool_use = kind == "kind=pre-tool-use";
    if !kind.is_empty() && !statusline && !pre_tool_use {
        return Err(NOT_FOUND);
    }
    if !matches!(fields.next(), Some("HTTP/1.0" | "HTTP/1.1")) || fields.next().is_some() {
        return Err(BAD_REQUEST);
    }
    if !registry.lock().unwrap().contains_key(token) {
        return Err(NOT_FOUND);
    }

    let mut length = None;
    let mut chunked = false;
    loop {
        let header = read_line(&mut reader, &mut header_budget)?;
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').ok_or(BAD_REQUEST)?;
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(BAD_REQUEST);
            }
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(BAD_REQUEST);
            }
            length = Some(value.parse::<usize>().map_err(|_| TOO_LARGE)?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if chunked || !value.trim().eq_ignore_ascii_case("chunked") {
                return Err(BAD_REQUEST);
            }
            chunked = true;
        }
    }
    let mut body = Vec::new();
    if chunked {
        if length.is_some() {
            return Err(BAD_REQUEST);
        }
        loop {
            let line = read_line(&mut reader, &mut header_budget)?;
            let length = usize::from_str_radix(line.split(';').next().unwrap_or(""), 16)
                .map_err(|_| BAD_REQUEST)?;
            if length == 0 {
                while !read_line(&mut reader, &mut header_budget)?.is_empty() {}
                break;
            }
            append_body(&mut reader, &mut body, length)?;
            if !read_line(&mut reader, &mut 2)?.is_empty() {
                return Err(BAD_REQUEST);
            }
        }
    } else {
        append_body(&mut reader, &mut body, length.ok_or(BAD_REQUEST)?)?;
    }
    let payload: Map<String, Value> = serde_json::from_slice(&body).map_err(|_| BAD_REQUEST)?;
    let mut registry = registry.lock().unwrap();
    let record = registry.get_mut(token).ok_or(NOT_FOUND)?;
    if record.retired.is_some() {
        if !record.reported_retirement
            && send
                .try_send(HookMessage::Retired(record.scope.clone()))
                .is_ok()
        {
            record.reported_retirement = true;
        }
        return Ok("{}".to_owned());
    }
    let received_at = Instant::now();
    if pre_tool_use && record.scope.profile != LaunchProfile::Antigravity {
        return Err(NOT_FOUND);
    }
    let eval_scope = (!statusline).then(|| {
        (
            record.data_root.clone(),
            record.scope.profile,
            record.scope.session_id.clone(),
            Value::Object(payload.clone()),
        )
    });
    let message = if record.scope.profile == LaunchProfile::Antigravity {
        use crate::antigravity_events::Source;
        let source = if statusline {
            Source::Statusline
        } else {
            Source::Hook
        };
        HookMessage::AntigravityObserved(
            record.scope.clone(),
            source,
            Value::Object(payload),
            received_at,
        )
    } else if record.scope.profile == LaunchProfile::Dsh {
        if statusline {
            return Err(BAD_REQUEST);
        }
        HookMessage::DshObserved(record.scope.clone(), Value::Object(payload), received_at)
    } else {
        let mut observation = Observation::parse_for(&payload, record.scope.profile);
        if statusline {
            observation.statusline = true;
            observation.event = "unknown";
            observation.notification = None;
            observation.codex = None;
        }
        observation.hud_payload = Some(Value::Object(payload));
        HookMessage::Observed(record.scope.clone(), observation, received_at)
    };
    drop(registry);
    match send.try_send(message) {
        Ok(()) => {
            let decision_json = match eval_scope {
                Some((data_root, profile, session_id, payload_val)) => evaluate_hook_fail_open(
                    interceptor,
                    &data_root,
                    profile,
                    &session_id,
                    &payload_val,
                ),
                None => "{}".to_owned(),
            };
            Ok(decision_json)
        }
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
            Err("503 Service Unavailable")
        }
    }
}

struct DeadlineRead<'a> {
    socket: &'a mut TcpStream,
    deadline: Instant,
}

impl Read for DeadlineRead<'_> {
    fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.socket.set_read_timeout(Some(remaining))?;
        self.socket.read(data)
    }
}

fn read_error(error: io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "408 Request Timeout",
        _ => BAD_REQUEST,
    }
}

fn read_line(reader: &mut impl BufRead, budget: &mut usize) -> HttpResult<String> {
    let mut line = Vec::new();
    let length = reader
        .take(*budget as u64)
        .read_until(b'\n', &mut line)
        .map_err(read_error)?;
    *budget -= length;
    let line = line.strip_suffix(b"\r\n").ok_or(BAD_REQUEST)?;
    String::from_utf8(line.to_vec()).map_err(|_| BAD_REQUEST)
}

fn append_body(reader: &mut impl Read, body: &mut Vec<u8>, length: usize) -> HttpResult<()> {
    if length > MAX_HOOK_BYTES - body.len() {
        return Err(TOO_LARGE);
    }
    let start = body.len();
    body.resize(start + length, 0);
    reader.read_exact(&mut body[start..]).map_err(read_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(run: &str) -> Scope {
        Scope {
            profile: LaunchProfile::ClaudeCode,
            session_id: "session".into(),
            run_id: run.into(),
            generation: "generation".into(),
            expected_id: None,
            fork_source_id: None,
        }
    }

    fn request(server: &ProviderHookServer, token: &str, method: &str, body: &[u8]) -> Result<u16> {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port))?;
        socket.set_nodelay(true)?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        write!(
            socket,
            "{method} /hooks/{token} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )?;
        if body.len() <= MAX_HOOK_BYTES {
            socket.write_all(body)?;
        }
        let _ = socket.shutdown(std::net::Shutdown::Write);
        let mut response = String::new();
        BufReader::new(socket).read_line(&mut response)?;
        Ok(response
            .split_ascii_whitespace()
            .nth(1)
            .context("HTTP status")?
            .parse()?)
    }

    #[test]
    fn shutdown_drains_a_connected_hook_that_sends_its_body_at_stop() -> Result<()> {
        let root = tempfile::tempdir()?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let (send, messages) = mpsc::sync_channel(HOOK_QUEUE_CAPACITY);
        let mut server = ProviderHookServer {
            port: listener.local_addr()?.port(),
            registry: Registry::default(),
            interceptor: Arc::new(HookInterceptor::new()),
            messages,
            stop: Arc::new(AtomicBool::new(false)),
            listener: None,
        };
        let registration = server.register(root.path(), scope("drained-run"))?;
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port))?;
        socket.set_nodelay(true)?;
        socket.set_read_timeout(Some(Duration::from_secs(2)))?;
        let body = br#"{"hook_event_name":"SessionStart","session_id":"drained-identity"}"#;
        write!(
            socket,
            "POST /hooks/{} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            registration.token,
            body.len()
        )?;
        // Start at the stop boundary so the connection is still in the TCP backlog.
        server.stop.store(true, Ordering::Relaxed);
        let registry = Arc::clone(&server.registry);
        let interceptor = Arc::clone(&server.interceptor);
        let stop = Arc::clone(&server.stop);
        server.listener = Some(thread::spawn(move || {
            listen(listener, &registry, &interceptor, &send, &stop)
        }));
        socket.write_all(body)?;
        server.finish_requests();

        let mut response = String::new();
        BufReader::new(socket).read_line(&mut response)?;
        assert_eq!(response, "HTTP/1.1 200 OK\r\n");
        let mut messages = server.messages();
        assert!(matches!(
            messages.next(),
            Some(HookMessage::Observed(scope, observation, _))
                if scope.run_id == "drained-run"
                    && observation.identity.as_deref() == Some("drained-identity")
        ));
        assert!(messages.next().is_none());
        Ok(())
    }

    #[test]
    fn windows_callbacks_use_raw_stdin_and_need_no_script_path() -> Result<()> {
        let script = Path::new("C:/Matou data & tools/provider-hooks/ignored.sh");
        let endpoint = "http://127.0.0.1:9000/hooks/token";
        for url in [endpoint.to_owned(), format!("{endpoint}?kind=statusline")] {
            let command = callback_command_for_platform(&url, script, true)?;
            assert_eq!(
                command,
                format!(
                    r#"curl.exe --disable --silent --noproxy "*" --max-time 2 --output NUL --request POST --header "content-type: application/json" --data-binary "@-" "{url}""#
                )
            );
        }
        Ok(())
    }

    #[test]
    fn bundled_claude_plugin_resolves_both_package_layouts() -> Result<()> {
        let root = tempfile::tempdir()?;
        let missing_manifest = root.path().join("missing manifest");
        fs::create_dir_all(missing_manifest.join("providers/claude-plugin/.claude-plugin"))?;
        let manifest_is_directory = root.path().join("manifest is directory");
        fs::create_dir_all(
            manifest_is_directory.join("providers/claude-plugin/.claude-plugin/plugin.json"),
        )?;
        let explicit = root.path().join("explicit assets");
        let explicit_plugin = explicit.join("providers/claude-plugin");
        fs::create_dir_all(explicit_plugin.join(".claude-plugin"))?;
        fs::write(explicit_plugin.join(".claude-plugin/plugin.json"), "{}")?;
        assert_eq!(
            bundled_claude_plugin(None, Some(&explicit)),
            Some(explicit_plugin.clone()),
        );
        for (executable, plugin) in [
            (
                root.path().join("Matou package/matou-native-runtime.exe"),
                root.path().join("Matou package/resources"),
            ),
            (
                root.path()
                    .join("Matou.app/Contents/MacOS/matou-native-runtime"),
                root.path().join("Matou.app/Contents/Resources"),
            ),
        ] {
            let plugin = plugin.join("control-assets/providers/claude-plugin");
            let metadata = plugin.join(".claude-plugin");
            fs::create_dir_all(&metadata)?;
            assert_eq!(bundled_claude_plugin(Some(&executable), None), None);
            fs::write(metadata.join("plugin.json"), "{}")?;
            for unavailable in [
                None,
                Some(Path::new("")),
                Some(missing_manifest.as_path()),
                Some(manifest_is_directory.as_path()),
            ] {
                assert_eq!(
                    bundled_claude_plugin(Some(&executable), unavailable),
                    Some(plugin.clone()),
                );
            }
            assert_eq!(
                bundled_claude_plugin(Some(&executable), Some(&explicit)),
                Some(explicit_plugin.clone()),
            );
        }
        Ok(())
    }

    #[test]
    fn files_and_ingress_match_contract() -> Result<()> {
        let root = tempfile::tempdir()?;
        let server = ProviderHookServer::start()?;
        let run = server.register(root.path(), scope("current"))?;
        let config: Value = serde_json::from_reader(File::open(&run.settings_path)?)?;
        let script = run
            .settings_path
            .with_file_name(format!("{}.statusline.sh", run.token));
        let url = format!("http://127.0.0.1:{}/hooks/{}", server.port, run.token);
        assert_eq!(run.token.len(), 43);
        assert!(
            run.token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        );
        assert!(b"AEIMQUYcgkosw048".contains(&run.token.as_bytes()[42]));
        assert_eq!(config["hooks"].as_object().unwrap().len(), 9);
        for (event, timeout) in [
            ("SessionEnd", 3),
            ("UserPromptSubmit", 5),
            ("PreToolUse", 10),
            ("PostToolUse", 10),
            ("PostToolUseFailure", 5),
            ("PermissionRequest", 10),
            ("Stop", 5),
            ("Notification", 5),
        ] {
            assert_eq!(
                config["hooks"][event],
                json!([{"hooks": [{"type": "http", "url": url, "timeout": timeout}]}])
            );
        }
        let session_start = &config["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(session_start["type"], "command");
        assert_eq!(session_start["timeout"], 5);
        let callback = session_start["command"].as_str().unwrap();
        assert!(callback.contains(&url));
        assert!(callback.contains(if cfg!(windows) {
            r#"--noproxy "*""#
        } else {
            "--noproxy '*'"
        }));
        #[cfg(not(windows))]
        {
            assert_eq!(
                config["statusLine"],
                json!({"type": "command", "command": script, "padding": 0})
            );
            assert_eq!(
                fs::read_to_string(&script)?,
                format!(
                    "#!/bin/sh\n/usr/bin/curl --silent --show-error --max-time 2 --request POST --header 'content-type: application/json' --data-binary @- '{url}?kind=statusline' >/dev/null 2>&1 || true\n"
                )
            );
            assert_eq!(fs::read_dir(script.parent().unwrap())?.count(), 2);
        }
        #[cfg(windows)]
        {
            assert_eq!(
                config["statusLine"],
                json!({"type": "command", "command": windows_callback_command(&format!("{url}?kind=statusline")), "padding": 0})
            );
            assert!(!script.exists());
            assert_eq!(fs::read_dir(script.parent().unwrap())?.count(), 1);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for (path, expected) in [
                (script.parent().unwrap(), 0o700),
                (&run.settings_path, 0o600),
                (&script, 0o700),
            ] {
                assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, expected);
            }
        }

        assert_eq!(request(&server, &run.token, "GET", b"")?, 405);
        assert_eq!(request(&server, "incorrect", "POST", b"")?, 404);
        for body in [b"null".as_slice(), b"[]", b"true", b"\"x\"", b"{"] {
            assert_eq!(request(&server, &run.token, "POST", body)?, 400);
        }
        assert!(server.messages().next().is_none());
        let mut exact = br#"{"hook_event_name":"Stop","session_id":"external"}"#.to_vec();
        exact.resize(MAX_HOOK_BYTES, b' ');
        assert_eq!(request(&server, &run.token, "POST", &exact)?, 200);
        exact.push(b' ');
        assert_eq!(request(&server, &run.token, "POST", &exact)?, 413);
        let Some(HookMessage::Observed(scope, observation, _)) = server.messages().next() else {
            panic!("hook body was not delivered");
        };
        assert_eq!(scope.run_id, "current");
        assert_eq!(observation.identity.as_deref(), Some("external"));
        assert_eq!(
            observation.notification.unwrap().0.kind,
            NotificationKind::Completed
        );
        assert!(server.messages().next().is_none());
        let statusline = format!("{}?kind=statusline", run.token);
        assert_eq!(
            request(
                &server,
                &statusline,
                "POST",
                br#"{"hook_event_name":"Stop","session_id":" exact-ID "}"#,
            )?,
            200
        );
        let Some(HookMessage::Observed(_, statusline, _)) = server.messages().next() else {
            panic!("statusline body was not delivered");
        };
        assert_eq!(statusline.event, "unknown");
        assert_eq!(statusline.identity.as_deref(), Some(" exact-ID "));
        assert!(statusline.notification.is_none());
        Ok(())
    }

    #[test]
    fn notifications_follow_frozen_text_precedence() {
        use NotificationKind::*;

        for (payload, expected) in [
            (
                json!({"hook_event_name": "Stop", "message": "approval"}),
                Some(Completed),
            ),
            (
                json!({"hook_event_name": "PermissionRequest", "message": "done"}),
                Some(Permission),
            ),
            (
                json!({"hook_event_name": "PreToolUse", "message": "done"}),
                None,
            ),
            (
                json!({"hook_event_name": "unknown", "message": "done"}),
                None,
            ),
        ] {
            assert_eq!(
                Observation::parse(payload.as_object().unwrap())
                    .notification
                    .map(|(event, _)| event.kind),
                expected
            );
        }
        for (mut payload, expected) in [
            (
                json!({"message": "DONE but waiting for APPROVAL"}),
                Permission,
            ),
            (json!({"body": "Approve", "text": "Done"}), Permission),
            (
                json!({"message": " \u{feff} ", "body": 123, "text": "Finished, awaiting input"}),
                Completed,
            ),
            (
                json!({"message": "Review this", "body": "success"}),
                Attention,
            ),
            (
                json!({"prompt": "Successful", "error": "permission"}),
                Completed,
            ),
            (
                json!({"error": "COMPLETED", "description": "needs your input"}),
                Completed,
            ),
            (
                json!({"description": "IDLE", "notification": {"message": "done"}}),
                Waiting,
            ),
            (
                json!({"notification": {"message": "Awaiting"}, "data": {"message": "done"}}),
                Waiting,
            ),
            (
                json!({"notification": [{"message": "done"}], "data": {"message": "INPUT"}}),
                Waiting,
            ),
            (
                json!({"message": "", "notification_type": "permission_prompt"}),
                Attention,
            ),
        ] {
            payload["hookEventName"] = json!("Notification");
            let parsed = Observation::parse(payload.as_object().unwrap());
            let event = parsed.notification.unwrap().0;
            assert_eq!(event.kind, expected, "{payload}");
            assert_eq!(event.payload()["subtitle"], expected.labels().1);
            assert_eq!(event.payload()["cooldownKey"], "Notification");
        }
    }

    fn notification(payload: Value) -> Value {
        Observation::parse(payload.as_object().unwrap())
            .notification
            .unwrap()
            .0
            .payload()
    }

    #[test]
    fn stop_notification_uses_the_last_reply_and_cwd_tail() {
        let mut payload = json!({
            "hook_event_name": "Stop", "cwd": "  C:\\workspace/工程\\\\  ",
            "last_assistant_message": " first\u{feff}\nreply ",
            "lastAssistantMessage": "second", "result": "third",
        });
        assert_eq!(
            notification(payload.clone()),
            json!({
                "eventType": "completed", "title": "Claude Code",
                "subtitle": "Completed in 工程", "body": "first reply",
                "sound": true, "cooldownKey": "Stop",
            })
        );
        payload["last_assistant_message"] = json!(" \n ");
        assert_eq!(notification(payload.clone())["body"], "second");
        payload["lastAssistantMessage"] = json!({"text": "ignored"});
        assert_eq!(notification(payload.clone())["body"], "third");
        payload["result"] = Value::Null;
        assert_eq!(
            notification(payload.clone())["body"],
            "Claude session completed in 工程"
        );
        payload["cwd"] = json!("/");
        let event = notification(payload);
        assert_eq!(event["subtitle"], "Completed");
        assert_eq!(event["body"], "Claude session completed");
    }

    #[test]
    fn permission_notification_uses_the_frozen_tool_and_detail_order() {
        let mut payload = json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": " Shell ", "toolName": "Edit",
            "message": " confirm this ", "description": "described",
            "tool_input": {"command": "snake command", "file_path": "snake file"},
            "toolInput": {"command": "camel command", "filePath": "camel file"},
        });
        assert_eq!(
            notification(payload.clone()),
            json!({
                "eventType": "permission", "title": "Claude Code",
                "subtitle": "Permission", "body": "Shell: confirm this",
                "sound": true, "cooldownKey": "Notification",
            })
        );
        for (field, previous, next) in [
            ("/message", "confirm this", "described"),
            ("/description", "described", "snake command"),
            ("/tool_input/command", "snake command", "snake file"),
            ("/tool_input/file_path", "snake file", "camel command"),
            ("/toolInput/command", "camel command", "camel file"),
        ] {
            assert_eq!(
                notification(payload.clone())["body"],
                format!("Shell: {previous}")
            );
            *payload.pointer_mut(field).unwrap() = json!(" ");
            assert_eq!(
                notification(payload.clone())["body"],
                format!("Shell: {next}")
            );
        }
        payload["tool_name"] = json!(" ");
        assert_eq!(notification(payload.clone())["body"], "Edit: camel file");
        payload["toolInput"] = json!([{"filePath": "ignored"}]);
        assert_eq!(
            notification(payload.clone())["body"],
            "Edit requires approval"
        );
        payload["toolName"] = Value::Null;
        assert_eq!(notification(payload)["body"], "operation requires approval");
    }

    #[test]
    fn notifications_compact_and_truncate_at_frozen_utf16_limits() {
        for (hook, field, prefix, limit) in [
            ("Stop", "result", "", 200),
            ("PermissionRequest", "description", "File: ", 180),
            ("Notification", "body", "", 180),
        ] {
            let mut payload = json!({"hook_event_name": hook, "tool_name": "File"});
            let base = format!("a {}", "界".repeat(limit - prefix.len() - 4));
            payload[field] = json!(format!(" a\n\t{}🙂 ", &base[2..]));
            assert_eq!(
                notification(payload.clone())["body"],
                format!("{prefix}{base}🙂")
            );
            payload[field] = json!(format!("{base}🙂 extra"));
            assert_eq!(notification(payload)["body"], format!("{prefix}{base}�…"));
        }
        assert_eq!(
            notification(json!({"hook_event_name": "Notification"})),
            json!({
                "eventType": "attention", "title": "Claude Code",
                "subtitle": "Attention", "body": "Claude needs your attention",
                "sound": true, "cooldownKey": "Notification",
            })
        );
    }

    #[test]
    fn retired_token_loses_authority_before_the_files_expire() -> Result<()> {
        let root = tempfile::tempdir()?;
        let server = ProviderHookServer::start()?;
        let first = server.register(root.path(), scope("first"))?;
        let old = first.token.clone();
        let old_file = first.settings_path.clone();
        let second = server.register(root.path(), scope("second"))?;
        assert_ne!(old, second.token);
        drop(first);
        let body = br#"{"hook_event_name":"UserPromptSubmit","sessionId":"external"}"#;
        assert!(old_file.exists());
        assert_eq!(request(&server, &old, "POST", body)?, 200);
        assert_eq!(request(&server, &second.token, "POST", body)?, 200);
        let mut messages = server.messages();
        assert!(
            matches!(messages.next(), Some(HookMessage::Retired(scope)) if scope.run_id == "first")
        );
        assert!(
            matches!(messages.next(), Some(HookMessage::Observed(scope, _, _)) if scope.run_id == "second")
        );
        assert!(messages.next().is_none());
        drop(second);
        let deadline = Instant::now() + Duration::from_secs(4);
        let directory = old_file.parent().unwrap();
        while fs::read_dir(directory)?.next().is_some() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        assert!(fs::read_dir(directory)?.next().is_none());
        assert_eq!(request(&server, &old, "POST", body)?, 404);
        Ok(())
    }

    #[test]
    fn identity_comes_only_from_hook_fields() {
        let id = "baf44be9-1de8-439a-9fe9-1c45a4022069";
        for field in [
            "session_id",
            "sessionId",
            "conversationId",
            "conversation_id",
        ] {
            let body = json!({field: " observed ", "transcript_path": format!("/path/{id}.jsonl")});
            let observation = Observation::parse(body.as_object().unwrap());
            assert_eq!(observation.identity.as_deref(), Some(" observed "));
        }
        let body = json!({"transcriptPath": format!("/path/{id}.JSONL"), "hookEventName": "Stop"});
        let observation = Observation::parse(body.as_object().unwrap());
        assert_eq!(observation.identity.as_deref(), Some(id));
        assert_eq!(observation.event, "Stop");
        let body = json!({"session_id": " ", "untrusted": id});
        assert!(
            Observation::parse(body.as_object().unwrap())
                .identity
                .is_none()
        );
    }
}
