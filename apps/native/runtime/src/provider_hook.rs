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
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::pty::{LaunchProfile, ShellLaunch};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::codex_events;
use crate::context_guard::{
    interceptor::{HookEvaluation, HookInterceptor},
    managed_cli_instructions,
};

pub(crate) const IDENTITY_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const HOOK_QUEUE_CAPACITY: usize = 128;
const RETIRE_GRACE: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_millis(1500);
const SHUTDOWN_DRAIN: Duration = Duration::from_millis(100);
const MAX_HOOK_BYTES: usize = 1024 * 1024;
const MAX_HEADERS: usize = 16 * 1024;
const MAX_CONNECTIONS: usize = 8;
const EVENTS: [(&str, u8); 10] = [
    ("SessionStart", 5),
    ("SessionEnd", 3),
    ("UserPromptSubmit", 5),
    ("PreToolUse", 10),
    ("PostToolUse", 10),
    ("PostToolUseFailure", 5),
    ("PostCompact", 5),
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
    pub notification: Option<Notification>,
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
            notification: Notification::parse(event, payload, title),
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
    pub received_at: SystemTime,
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
            received_at: SystemTime::now(),
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

pub(crate) const FOREMAN_CAPABILITY_INDEX: &str = "\
## Matou Built-in Task Dispatch (`mt-foreman`)

When the user asks to delegate or hand off work to another coding agent (e.g. 用 Codex / 交给 Codex / 让 Codex 做 / 交给 DSH / 交给 Antigravity / 交给反重力), or when splitting out well-defined execution work:
1. Run `mt-foreman guide` first to read the full brief, supervision, and acceptance workflow on demand (it automatically includes the user's latest Matou agent niche settings; use `mt-foreman roles` to re-check niche changes anytime, and `mt-foreman guide brief|parallel|reference` for templates and command details).
2. Dispatch and supervise tasks with `mt-foreman` (`start`, `watch`, `answer`, `note`, `send`, `status`, `stop`) instead of raw `codex exec` or ad-hoc relay subagents.
3. Do not dispatch recursively if this session is already a foreman worker (`MATOU_FOREMAN_DEPTH` is set).";

pub(crate) fn compose_managed_instructions(
    base: &str,
    foreman_available: bool,
    roles_markdown: Option<&str>,
    context_instructions: Option<&str>,
) -> String {
    let mut instructions = base.to_owned();
    if foreman_available {
        append_instruction_section(&mut instructions, FOREMAN_CAPABILITY_INDEX);
        if let Some(roles) = roles_markdown {
            append_instruction_section(&mut instructions, roles);
        }
    }
    if let Some(context) = context_instructions {
        append_instruction_section(&mut instructions, context);
    }
    instructions
}

fn append_instruction_section(instructions: &mut String, section: &str) {
    instructions.truncate(instructions.trim_end().len());
    instructions.push_str("\n\n");
    instructions.push_str(section);
}

pub(crate) struct Registration {
    token: String,
    registry: Registry,
    pub settings_path: PathBuf,
    script_path: PathBuf,
    data_root: PathBuf,
    url: String,
    provider_assets: Option<tempfile::TempDir>,
}

impl Registration {
    pub fn attach_dsh(
        &mut self,
        dump: &str,
        run_id: &str,
        foreman_available: bool,
        roles_markdown: Option<&str>,
    ) -> Result<PathBuf> {
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
        let system_message = compose_managed_instructions(
            include_str!("../../../runtime/control-assets/providers/dsh-system-message.md"),
            foreman_available,
            roles_markdown,
            managed_cli_instructions(&self.data_root, "dsh"),
        );
        write_private(
            &assets.path().join("dsh-system-message.md"),
            system_message.as_bytes(),
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
        self.provider_assets = Some(assets);
        Ok(path)
    }

    pub fn attach_antigravity_instructions(
        &mut self,
        foreman_available: bool,
        roles_markdown: Option<&str>,
    ) -> Result<PathBuf> {
        let assets = tempfile::Builder::new().prefix("antigravity-").tempdir_in(
            self.settings_path
                .parent()
                .context("Antigravity hook directory")?,
        )?;
        let path = assets.path().join("antigravity-system-message.json");
        let system_message = compose_managed_instructions(
            include_str!("../../../runtime/control-assets/providers/antigravity-system-message.md"),
            foreman_available,
            roles_markdown,
            managed_cli_instructions(&self.data_root, "antigravity"),
        );
        let payload =
            json!({"injectSteps": [{"systemMessage": {"systemMessage": system_message}}]});
        write_private(&path, &serde_json::to_vec(&payload)?, 0o600)?;
        self.provider_assets = Some(assets);
        Ok(path)
    }

    pub fn antigravity_environment(
        &self,
        system_message_file: Option<&Path>,
    ) -> std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> {
        let mut env = std::collections::BTreeMap::from([
            ("MATOU_PROVIDER_HOOK_URL".into(), self.url.clone().into()),
            (
                "MATOU_PROVIDER_HOOK_TOKEN".into(),
                self.token.clone().into(),
            ),
        ]);
        if let Some(path) = system_message_file {
            env.insert(
                "MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE".into(),
                path.as_os_str().to_os_string(),
            );
        }
        env
    }

    pub fn attach_codex(
        &self,
        launch: &mut ShellLaunch,
        foreman_available: bool,
        roles_markdown: Option<&str>,
    ) -> Result<()> {
        let instructions = compose_managed_instructions(
            include_str!(
                "../../../runtime/control-assets/providers/codex-developer-instructions.md"
            ),
            foreman_available,
            roles_markdown,
            managed_cli_instructions(&self.data_root, "codex"),
        );
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
            format!("developer_instructions={}", json!(instructions)),
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
            let status_url = format!("{url}?kind=statusline");
            let command = if cfg!(windows) {
                windows_notification_command(&status_url)
            } else {
                callback_command(&status_url, &files.script)?
            };
            let settings = json!({"statusLine": {
                "type": "command", "command": command,
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
                LaunchProfile::Codex => synchronous_callback_script(&url),
                _ => synchronous_callback_script(&format!("{url}?kind=statusline")),
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
            data_root: data_root.to_path_buf(),
            url,
            provider_assets: None,
        })
    }

    pub fn messages(&self) -> impl Iterator<Item = HookMessage> + '_ {
        self.messages.try_iter()
    }

    pub(crate) fn finish_requests(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
            crate::context_guard::store::flush_saved_stats();
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
    // Codex uses cmd.exe by default. Encode PowerShell's command so neither
    // shell interprets it. Inherit raw stdin and publish stdout only on success.
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
try {{
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = 'curl.exe'
    $start.Arguments = '--disable --silent --fail --noproxy "*" --max-time 2 --request POST --header "content-type: application/json" --data-binary "@-" "{url}"'
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $process = [System.Diagnostics.Process]::Start($start)
    $body = New-Object System.IO.MemoryStream
    $process.StandardOutput.BaseStream.CopyTo($body)
    $process.WaitForExit()
    if ($process.ExitCode -eq 0) {{
        $body.Position = 0
        $body.CopyTo([Console]::OpenStandardOutput())
    }}
}} catch {{}}
exit 0
"#
    );
    let encoded = STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    format!("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {encoded}")
}

fn windows_notification_command(url: &str) -> String {
    format!(
        r#"curl.exe --disable --silent --noproxy "*" --max-time 2 --output NUL --request POST --header "content-type: application/json" --data-binary "@-" "{url}""#
    )
}

fn bundled_control_assets_candidates(
    executable: Option<&Path>,
    control_asset_root: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = control_asset_root.filter(|root| !root.as_os_str().is_empty()) {
        roots.push(root.to_path_buf());
    }
    if let Some(executable) = executable
        && let Some(executable_dir) = executable.parent()
    {
        let resources = if executable
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            Some(executable_dir.join("resources"))
        } else {
            executable_dir.parent().and_then(|contents| {
                (executable_dir.file_name() == Some(std::ffi::OsStr::new("MacOS"))
                    && contents.file_name() == Some(std::ffi::OsStr::new("Contents")))
                .then(|| contents.join("Resources"))
            })
        };
        if let Some(resources) = resources {
            roots.push(resources.join("control-assets"));
        }
    }
    roots
}

pub(crate) fn bundled_claude_plugin(
    executable: Option<&Path>,
    control_asset_root: Option<&Path>,
) -> Option<PathBuf> {
    bundled_control_assets_candidates(executable, control_asset_root)
        .into_iter()
        .find_map(|root| {
            let plugin = root.join("providers/claude-plugin");
            plugin
                .join(".claude-plugin/plugin.json")
                .is_file()
                .then_some(plugin)
        })
}

pub(crate) fn bundled_foreman_script(
    executable: Option<&Path>,
    control_asset_root: Option<&Path>,
) -> Option<PathBuf> {
    bundled_control_assets_candidates(executable, control_asset_root)
        .into_iter()
        .find_map(|root| {
            let script =
                root.join("providers/claude-plugin/skills/agent-foreman/scripts/foreman.mjs");
            script.is_file().then_some(script)
        })
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
        |mcp: &mut BTreeSet<String>, settings: &Value, disabled: &[&str], include_hooks: bool| {
            if let Some(servers) = settings["mcpServers"].as_object() {
                mcp.extend(servers.keys().cloned());
            }
            for name in disabled
                .iter()
                .filter_map(|key| settings[*key].as_array())
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
        add_config(&mut user_mcp, &read(config.join(name)), &[], true);
    }
    let claude_json = config
        .parent()
        .map(|parent| read(parent.join(".claude.json")))
        .unwrap_or_default();
    add_config(&mut user_mcp, &claude_json, &["disabledMcpServers"], false);
    add_config(&mut project_mcp, &read(cwd.join(".mcp.json")), &[], false);
    for name in ["settings.json", "settings.local.json"] {
        add_config(
            &mut project_mcp,
            &read(project.join(name)),
            &["disabledMcpjsonServers"],
            true,
        );
    }
    if let Some(projects) = claude_json["projects"].as_object() {
        let project_at =
            |path: &Path| projects.get(path.to_str()?).filter(|entry| entry.is_object());
        let local = project_at(cwd)
            .or_else(|| fs::canonicalize(cwd).ok().and_then(|path| project_at(&path)));
        if let Some(local) = local {
            add_config(
                &mut project_mcp,
                local,
                &["disabledMcpServers", "disabledMcpjsonServers"],
                false,
            );
        }
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
        windows_notification_command(url)
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
fn synchronous_callback_script(url: &str) -> String {
    format!(
        "#!/bin/sh\nresponse=$(/usr/bin/curl --disable --silent --fail --noproxy '*' --max-time 2 --request POST --header 'content-type: application/json' --data-binary @- '{url}' 2>/dev/null && printf '.') || exit 0\nprintf '%s' \"${{response%.}}\"\n"
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
    budget: Duration,
) -> HookEvaluation {
    let deadline = Instant::now() + budget;
    let (tx, rx) = mpsc::sync_channel(1);
    let interceptor = Arc::clone(interceptor);
    let data_root = data_root.to_path_buf();
    let session_id = session_id.to_owned();
    let payload = payload.clone();
    let spawned = thread::Builder::new()
        .name("context-guard-eval".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                interceptor.evaluate_hook(&data_root, profile, &session_id, &payload, deadline)
            }))
            .unwrap_or_else(|_| HookEvaluation::allow());
            let _ = tx.try_send(result);
        });
    if spawned.is_err() {
        return HookEvaluation::allow();
    }
    let wait = deadline.saturating_duration_since(Instant::now());
    rx.recv_timeout(wait)
        .unwrap_or_else(|_| HookEvaluation::allow())
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
    let (status, body) = match &result {
        Ok((decision, _)) => ("200 OK", decision.response_json.as_str()),
        Err(status) => (*status, ""),
    };
    let _ = socket.set_write_timeout(Some(Duration::from_millis(100)));
    if write!(
        socket,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ).is_ok() {
        if let Ok((decision, Some((data_root, session_id)))) = result {
            interceptor.record_delivered_savings(data_root, &session_id, decision, SystemTime::now());
        }
    }
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
) -> HttpResult<(HookEvaluation, Option<(PathBuf, String)>)> {
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
    let tool_event = match kind {
        "kind=pre-tool-use" => Some("PreToolUse"),
        "kind=post-tool-use" => Some("PostToolUse"),
        "" | "kind=statusline" => None,
        _ => return Err(NOT_FOUND),
    };
    if !matches!(fields.next(), Some("HTTP/1.0" | "HTTP/1.1")) || fields.next().is_some() {
        return Err(BAD_REQUEST);
    }
    let profile = registry
        .lock()
        .unwrap()
        .get(token)
        .ok_or(NOT_FOUND)?
        .scope
        .profile;

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
    let mut payload: Map<String, Value> = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(_) if profile == LaunchProfile::ClaudeCode && !statusline => {
            return Ok((HookEvaluation::allow(), None));
        }
        Err(_) => return Err(BAD_REQUEST),
    };
    let mut registry = registry.lock().unwrap();
    let record = registry.get_mut(token).ok_or(NOT_FOUND)?;
    if let Some(event) = tool_event {
        if record.scope.profile != LaunchProfile::Antigravity {
            return Err(NOT_FOUND);
        }
        payload.remove("hook_event_name");
        payload.insert("hookEventName".to_owned(), json!(event));
    }
    let agy_pre_tool_use = record.scope.profile == LaunchProfile::Antigravity
        && payload.get("hookEventName").and_then(Value::as_str) == Some("PreToolUse");
    let provider_response = |mut evaluation: HookEvaluation, allow_sandbox_bypass: bool| {
        if agy_pre_tool_use && evaluation.response_json == "{}" {
            // The native response requires a decision. Allow leaves approval to
            // the next hook or Antigravity's default permission policy, except
            // for whitelisted Matou control commands that live under ~/.matou.
            evaluation.response_json = if allow_sandbox_bypass {
                r#"{"decision":"allow","overwrite":{"BypassSandbox":true}}"#.to_owned()
            } else {
                r#"{"decision":"allow"}"#.to_owned()
            };
        }
        evaluation
    };
    if record.retired.is_some() {
        if !record.reported_retirement
            && send
                .try_send(HookMessage::Retired(record.scope.clone()))
                .is_ok()
        {
            record.reported_retirement = true;
        }
        return Ok((provider_response(HookEvaluation::allow(), false), None));
    }
    let agy_sandbox_bypass =
        agy_pre_tool_use && should_bypass_antigravity_sandbox(&payload, &record.data_root);
    let received_at = Instant::now();
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
    // Keep admission timestamps and queue order aligned for concurrent hooks.
    let queued = send.try_send(message);
    drop(registry);
    match queued {
        Ok(()) => {
            let Some((data_root, profile, session_id, payload_val)) = eval_scope else {
                let mut response = HookEvaluation::allow();
                response.response_json.clear();
                return Ok((response, None));
            };
            let decision = evaluate_hook_fail_open(
                interceptor,
                &data_root,
                profile,
                &session_id,
                &payload_val,
                HOOK_DECISION_BUDGET,
            );
            Ok((
                provider_response(decision, agy_sandbox_bypass),
                matches!(
                    profile,
                    LaunchProfile::ClaudeCode | LaunchProfile::Antigravity
                )
                .then_some((data_root, session_id)),
            ))
        }
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
            Err("503 Service Unavailable")
        }
    }
}

/// Allows Antigravity's terminal sandbox to be bypassed only for direct,
/// shell-metacharacter-free `mt` / `mt-foreman` control-plane commands
/// (optionally chained with `&&`). `mt ctx exec` and unknown `mt` subcommands
/// are explicitly rejected so arbitrary commands remain sandboxed.
fn should_bypass_antigravity_sandbox(payload: &Map<String, Value>, data_root: &Path) -> bool {
    let Some(tool_call) = payload.get("toolCall").and_then(Value::as_object) else {
        return false;
    };
    if tool_call.get("name").and_then(Value::as_str) != Some("run_command") {
        return false;
    }
    let Some(raw_command) = tool_call
        .get("args")
        .and_then(Value::as_object)
        .and_then(|args| args.get("CommandLine"))
        .and_then(Value::as_str)
    else {
        return false;
    };
    let trimmed = raw_command.trim();
    if trimmed.is_empty() {
        return false;
    }

    let bin_dir = data_root.join("control").join("bin");
    trimmed
        .split("&&")
        .all(|segment| is_whitelisted_matou_segment(segment.trim(), &bin_dir))
}

fn is_whitelisted_matou_segment(segment: &str, bin_dir: &Path) -> bool {
    if segment.is_empty() || segment.chars().any(is_disallowed_shell_char) {
        return false;
    }
    let Some((first_word, rest)) = split_first_shell_word(segment) else {
        return false;
    };
    if matches_control_binary(first_word, bin_dir, "mt-foreman") {
        return true;
    }
    if matches_control_binary(first_word, bin_dir, "mt") {
        return is_whitelisted_mt_args(rest);
    }
    false
}

fn is_disallowed_shell_char(ch: char) -> bool {
    matches!(
        ch,
        ';' | '|' | '`' | '$' | '(' | ')' | '<' | '>' | '{' | '}' | '&' | '\r' | '\n' | '\0'
    )
}

fn matches_control_binary(word: &str, bin_dir: &Path, name: &str) -> bool {
    if word == name {
        return true;
    }
    let word_path = Path::new(word);
    if word_path == bin_dir.join(name) || word_path == bin_dir.join(format!("{name}.cmd")) {
        return true;
    }
    #[cfg(windows)]
    {
        if word.eq_ignore_ascii_case(&bin_dir.join(name).to_string_lossy())
            || word.eq_ignore_ascii_case(&bin_dir.join(format!("{name}.cmd")).to_string_lossy())
        {
            return true;
        }
    }
    false
}

fn is_whitelisted_mt_args(rest: &str) -> bool {
    if rest.is_empty() {
        return true;
    }
    let Some((subcommand, after_sub)) = split_first_shell_word(rest) else {
        return false;
    };
    match subcommand {
        "identify" | "list" | "read" | "history" | "commands" | "send" | "key" | "create"
        | "fork" | "remove" | "close" | "focus" | "switch" | "help" | "--help" | "-h" => true,
        "ctx" => {
            if after_sub.is_empty() {
                return true;
            }
            let Some((ctx_sub, _)) = split_first_shell_word(after_sub) else {
                return false;
            };
            matches!(
                ctx_sub,
                "outline"
                    | "batch-search"
                    | "index"
                    | "log"
                    | "search"
                    | "resume"
                    | "help"
                    | "--help"
                    | "-h"
            )
        }
        _ => false,
    }
}

fn split_first_shell_word(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim_start();
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    if first == '"' || first == '\'' {
        let end = trimmed[1..].find(first)?;
        if end == 0 {
            return None;
        }
        let word = &trimmed[1..1 + end];
        let after = &trimmed[2 + end..];
        if !after.is_empty() && !after.starts_with(char::is_whitespace) {
            return None;
        }
        Some((word, after.trim_start()))
    } else {
        match trimmed.find(char::is_whitespace) {
            Some(pos) => Some((&trimmed[..pos], trimmed[pos..].trim_start())),
            None => Some((trimmed, "")),
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

    #[test]
    fn claude_config_counts_include_local_project_mcps_and_filter_disabled_servers() -> Result<()> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let config = home.join(".claude");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&config)?;
        fs::create_dir(&workspace)?;
        let cwd = workspace.canonicalize()?;
        let original = workspace.join(".");
        let alias = workspace.join(".").join(".");
        fs::write(
            cwd.join(".mcp.json"),
            r#"{"mcpServers":{"shared":{},"disabled-shared":{}}}"#,
        )?;
        fs::write(
            home.join(".claude.json"),
            json!({
                "mcpServers": {"global": {}},
                "projects": {
                    (original.to_str().context("original path")?): {"mcpServers": {"exact": {}}},
                    (cwd.to_str().context("canonical path")?): {
                        "mcpServers": {"local": {}, "disabled-local": {}},
                        "disabledMcpServers": ["disabled-local"],
                        "disabledMcpjsonServers": ["disabled-shared"],
                    },
                },
            })
            .to_string(),
        )?;

        let counts = claude_config_counts(&alias, &config);
        assert_eq!(counts["mcps"], 3);
        assert_eq!(counts["mcpServers"], 3);
        assert_eq!(counts["mcpServerNames"], json!(["global", "local", "shared"]));

        let exact = claude_config_counts(&original, &config);
        assert_eq!(
            exact["mcpServerNames"],
            json!(["global", "disabled-shared", "exact", "shared"])
        );
        Ok(())
    }

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

    fn claude_hook_reply(
        server: &ProviderHookServer,
        run: &Registration,
        body: &str,
    ) -> Result<String> {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port))?;
        socket.set_read_timeout(Some(Duration::from_secs(2)))?;
        write!(
            socket,
            "POST /hooks/{} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
            run.token,
            body.len()
        )?;
        let mut response = String::new();
        socket.read_to_string(&mut response)?;
        ensure!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        Ok(response
            .split_once("\r\n\r\n")
            .context("HTTP response headers")?
            .1
            .to_owned())
    }

    fn replay_callback(
        command: &mut std::process::Command,
        payload: &str,
    ) -> Result<std::process::Output> {
        use std::process::Stdio;

        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .context("callback stdin")?
            .write_all(payload.as_bytes())?;
        Ok(child.wait_with_output()?)
    }

    fn callback_fixture(
        listener: TcpListener,
        response: &'static str,
        stall_after_body: Option<Duration>,
    ) -> JoinHandle<Result<(String, String)>> {
        thread::spawn(move || {
            let (mut socket, _) = listener.accept()?;
            socket.set_read_timeout(Some(Duration::from_secs(3)))?;
            let mut reader = BufReader::new(&mut socket);
            let mut request = String::new();
            reader.read_line(&mut request)?;
            let mut length = 0;
            loop {
                let mut header = String::new();
                ensure!(
                    reader.read_line(&mut header)? > 0,
                    "missing callback headers"
                );
                if header == "\r\n" {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>()?;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len() + usize::from(stall_after_body.is_some())
            )?;
            if let Some(delay) = stall_after_body {
                thread::sleep(delay);
            }
            Ok((request, String::from_utf8(body)?))
        })
    }

    #[test]
    fn codex_callback_preserves_raw_decision_and_fails_open_on_connection_errors() -> Result<()> {
        use std::process::Command;

        let root = tempfile::tempdir()?;
        let payload = "{\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Read\"}\n";
        let decision = "{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\"permissionDecision\":\"deny\",\"permissionDecisionReason\":\"决策\"}}\n\n";
        for (case, stall) in [
            ("success", None),
            ("timeout", Some(Duration::from_millis(2300))),
            ("unreachable", None),
        ] {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            let url = format!("http://{}/hooks/fixture", listener.local_addr()?);
            let fixture = if case == "unreachable" {
                drop(listener);
                None
            } else {
                Some(callback_fixture(listener, decision, stall))
            };
            let script = root.path().join(format!("{case}.sh"));
            #[cfg(unix)]
            let mut command = {
                write_private(&script, synchronous_callback_script(&url).as_bytes(), 0o700)?;
                let mut shell = Command::new("/bin/sh");
                shell.args(["-c", &callback_command(&url, &script)?]);
                shell
            };
            #[cfg(windows)]
            let mut command = {
                use std::os::windows::process::CommandExt;
                let callback = callback_command(&url, &script)?;
                let mut shell = Command::new("cmd.exe");
                shell.args(["/D", "/C"]).raw_arg(format!(r#""{callback}""#));
                shell
            };
            let output = replay_callback(&mut command, payload)?;
            let expected = if case == "success" { decision } else { "" };
            assert!(output.status.success(), "{case}: {:?}", output);
            assert_eq!(output.stdout, expected.as_bytes(), "{case}");
            assert!(output.stderr.is_empty(), "{case}: {:?}", output);
            if let Some(worker) = fixture {
                let (request, raw_body) = worker.join().unwrap()?;
                assert_eq!(request, "POST /hooks/fixture HTTP/1.1\r\n");
                assert_eq!(raw_body, payload);
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn antigravity_script_routes_tool_events_without_waiting_for_post_tool_use() -> Result<()> {
        use std::process::Command;

        let temporary = tempfile::tempdir()?;
        let plugin = temporary.path().canonicalize()?.join("matou");
        crate::antigravity_launch::ensure_plugin_installed(&plugin)?;
        let hooks: Value = serde_json::from_reader(File::open(plugin.join("hooks.json"))?)?;
        let decision = "{\"decision\":\"allow\",\"overwrite\":{\"CommandLine\":\"mt ctx exec -- git status\"}}\n\n";
        let nested = "{\"toolCall\":{\"name\":\"run_command\",\"args\":{\"CommandLine\":\"git status\",\"WaitMsBeforeAsync\":5000}}}\n";
        let old = "{\"hookEventName\":  \"PreToolUse\"}\n";
        for (command, kind, payload, stall) in [
            (
                hooks["matou"]["PreToolUse"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap(),
                "pre-tool-use",
                nested,
                None,
            ),
            (
                hooks["matou"]["PostToolUse"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap(),
                "post-tool-use",
                old,
                Some(Duration::from_millis(750)),
            ),
            ("./hooks/matou-hook.sh", "pre-tool-use", old, None),
            (
                "./hooks/matou-hook.sh pre-tool-use",
                "pre-tool-use",
                nested,
                Some(Duration::from_millis(1300)),
            ),
        ] {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            let url = format!("http://{}/hooks/fixture", listener.local_addr()?);
            let fixture = callback_fixture(listener, decision, stall);
            let started = Instant::now();
            let output = replay_callback(
                Command::new("/bin/sh")
                    .args(["-c", command])
                    .current_dir(&plugin)
                    .env("MATOU_PROVIDER_HOOK_URL", url)
                    .env("MATOU_PROVIDER_HOOK_TOKEN", "test"),
                payload,
            )?;
            let elapsed = started.elapsed();
            let expected = if stall.is_none() { decision } else { "" };
            assert!(output.status.success());
            assert_eq!(output.stdout, expected.as_bytes());
            if kind == "post-tool-use" {
                assert!(
                    elapsed < Duration::from_millis(750),
                    "async hook took {elapsed:?}"
                );
            } else if stall.is_some() {
                assert!(
                    (Duration::from_millis(850)..Duration::from_millis(1300)).contains(&elapsed),
                    "timeout took {elapsed:?}"
                );
            }
            let (request, raw_body) = fixture.join().unwrap()?;
            assert_eq!(
                request,
                format!("POST /hooks/fixture?kind={kind} HTTP/1.1\r\n")
            );
            assert_eq!(raw_body, payload);
        }
        Ok(())
    }

    #[test]
    fn antigravity_ingress_accepts_event_hint_and_records_delivered_read_savings() -> Result<()> {
        use crate::context_guard::store::get_saved_stats;
        use matou_native_core::context_optimizer_config::{
            AgentOptimizerSettings, ContextOptimizerConfig,
        };

        let root = tempfile::tempdir()?;
        ContextOptimizerConfig::for_data_root(root.path()).set_agent_settings(
            "antigravity",
            AgentOptimizerSettings {
                read_dedup: true,
                block_sensitive_reads: true,
                ..Default::default()
            },
        )?;
        let path = root.path().join("source.txt");
        let content = "repeatable for deduplication\n".repeat(350);
        fs::write(&path, &content)?;
        let server = ProviderHookServer::start()?;
        let registration = server.register(
            root.path(),
            Scope {
                profile: LaunchProfile::Antigravity,
                ..scope("agy")
            },
        )?;
        let post = |kind: &str, payload: &Value| -> Result<Value> {
            let body = payload.to_string();
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port))?;
            socket.set_read_timeout(Some(Duration::from_secs(2)))?;
            write!(
                socket,
                "POST /hooks/{}?kind={kind} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
                registration.token,
                body.len()
            )?;
            let mut response = String::new();
            socket.read_to_string(&mut response)?;
            ensure!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
            Ok(serde_json::from_str(
                response.split_once("\r\n\r\n").context("response")?.1,
            )?)
        };
        let read = |path: &Path, step| json!({"toolCall":{"name":"view_file","args":{"AbsolutePath":path}}, "conversationId":"agy-host", "stepIdx":step});
        // agy 1.5.17 sends the event and uses read_file with Path in the native CLI.
        let native_read = |path: &Path, step: u64| {
            json!({
                "hookEventName": "PreToolUse",
                "conversationId": "agy-host",
                "stepIdx": step,
                "cwd": root.path(),
                "workspacePaths": [root.path()],
                "toolCall": {"id": format!("call_{step}"), "name": "read_file", "args": {"Path": path}},
            })
        };
        assert_eq!(
            post("pre-tool-use", &read(&path, 19))?,
            json!({"decision":"allow"})
        );
        assert_eq!(post("post-tool-use", &read(&path, 19))?, json!({}));
        let response = post("pre-tool-use", &native_read(&path, 20))?;
        assert_eq!(response["decision"], "deny");
        assert!(response["reason"].as_str().unwrap().contains("120s"));
        assert_eq!(server.saved_noise_bytes("session"), content.len() as u64);
        let stats = serde_json::to_value(get_saved_stats(root.path(), SystemTime::now()))?;
        assert_eq!(
            stats["agents"]["antigravity"],
            json!({"bytes7d":content.len(), "events7d":1})
        );
        let blocked = post("pre-tool-use", &native_read(&root.path().join(".env"), 21))?;
        assert_eq!(blocked["decision"], "deny");
        assert!(
            blocked["reason"]
                .as_str()
                .unwrap()
                .contains("sensitive file")
        );
        assert_eq!(
            post("pre-tool-use", &native_read(&root.path().join(".env.example"), 22))?,
            json!({"decision":"allow"})
        );
        Ok(())
    }

    #[test]
    fn antigravity_sandbox_bypass_allows_whitelisted_mt_commands_and_rejects_unsafe_ones() {
        let data_root = Path::new("/Users/tester/.matou");
        let run_cmd = |command: &str| {
            json!({
                "hookEventName": "PreToolUse",
                "toolCall": {
                    "name": "run_command",
                    "args": { "CommandLine": command }
                }
            })
            .as_object()
            .unwrap()
            .clone()
        };

        let allowed = [
            "mt",
            "mt identify --json",
            "mt list --all --json",
            "mt read session:1 --json",
            "mt send left \"hello world\" --enter",
            "mt key left enter",
            "mt ctx",
            "mt ctx search foo",
            "mt ctx log --limit 20",
            "mt-foreman guide",
            "mt-foreman status",
            "mt-foreman start task-01 --cd /tmp/repo --brief /tmp/brief.md",
            "mt identify --json && mt list --json",
            "mt identify --json && mt-foreman guide && mt-foreman roles",
            "\"mt\" \"identify\" --json",
            "'mt-foreman' guide",
            "/Users/tester/.matou/control/bin/mt list --json",
            "\"/Users/tester/.matou/control/bin/mt\" identify --json",
            "/Users/tester/.matou/control/bin/mt-foreman guide",
            "/Users/tester/.matou/control/bin/mt.cmd list --json",
            "/Users/tester/.matou/control/bin/mt-foreman.cmd status",
        ];
        for command in allowed {
            assert!(
                should_bypass_antigravity_sandbox(&run_cmd(command), data_root),
                "expected sandbox bypass for: {command}"
            );
        }

        let rejected = [
            "",
            "   ",
            "git status",
            "ls ~/.matou/control/bin",
            "mtx foo",
            "sudo mt list",
            "mt shell",
            "mt unknown-subcommand",
            "mt ctx exec -- rm -rf /",
            "mt ctx exec --session s1 -- ls",
            "mt 'ctx' \"exec\" -- ls",
            "mt ctx --session s1 exec -- ls",
            "mt list --json && mt ctx exec -- rm -rf /",
            "mt list --json && rm -rf /",
            "mt identify --json && ",
            "&& mt identify --json",
            "mt list | head",
            "mt list; rm -rf /",
            "mt list &",
            "mt identify --json &&& mt list --json",
            "mt $(whoami)",
            "mt `whoami`",
            "mt list > /tmp/out",
            "mt-foreman start t1 <<'EOF'\nhi\nEOF",
            "mt list\nrm -rf /",
            "mt \"\"",
            "\"mt\"list --json",
            "/tmp/evil/mt list --json",
        ];
        for command in rejected {
            assert!(
                !should_bypass_antigravity_sandbox(&run_cmd(command), data_root),
                "expected sandbox restriction for: {command:?}"
            );
        }

        let non_run_command = json!({
            "hookEventName": "PreToolUse",
            "toolCall": {
                "name": "read_file",
                "args": { "CommandLine": "mt identify --json", "Path": "/tmp/a" }
            }
        })
        .as_object()
        .unwrap()
        .clone();
        assert!(!should_bypass_antigravity_sandbox(
            &non_run_command,
            data_root
        ));

        let missing_command = json!({
            "hookEventName": "PreToolUse",
            "toolCall": { "name": "run_command", "args": {} }
        })
        .as_object()
        .unwrap()
        .clone();
        assert!(!should_bypass_antigravity_sandbox(
            &missing_command,
            data_root
        ));
    }

    #[test]
    fn antigravity_pre_tool_use_bypasses_sandbox_for_mt_without_overriding_non_mt_or_retired_or_deny()
    -> Result<()> {
        use matou_native_core::context_optimizer_config::{
            AgentOptimizerSettings, ContextOptimizerConfig,
        };

        let root = tempfile::tempdir()?;
        ContextOptimizerConfig::for_data_root(root.path()).set_agent_settings(
            "antigravity",
            AgentOptimizerSettings {
                block_sensitive_reads: true,
                ..Default::default()
            },
        )?;
        let server = ProviderHookServer::start()?;
        let agy = server.register(
            root.path(),
            Scope {
                profile: LaunchProfile::Antigravity,
                ..scope("agy-sandbox")
            },
        )?;
        let claude = server.register(root.path(), scope("claude-control"))?;

        let post_agy = |token: &str, kind: &str, payload: &Value| -> Result<Value> {
            let body = payload.to_string();
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port))?;
            socket.set_read_timeout(Some(Duration::from_secs(2)))?;
            write!(
                socket,
                "POST /hooks/{token}?kind={kind} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )?;
            let mut response = String::new();
            socket.read_to_string(&mut response)?;
            ensure!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
            Ok(serde_json::from_str(
                response.split_once("\r\n\r\n").context("response")?.1,
            )?)
        };
        let run_cmd = |command: &str| {
            json!({
                "conversationId": "agy-sandbox-conv",
                "stepIdx": 2,
                "toolCall": {
                    "id": "call_1",
                    "name": "run_command",
                    "args": { "CommandLine": command }
                }
            })
        };

        let bypass = json!({"decision":"allow","overwrite":{"BypassSandbox":true}});
        let allow_only = json!({"decision":"allow"});
        let abs_mt = format!(
            "{} list --json",
            root.path().join("control/bin/mt").display()
        );

        for command in [
            "mt identify --json",
            "mt-foreman guide",
            "mt identify --json && mt list --json",
            "mt ctx search foo",
            abs_mt.as_str(),
        ] {
            assert_eq!(
                post_agy(&agy.token, "pre-tool-use", &run_cmd(command))?,
                bypass,
                "expected BypassSandbox overwrite for {command}"
            );
        }

        for command in [
            "mt ctx exec -- rm -rf /",
            "mt list --json && mt ctx exec -- rm -rf /",
            "git status",
            "mt list | head",
        ] {
            assert_eq!(
                post_agy(&agy.token, "pre-tool-use", &run_cmd(command))?,
                allow_only,
                "expected plain allow without overwrite for {command}"
            );
        }

        // PostToolUse never receives a PreToolUse overwrite.
        assert_eq!(
            post_agy(&agy.token, "post-tool-use", &run_cmd("mt identify --json"))?,
            json!({})
        );

        // Explicit interceptor deny is preserved.
        let env_read = json!({
            "conversationId": "agy-sandbox-conv",
            "stepIdx": 3,
            "toolCall": {
                "id": "call_env",
                "name": "read_file",
                "args": { "Path": root.path().join(".env") }
            }
        });
        let denied = post_agy(&agy.token, "pre-tool-use", &env_read)?;
        assert_eq!(denied["decision"], "deny");

        // Claude Code profile is unaffected.
        let claude_payload = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "run_command",
            "toolCall": {
                "name": "run_command",
                "args": { "CommandLine": "mt identify --json" }
            }
        })
        .to_string();
        assert_eq!(claude_hook_reply(&server, &claude, &claude_payload)?, "{}");

        // Retired Antigravity token never receives BypassSandbox overwrite.
        let retired_token = agy.token.clone();
        drop(agy);
        assert_eq!(
            post_agy(&retired_token, "pre-tool-use", &run_cmd("mt identify --json"))?,
            allow_only
        );
        Ok(())
    }

    #[test]
    fn claude_http_replacement_counts_the_persisted_preview_once_and_keeps_the_full_log()
    -> Result<()> {
        use crate::context_guard::store::{ContextGuardStore, get_saved_stats};
        use matou_native_core::context_optimizer_config::{
            AgentOptimizerSettings, ContextOptimizerConfig,
        };

        let root = tempfile::tempdir()?;
        let server = ProviderHookServer::start()?;
        let run = server.register(root.path(), scope("ctx-trim"))?;
        let session = Uuid::new_v4().to_string();
        let project = root.path().join("projects/workspace");
        let transcript = project.join(format!("{session}.jsonl"));
        let path = project
            .join(&session)
            .join("tool-results")
            .join("b50kb.txt");
        fs::create_dir_all(path.parent().unwrap())?;
        let full_log = format!("{}\n", "L".repeat(50_000));
        fs::write(&path, &full_log)?;
        let payload = json!({
            "hook_event_name": "PostToolUse", "session_id": session, "transcript_path": transcript,
            "tool_use_id": "toolu_50kb", "tool_name": "Bash", "tool_input": {"command": "fixture"},
            "tool_response": {"stdout": &full_log[..30_000], "stderr": "", "interrupted": false,
                "isImage": false, "persistedOutputPath": path, "persistedOutputSize": full_log.len()}
        }).to_string();
        let config = ContextOptimizerConfig::for_data_root(root.path());
        config.set_agent_settings(
            "claude-code",
            AgentOptimizerSettings {
                noise_filter: false,
                ..AgentOptimizerSettings::recommended_for("claude-code")
            },
        )?;
        assert_eq!(claude_hook_reply(&server, &run, &payload)?, "{}");
        assert_eq!(claude_hook_reply(&server, &run, "{")?, "{}");
        assert_eq!(server.saved_noise_bytes("session"), 0);
        assert!(!root.path().join("context_guard.sqlite").exists());
        config.set_agent_settings(
            "claude-code",
            AgentOptimizerSettings {
                noise_filter: true,
                ..Default::default()
            },
        )?;
        let response = claude_hook_reply(&server, &run, &payload)?;
        let reply: Value = serde_json::from_str(&response)?;
        let replacement = &reply["hookSpecificOutput"]["updatedToolOutput"];
        let summary = replacement["stdout"].as_str().context("Bash replacement")?;
        assert!(replacement.get("persistedOutputPath").is_none());
        assert!(summary.contains("mt ctx log log_0001"));
        let original_model_body = format!(
            "<persisted-output>\nOutput too large (48.8KB). Full output saved to: {}\n\nPreview (first 2KB):\n{}\n...\n</persisted-output>",
            path.display(),
            "L".repeat(2000),
        );
        let saved = (original_model_body.len() - summary.len()) as u64;
        assert_eq!(server.saved_noise_bytes("session"), saved);
        assert_eq!(claude_hook_reply(&server, &run, &payload)?, response);
        assert_eq!(server.saved_noise_bytes("session"), saved);
        let stats = serde_json::to_value(get_saved_stats(root.path(), SystemTime::now()))?;
        assert_eq!(
            stats["agents"]["claude-code"],
            json!({"bytes7d": saved, "events7d": 1})
        );
        let store = ContextGuardStore::open(root.path())?;
        assert_eq!(store.list_session_logs("session", 10)?.len(), 1);
        assert_eq!(store.get_full_log("log_0001")?, Some(full_log));

        fs::remove_file(path)?;
        let partial = claude_hook_reply(
            &server,
            &run,
            &payload.replace("toolu_50kb", "toolu_missing"),
        )?;
        assert!(partial.contains("Available output saved"));
        assert_eq!(
            store.get_full_log("log_0002")?.as_deref(),
            Some("L".repeat(30_000).as_str())
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stats_command_counts_delivered_replacement_and_read_dedup_once() -> Result<()> {
        use crate::host::{Command, Event, LaunchOptions, OfflineFixtures, RuntimeHost};
        use matou_native_core::context_optimizer_config::{
            AgentOptimizerSettings, ContextOptimizerConfig,
        };

        let root = tempfile::tempdir()?;
        let data_root = root.path().join("data");
        let mut runtime = RuntimeHost::start_with_offline_fixtures(
            LaunchOptions {
                data_root: data_root.clone(),
                workspace: root.path().to_owned(),
                shell_home: root.path().to_owned(),
                shell: "/bin/sh".into(),
            },
            OfflineFixtures::default(),
        )?;
        let read_stats = |request_id| -> Result<Value> {
            runtime
                .commands
                .send(Command::GetContextOptimizerStats { request_id })
                .map_err(|_| anyhow::anyhow!("runtime closed before the stats command"))?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match runtime.events.try_recv() {
                    Ok(Event::Graph(value))
                        if value["type"] == "contextOptimizerStatsResult"
                            && value["requestId"] == request_id =>
                    {
                        return Ok(value["result"].clone());
                    }
                    Ok(Event::Diagnostic(message)) => anyhow::bail!("{message}"),
                    Ok(_) => {}
                    Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => return Err(error).context("waiting for stats command"),
                }
            }
        };
        let server = ProviderHookServer::start()?;
        let run = server.register(&data_root, scope("stats"))?;
        let config = ContextOptimizerConfig::for_data_root(&data_root);
        let settings = AgentOptimizerSettings {
            read_dedup: true,
            noise_filter: true,
            ..Default::default()
        };
        config.set_agent_settings("claude-code", settings.clone())?;
        let empty = json!({"agents": {
            "claude-code": {"bytes7d": 0, "events7d": 0},
            "antigravity": {"bytes7d": 0, "events7d": 0},
            "codex": {"bytes7d": 0, "events7d": 0},
            "dsh": {"bytes7d": 0, "events7d": 0},
        }});
        assert_eq!(read_stats(1)?, empty);

        let stdout = format!("\n  {}\n", "O".repeat(24_000));
        let output = |id| {
            json!({
                "hook_event_name": "PostToolUse", "session_id": "provider-session", "tool_use_id": id,
                "tool_name": "Bash", "tool_input": {"command": "fixture"},
                "tool_response": {"stdout": stdout, "stderr": "", "isImage": false, "interrupted": false},
            })
        };
        // An expired decision returns the allow response. An evaluated result
        // dropped before delivery may store its log, but must not add savings.
        let timed_out = evaluate_hook_fail_open(
            &server.interceptor,
            &data_root,
            LaunchProfile::ClaudeCode,
            "session",
            &output("timeout"),
            Duration::ZERO,
        );
        assert_eq!(timed_out, HookEvaluation::allow());
        server.interceptor.record_delivered_savings(
            data_root.clone(),
            "session",
            timed_out,
            SystemTime::now(),
        );
        let discarded = server.interceptor.evaluate_hook(
            &data_root,
            LaunchProfile::ClaudeCode,
            "session",
            &output("discarded"),
            Instant::now() + Duration::from_secs(2),
        );
        assert!(discarded.saved_noise_bytes > 0);
        drop(discarded);
        assert_eq!(claude_hook_reply(&server, &run, "{")?, "{}");
        assert_eq!(read_stats(2)?, empty);

        let text = "plain é line\n".repeat(600);
        let read_path = root.path().join("fixture.txt");
        fs::write(&read_path, &text)?;
        let read = |id| {
            json!({
                "hook_event_name": "PreToolUse", "tool_name": "Read", "tool_use_id": id,
                "tool_input": {"file_path": read_path},
            })
            .to_string()
        };
        assert_eq!(claude_hook_reply(&server, &run, &read("first"))?, "{}");
        let repeated = read("repeated");
        assert!(
            claude_hook_reply(&server, &run, &repeated)?
                .contains("\"permissionDecision\":\"deny\"")
        );
        assert_eq!(claude_hook_reply(&server, &run, &repeated)?, "{}");
        assert!(
            claude_hook_reply(&server, &run, &repeated)?
                .contains("\"permissionDecision\":\"deny\"")
        );

        let mut expected = empty.clone();
        // Read dedup uses the disk slice estimate, without the host's line labels.
        expected["agents"]["claude-code"] = json!({"bytes7d": text.len(), "events7d": 1});
        assert_eq!(read_stats(3)?, expected);

        let delivered = output("delivered").to_string();
        let response = claude_hook_reply(&server, &run, &delivered)?;
        let decision: Value = serde_json::from_str(&response)?;
        let summary = decision["hookSpecificOutput"]["updatedToolOutput"]["stdout"]
            .as_str()
            .context("replacement for Bash")?;
        // Output replacement measures the rendered body after host whitespace handling.
        let actual_saved = stdout.trim().len() - summary.trim().len();
        assert_ne!(actual_saved, stdout.len() - summary.len());
        assert_eq!(claude_hook_reply(&server, &run, &delivered)?, response);
        expected["agents"]["claude-code"] =
            json!({"bytes7d": text.len() + actual_saved, "events7d": 2});
        assert_eq!(read_stats(4)?, expected);

        config.set_agent_settings("codex", settings.clone())?;
        let codex = server.register(
            &data_root,
            Scope {
                profile: LaunchProfile::Codex,
                ..scope("stats-codex")
            },
        )?;
        let appended = claude_hook_reply(&server, &codex, &output("appended").to_string())?;
        assert!(appended.contains("\"additionalContext\""));
        assert_eq!(read_stats(5)?, expected);

        config.set_agent_settings(
            "claude-code",
            AgentOptimizerSettings {
                enabled: false,
                ..settings.clone()
            },
        )?;
        assert_eq!(claude_hook_reply(&server, &run, &read("disabled"))?, "{}");
        assert_eq!(
            claude_hook_reply(&server, &run, &output("disabled").to_string())?,
            "{}"
        );
        assert_eq!(read_stats(6)?, expected);

        config.set_agent_settings("claude-code", settings)?;
        assert_eq!(
            claude_hook_reply(&server, &run, &read("before-lock"))?,
            "{}"
        );
        let conn = rusqlite::Connection::open(data_root.join("context_guard.sqlite"))?;
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let response = claude_hook_reply(&server, &run, &read("locked-write"))?;
        assert!(response.contains("\"permissionDecision\":\"deny\""));
        // Keep the DB locked until the worker has attempted the queued write.
        assert_eq!(read_stats(7)?, empty);
        conn.execute_batch("ROLLBACK")?;
        assert_eq!(read_stats(8)?, empty);
        drop(server);
        runtime.shutdown()?;
        Ok(())
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
            let encoded = command
                .strip_prefix("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand ")
                .context("shell-neutral Windows callback")?;
            let bytes = STANDARD.decode(encoded)?;
            let script = String::from_utf16(
                &bytes
                    .chunks_exact(2)
                    .map(|word| u16::from_le_bytes([word[0], word[1]]))
                    .collect::<Vec<_>>(),
            )?;
            assert!(script.contains(&format!(r#"--data-binary "@-" "{url}""#)));
            assert!(script.contains("--fail --noproxy \"*\" --max-time 2"));
            assert!(!script.contains("RedirectStandardInput"));
            assert!(!script.contains("--output NUL"));
        }
        assert!(windows_notification_command(endpoint).contains("--output NUL"));
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
    fn bundled_foreman_script_and_composed_instructions_work() -> Result<()> {
        let root = tempfile::tempdir()?;
        let explicit = root.path().join("explicit assets");
        let explicit_script =
            explicit.join("providers/claude-plugin/skills/agent-foreman/scripts/foreman.mjs");
        fs::create_dir_all(explicit_script.parent().unwrap())?;
        fs::write(&explicit_script, "// foreman")?;
        assert_eq!(
            bundled_foreman_script(None, Some(&explicit)),
            Some(explicit_script.clone()),
        );
        for (executable, resources) in [
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
            let script = resources.join(
                "control-assets/providers/claude-plugin/skills/agent-foreman/scripts/foreman.mjs",
            );
            fs::create_dir_all(script.parent().unwrap())?;
            fs::write(&script, "// bundled")?;
            assert_eq!(
                bundled_foreman_script(Some(&executable), None),
                Some(script),
            );
            assert_eq!(
                bundled_foreman_script(Some(&executable), Some(&explicit)),
                Some(explicit_script.clone()),
            );
        }

        let base = "Base host instructions.\n";
        assert_eq!(
            compose_managed_instructions(base, false, Some("roles"), None),
            base
        );
        let composed = compose_managed_instructions(base, true, Some("roles"), None);
        assert!(composed.starts_with("Base host instructions.\n\n"));
        assert!(composed.contains("mt-foreman guide"));
        assert!(composed.contains("MATOU_FOREMAN_DEPTH"));
        assert!(composed.ends_with("\n\nroles"));
        Ok(())
    }

    #[test]
    fn managed_system_messages_append_the_same_roles_snapshot() -> Result<()> {
        use matou_native_core::dispatch_roles::{DispatchRolesConfig, format_roles_markdown};

        let root = tempfile::tempdir()?;
        let server = ProviderHookServer::start()?;
        let mut agy_scope = scope("agy");
        agy_scope.profile = LaunchProfile::Antigravity;
        let mut agy = server.register(root.path(), agy_scope)?;
        let mut dsh_scope = scope("dsh");
        dsh_scope.profile = LaunchProfile::Dsh;
        let mut dsh = server.register(root.path(), dsh_scope)?;
        let mut config = DispatchRolesConfig::default();
        config.agents[2].role = "离线共用职责夹具".into();
        let expected = format_roles_markdown(&config);
        let agy_file = agy.attach_antigravity_instructions(true, Some(&expected))?;
        let dsh_patch = dsh.attach_dsh(
            "- id: dsh-tui\n  config:\n    model: offline\n",
            "dsh",
            true,
            Some(&expected),
        )?;
        let directory = dsh_patch.parent().unwrap();
        for message in [
            read_antigravity_instructions(&agy_file)?,
            fs::read_to_string(directory.join("dsh-system-message.md"))?,
        ] {
            assert!(message.contains(FOREMAN_CAPABILITY_INDEX));
            assert!(message.contains(&expected));
            assert!(message.ends_with(managed_cli_instructions(root.path(), "dsh").unwrap()));
        }
        Ok(())
    }

    fn read_antigravity_instructions(path: &Path) -> Result<String> {
        let payload: Value = serde_json::from_reader(File::open(path)?)?;
        let steps = payload["injectSteps"]
            .as_array()
            .context("injected steps")?;
        assert_eq!(steps.len(), 1);
        steps[0]["systemMessage"]["systemMessage"]
            .as_str()
            .map(str::to_owned)
            .context("injected system message")
    }

    #[test]
    fn managed_context_instructions_follow_each_provider_switch() -> Result<()> {
        use matou_native_core::context_optimizer_config::{
            AgentOptimizerSettings, ContextOptimizerConfig,
        };

        let root = tempfile::tempdir()?;
        let config = ContextOptimizerConfig::for_data_root(root.path());
        let server = ProviderHookServer::start()?;
        let agy_base =
            include_str!("../../../runtime/control-assets/providers/antigravity-system-message.md");
        let codex_base = include_str!(
            "../../../runtime/control-assets/providers/codex-developer-instructions.md"
        );
        let dsh_base =
            include_str!("../../../runtime/control-assets/providers/dsh-system-message.md");
        for (enabled, cli_tools) in [(true, true), (true, false), (false, true)] {
            for agent in ["antigravity", "codex", "dsh"] {
                config.set_agent_settings(
                    agent,
                    AgentOptimizerSettings {
                        enabled,
                        cli_tools,
                        ..Default::default()
                    },
                )?;
            }
            let register = |profile| {
                server.register(
                    root.path(),
                    Scope {
                        profile,
                        ..scope("instructions")
                    },
                )
            };
            let mut agy = register(LaunchProfile::Antigravity)?;
            let mut dsh = register(LaunchProfile::Dsh)?;
            let codex = register(LaunchProfile::Codex)?;
            let agy_message =
                read_antigravity_instructions(&agy.attach_antigravity_instructions(false, None)?)?;
            let dsh_patch = dsh.attach_dsh(
                "- id: dsh-tui\n  config:\n    model: offline\n",
                "dsh",
                false,
                None,
            )?;
            let dsh_message =
                fs::read_to_string(dsh_patch.with_file_name("dsh-system-message.md"))?;
            let mut launch = ShellLaunch {
                profile: LaunchProfile::Codex,
                executable: PathBuf::from("codex"),
                argv: Vec::new(),
                cwd: root.path().to_path_buf(),
                data_root: root.path().to_path_buf(),
                inherited_env: Default::default(),
                env: Default::default(),
                cols: 80,
                rows: 24,
            };
            codex.attach_codex(&mut launch, false, None)?;
            let codex_message: String = serde_json::from_str(
                launch.argv[1]
                    .strip_prefix("developer_instructions=")
                    .unwrap(),
            )?;
            assert!(launch.argv[3].contains("PostCompact = ["));
            for (agent, message, base) in [
                ("antigravity", agy_message, agy_base),
                ("codex", codex_message, codex_base),
                ("dsh", dsh_message, dsh_base),
            ] {
                if enabled && cli_tools {
                    let note = managed_cli_instructions(root.path(), agent).unwrap();
                    assert!(note.contains("mt ctx outline"));
                    assert_eq!(
                        note.lines().filter(|line| line.starts_with("- ")).count(),
                        4
                    );
                    assert_eq!(message, format!("{}\n\n{note}", base.trim_end()));
                } else {
                    assert_eq!(message, base);
                }
            }
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
        assert_eq!(config["hooks"].as_object().unwrap().len(), EVENTS.len());
        for (event, timeout) in [
            ("SessionEnd", 3),
            ("UserPromptSubmit", 5),
            ("PreToolUse", 10),
            ("PostToolUse", 10),
            ("PostToolUseFailure", 5),
            ("PostCompact", 5),
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
                synchronous_callback_script(&format!("{url}?kind=statusline"))
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
            assert_eq!(request(&server, &run.token, "POST", body)?, 200);
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
            observation.notification.unwrap().kind,
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
                    .map(|event| event.kind),
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
            let event = parsed.notification.unwrap();
            assert_eq!(event.kind, expected, "{payload}");
            assert_eq!(event.payload()["subtitle"], expected.labels().1);
            assert_eq!(event.payload()["cooldownKey"], "Notification");
        }
    }

    fn notification(payload: Value) -> Value {
        Observation::parse(payload.as_object().unwrap())
            .notification
            .unwrap()
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
