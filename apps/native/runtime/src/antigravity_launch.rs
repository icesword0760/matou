//! Prepare an Antigravity CLI launch and maintain Matou's lifecycle plugin.

use std::{
    collections::{BTreeMap, HashMap},
    ffi::{OsStr, OsString},
    fs, io,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
#[cfg(unix)]
use matou_native_core::provider_launch::resolve_command_with_login_path;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::dsh_events::{js_whitespace, valid_session_id};

const COMMAND_OVERRIDE: &str = "MATOU_ANTIGRAVITY_COMMAND";
const PLUGIN_OVERRIDE: &str = "MATOU_ANTIGRAVITY_PLUGIN_DIR";
const STAGGER_OVERRIDE: &str = "MATOU_ANTIGRAVITY_STAGGER_MS";
const PHASE_PERIOD: Duration = Duration::from_secs(60);
const MAX_GAP_MS: u64 = 30_000;
const DEFAULT_GAP: Duration = if cfg!(target_os = "macos") {
    Duration::from_secs(6)
} else {
    Duration::ZERO
};
const STAMP_PATH: &str = ".matou-plugin-version";
pub(crate) const MODES: &[&str] = &[
    "default",
    "auto",
    "ask",
    "acceptEdits",
    "plan",
    "turbo",
    "bypassPermissions",
];
const RUN_VARIABLES: &[&str] = &[
    "MATOU_AGENT_STATUS_DIR",
    "MATOU_FOREMAN_ROLES_FILE",
    "MATOU_PROVIDER_HOOK_URL",
    "MATOU_PROVIDER_HOOK_TOKEN",
    crate::native_hooks::PROFILE_ENV,
    "MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE",
    "MATOU_CONTROL_ENDPOINT",
    "MATOU_CONTROL_TOKEN",
    "MATOU_CONTROL_PROTOCOL",
    "MATOU_CONTROL_CALLER_SESSION",
    "MATOU_CONTROL_CALLER_RUN",
    "MATOU_CONTROL_NODE_EXECUTABLE",
    "MATOU_CONTROL_ASSET_ROOT",
];
const CALLER_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_PID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_NAME",
    "CLAUDE_CODE_SESSION_KIND",
    "CLAUDE_BG_SOURCE",
    "CLAUDE_BG_BACKEND",
    "CLAUDE_BG_RENDEZVOUS_SOCK",
    "CLAUDE_CODE_RESUME_PROMPT",
    "CLAUDE_CODE_RESUME_INTERRUPTED_TURN",
    crate::dsh_launch::FALLBACK_CONTROL_TOKEN,
];

pub(crate) struct LaunchStagger {
    epoch: Instant,
    min_gap: Duration,
    launches: HashMap<String, Instant>,
}

impl LaunchStagger {
    fn new(epoch: Instant, min_gap: Duration) -> Self {
        Self {
            epoch,
            min_gap,
            launches: HashMap::new(),
        }
    }

    pub(crate) fn from_environment(environment: &BTreeMap<OsString, OsString>) -> Self {
        let min_gap = env_value(environment, STAGGER_OVERRIDE)
            .and_then(OsStr::to_str)
            .and_then(|value| {
                let digits = value.strip_prefix('+').unwrap_or(value);
                if digits.is_empty() {
                    return None;
                }
                digits.chars().try_fold(0, |ms, digit| {
                    digit
                        .to_digit(10)
                        .map(|digit| (ms * 10 + u64::from(digit)).min(MAX_GAP_MS))
                })
            })
            .map_or(DEFAULT_GAP, Duration::from_millis);
        Self::new(Instant::now(), min_gap)
    }

    fn phase(&self, time: Instant) -> Duration {
        let elapsed = time.saturating_duration_since(self.epoch);
        Duration::from_nanos((elapsed.as_nanos() % PHASE_PERIOD.as_nanos()) as u64)
    }

    pub(crate) fn is_ready(&self, session_id: &str, now: Instant) -> bool {
        if self.min_gap.is_zero() {
            return true;
        }
        let phase = self.phase(now);
        let mut phases: Vec<_> = self
            .launches
            .iter()
            .filter(|(id, _)| *id != session_id)
            .map(|(_, &launched_at)| self.phase(launched_at))
            .collect();
        let Some(nearest) = phases
            .iter()
            .map(|&other| {
                let direct = phase.abs_diff(other);
                direct.min(PHASE_PERIOD - direct)
            })
            .min()
        else {
            return true;
        };
        if nearest >= self.min_gap {
            return true;
        }

        phases.sort_unstable();
        let wrap_gap = PHASE_PERIOD - phases[phases.len() - 1] + phases[0];
        let max_gap = phases
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .fold(wrap_gap, Duration::max);
        // Keep at least a 500 ms window around the midpoint for the host loop to hit.
        let effective_gap = if max_gap < Duration::from_secs(1) {
            Duration::ZERO
        } else {
            self.min_gap
                .min((max_gap / 2).saturating_sub(Duration::from_millis(250)))
        };
        nearest >= effective_gap
    }

    pub(crate) fn record_launch(&mut self, session_id: String, launched_at: Instant) {
        self.launches.insert(session_id, launched_at);
    }

    pub(crate) fn remove(&mut self, session_id: &str) {
        self.launches.remove(session_id);
    }

    pub(crate) fn retain_active(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.launches.retain(|id, _| keep(id));
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    #[default]
    Default,
    Auto,
    Ask,
    AcceptEdits,
    Plan,
    Turbo,
    BypassPermissions,
}

impl PermissionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Auto => "auto",
            Self::Ask => "ask",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::Turbo => "turbo",
            Self::BypassPermissions => "bypassPermissions",
        }
    }

    pub(crate) fn from_saved(mode: &str) -> Self {
        match mode {
            "auto" => Self::Auto,
            "ask" => Self::Ask,
            "acceptEdits" => Self::AcceptEdits,
            "plan" => Self::Plan,
            "turbo" => Self::Turbo,
            "bypassPermissions" => Self::BypassPermissions,
            _ => Self::Default,
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            Self::Default | Self::Ask => &[],
            Self::Auto => &["--mode", "auto"],
            Self::AcceptEdits => &["--mode", "accept-edits"],
            Self::Plan => &["--mode", "plan"],
            Self::Turbo | Self::BypassPermissions => &["--dangerously-skip-permissions"],
        }
    }

    fn settings_overrides(self) -> Option<Map<String, Value>> {
        let tool_permission = match self {
            Self::Default | Self::BypassPermissions => return None,
            Self::Turbo => "always-proceed",
            Self::Ask | Self::Auto | Self::AcceptEdits | Self::Plan => "request-review",
        };
        let mut settings = Map::from_iter([
            ("toolPermission".into(), Value::from(tool_permission)),
            (
                "allowNonWorkspaceAccess".into(),
                Value::from(self == Self::Turbo),
            ),
        ]);
        if self == Self::Ask {
            // The empty agent mode selects the CLI's default execution mode.
            settings.insert("agentMode".into(), Value::from(""));
        }
        if self == Self::Turbo {
            // Auto can enable sandboxing, and the default execution mode can request diff review.
            settings.insert("agentMode".into(), Value::from("accept-edits"));
            settings.insert("enableTerminalSandbox".into(), Value::from(false));
        }
        Some(settings)
    }
}

/// Recognize the same interactive launches as Shell-panel promotion.
pub fn interactive_permission(command: &str) -> Option<PermissionMode> {
    let mut tokens = command
        .split(js_whitespace)
        .filter(|token| !token.is_empty());
    if tokens.next() != Some("agy") {
        return None;
    }
    let mut permission = PermissionMode::Default;
    let mut skips_tool_review = false;
    while let Some(token) = tokens.next() {
        if token == "--dangerously-skip-permissions" {
            skips_tool_review = true;
            continue;
        }
        let mode = if token == "--mode" {
            tokens.next()?
        } else {
            token.strip_prefix("--mode=")?
        };
        permission = match mode {
            "auto" => PermissionMode::Default,
            "accept-edits" => PermissionMode::AcceptEdits,
            "plan" => PermissionMode::Plan,
            _ => return None,
        };
    }
    Some(if skips_tool_review {
        PermissionMode::BypassPermissions
    } else {
        permission
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LaunchMode {
    #[default]
    Fresh,
    Resume(String),
    Fork(String),
}

impl LaunchMode {
    pub fn conversation_id(&self) -> Option<&str> {
        match self {
            Self::Fresh => None,
            Self::Resume(id) | Self::Fork(id) => Some(id),
        }
    }
}

pub struct LaunchInput<'a> {
    pub cwd: &'a Path,
    pub settings_path: Option<&'a Path>,
    pub model: Option<&'a str>,
    pub permission: PermissionMode,
    pub mode: LaunchMode,
    pub inherited_environment: BTreeMap<OsString, OsString>,
    pub run_environment: BTreeMap<OsString, OsString>,
}

pub struct LaunchPlan {
    pub executable: PathBuf,
    pub argv: Vec<OsString>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<OsString, OsString>,
    pub mode: LaunchMode,
}

pub(crate) fn valid_conversation_id(id: &str) -> bool {
    id.len() <= 160 && valid_session_id(id)
}

fn env_key_eq(key: &OsStr, name: &str) -> bool {
    if cfg!(windows) {
        key.to_str()
            .is_some_and(|key| key.eq_ignore_ascii_case(name))
    } else {
        key == OsStr::new(name)
    }
}

pub(crate) fn env_value<'a>(
    environment: &'a BTreeMap<OsString, OsString>,
    name: &str,
) -> Option<&'a OsStr> {
    environment
        .iter()
        .find_map(|(key, value)| env_key_eq(key, name).then_some(value.as_os_str()))
}

fn set_env(environment: &mut BTreeMap<OsString, OsString>, key: OsString, value: OsString) {
    if cfg!(windows)
        && let Some(name) = key.to_str()
    {
        environment.retain(|old, _| !env_key_eq(old, name));
    }
    environment.insert(key, value);
}

/// Prepare the launch, probing the login-shell PATH if the CLI is missing.
pub fn prepare(input: LaunchInput<'_>) -> Result<LaunchPlan> {
    let mut mode = input.mode;
    if let LaunchMode::Resume(id) | LaunchMode::Fork(id) = &mut mode {
        *id = id.trim_matches(js_whitespace).to_owned();
        ensure!(valid_conversation_id(id), "Invalid Antigravity session ID");
    }
    ensure!(
        !input.cwd.as_os_str().is_empty(),
        "Antigravity working directory is missing"
    );
    let cwd = std::path::absolute(input.cwd)?;
    ensure!(
        cwd.is_dir(),
        "Antigravity working directory is unavailable: {}",
        cwd.display()
    );
    let command = env_value(&input.inherited_environment, COMMAND_OVERRIDE)
        .unwrap_or(OsStr::new("agy"))
        .to_owned();

    let mut environment = input.inherited_environment;
    environment.retain(|key, _| !RUN_VARIABLES.iter().any(|name| env_key_eq(key, name)));
    for (key, value) in input.run_environment {
        set_env(&mut environment, key, value);
    }
    environment.retain(|key, _| !CALLER_MARKERS.iter().any(|name| env_key_eq(key, name)));
    set_env(&mut environment, "TERM".into(), "xterm-256color".into());
    #[cfg(unix)]
    environment.insert("PWD".into(), cwd.clone().into_os_string());

    let mut argv = Vec::new();
    let settings_path = input
        .settings_path
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| checked_path(&cwd, path))
        .transpose()?;
    if let Some(settings) = settings_argument(settings_path, input.permission)? {
        argv.extend([OsString::from("--settings"), settings]);
    }
    if let Some(path) = env_value(&environment, "MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE")
        .filter(|path| !path.is_empty())
    {
        let path = checked_path(&cwd, Path::new(path))?;
        set_env(
            &mut environment,
            "MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE".into(),
            path.into_os_string(),
        );
    }
    if let Some(model) = input
        .model
        .map(|model| model.trim_matches(js_whitespace))
        .filter(|model| !model.is_empty())
    {
        ensure!(
            !model.starts_with('-') && !model.contains('\0'),
            "Invalid Antigravity model"
        );
        argv.extend([OsString::from("--model"), model.into()]);
    }
    if let Some(id) = mode.conversation_id() {
        argv.extend([OsString::from("--conversation"), id.into()]);
    }
    argv.extend(input.permission.args().iter().map(OsString::from));

    let command_path = Path::new(&command);
    #[cfg(unix)]
    let executable = resolve_command_with_login_path(command_path, &cwd, &mut environment).ok();
    #[cfg(not(unix))]
    let executable = crate::codex_launch::resolve_executable(command_path, &cwd, &environment);
    let executable = executable.with_context(|| {
        let command = command.to_string_lossy();
        if env_value(&environment, "MATOU_LOCALE") == Some(OsStr::new("en")) {
            format!(
                "Antigravity CLI ({command}) was not found. Install it and ensure it is on PATH."
            )
        } else {
            format!("未检测到 Antigravity CLI（命令：{command}）。请先安装并确保它在 PATH 中。")
        }
    })?;

    Ok(LaunchPlan {
        executable,
        argv,
        cwd,
        environment,
        mode,
    })
}

fn settings_argument(
    path: Option<PathBuf>,
    permission: PermissionMode,
) -> Result<Option<OsString>> {
    let Some(overrides) = permission.settings_overrides() else {
        return Ok(path.map(PathBuf::into_os_string));
    };
    let mut settings = match path {
        Some(path) => {
            let bytes = fs::read(&path).with_context(|| {
                format!("Could not read Antigravity settings: {}", path.display())
            })?;
            serde_json::from_slice::<Map<String, Value>>(&bytes)
                .with_context(|| format!("Invalid Antigravity settings: {}", path.display()))?
        }
        None => Map::new(),
    };
    settings.extend(overrides);
    Ok(Some(serde_json::to_string(&settings)?.into()))
}

fn checked_path(cwd: &Path, path: &Path) -> Result<PathBuf> {
    let path = cwd.join(path);
    ensure!(
        path.is_absolute() && !path.as_os_str().as_encoded_bytes().contains(&0),
        "Invalid Antigravity launch path"
    );
    Ok(path)
}

pub fn plugin_directory(home: &Path, environment: &BTreeMap<OsString, OsString>) -> PathBuf {
    if let Some(path) = env_value(environment, PLUGIN_OVERRIDE)
        .and_then(OsStr::to_str)
        .map(|path| path.trim_matches(js_whitespace))
        .filter(|path| !path.is_empty())
    {
        return path.into();
    }
    home.join(".gemini/config/plugins/matou")
}

pub(crate) fn prepare_environment(
    shell_home: &Path,
    environment: &mut BTreeMap<OsString, OsString>,
) -> Result<()> {
    check_owned_path(shell_home, true)?;
    let home = shell_home.canonicalize()?;
    let plugin = plugin_directory(&home, environment);
    let plugin = match plugin.strip_prefix(shell_home) {
        Ok(relative) => home.join(relative),
        Err(_) => plugin,
    };
    ensure_plugin_installed(&plugin)?;
    set_env(
        environment,
        "HOME".into(),
        shell_home.as_os_str().to_owned(),
    );
    #[cfg(windows)]
    set_env(
        environment,
        "USERPROFILE".into(),
        shell_home.as_os_str().to_owned(),
    );
    environment.retain(|key, _| !env_key_eq(key, PLUGIN_OVERRIDE));
    // Preserve loopback exclusions for the agent and its tools.
    let no_proxy = env_value(environment, "no_proxy")
        .or_else(|| env_value(environment, "NO_PROXY"))
        .unwrap_or_default()
        .to_string_lossy();
    let no_proxy = if no_proxy.is_empty() {
        "127.0.0.1".to_owned()
    } else {
        format!("127.0.0.1,{no_proxy}")
    };
    set_env(environment, "NO_PROXY".into(), no_proxy.clone().into());
    #[cfg(unix)]
    set_env(environment, "no_proxy".into(), no_proxy.into());
    Ok(())
}

/// Install the managed manifests atomically and publish the completion stamp last.
pub fn ensure_plugin_installed(directory: &Path) -> Result<()> {
    static INSTALL: Mutex<()> = Mutex::new(());
    let _guard = INSTALL.lock().unwrap_or_else(|error| error.into_inner());
    ensure!(
        directory.is_absolute()
            && directory.file_name().is_some()
            && !directory
                .components()
                .any(|part| part == Component::ParentDir),
        "Invalid Antigravity plugin directory"
    );
    for path in directory.ancestors().filter(|path| path.is_absolute()) {
        check_owned_path(path, true)?;
    }
    check_owned_path(&directory.join("hooks"), true)?;
    let manifest = directory.join("plugin.json");
    let hooks = directory.join("hooks.json");
    let stamp = directory.join(STAMP_PATH);
    for path in [&manifest, &hooks, &stamp] {
        check_owned_path(path, false)?;
    }
    // Continue to reject symlink replacements of previously managed files.
    for name in ["hooks/matou-hook.sh", "hooks/session-start.sh"] {
        check_owned_path(&directory.join(name), false)?;
    }
    if let Ok(bytes) = fs::read(&manifest)
        && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && let Some(name) = value["name"].as_str()
    {
        ensure!(name == "matou", "Antigravity plugin belongs to {name}");
    }
    let hooks_json = plugin_hooks(&std::env::current_exe()?)?;
    let files = [
        (&manifest, PLUGIN_MANIFEST),
        (&hooks, hooks_json.as_str()),
        (&stamp, PLUGIN_STAMP),
    ];
    if files.iter().all(|(path, content)| file_matches(path, content, 0o644)) {
        return Ok(());
    }
    let mut directories = fs::DirBuilder::new();
    directories.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directories.mode(0o755);
    }
    directories.create(directory)?;
    for (path, contents) in files {
        write_atomic(path, contents, 0o644)?;
    }
    Ok(())
}

fn plugin_hooks(runtime: &Path) -> Result<String> {
    let hook = |event| -> Result<Value> {
        Ok(json!({"command": crate::native_hooks::antigravity_command(runtime, event)?}))
    };
    Ok(serde_json::to_string_pretty(&json!({"matou": {
        "SessionStart": [hook("session-start")?],
        "PreInvocation": [hook("observe")?],
        "PostInvocation": [hook("observe")?],
        "PreToolUse": [{"hooks": [hook("pre-tool-use")?]}],
        "PostToolUse": [{"hooks": [hook("post-tool-use")?]}],
        "Stop": [hook("observe")?]
    }}))?)
}

fn check_owned_path(path: &Path, directory: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            if directory {
                metadata.is_dir()
            } else {
                metadata.is_file()
            },
            "Unsafe Antigravity plugin path: {}",
            path.display()
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn file_matches(path: &Path, contents: &str, mode: u32) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.len() == contents.len() as u64)
        && fs::read(path).is_ok_and(|bytes| bytes == contents.as_bytes())
        && mode_matches(path, mode)
}

#[cfg(unix)]
fn mode_matches(path: &Path, mode: u32) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o777 == mode)
}

#[cfg(not(unix))]
fn mode_matches(_path: &Path, _mode: u32) -> bool {
    true
}

fn write_atomic(path: &Path, contents: &str, _mode: u32) -> Result<()> {
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().expect("plugin file has a parent"))?;
    temp.write_all(contents.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(_mode))?;
    }
    temp.persist(path)?;
    Ok(())
}

// The stamp fingerprints the native plugin files, including their modes.
const PLUGIN_MANIFEST: &str = r#"{
  "name": "matou",
  "version": "1.0.0",
  "description": "Matou lifecycle integration for Antigravity CLI"
}
"#;

const PLUGIN_STAMP: &str = "2.0.0:native-hooks\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_stagger_starts_immediately_when_alone_or_at_a_safe_phase() {
        let epoch = Instant::now();
        let mut stagger = LaunchStagger::new(epoch, Duration::from_secs(6));
        let at = |seconds| epoch + Duration::from_secs(seconds);
        assert!(stagger.is_ready("first", epoch));
        stagger.record_launch("first".into(), epoch);
        assert!(!stagger.is_ready("second", epoch));
        assert!(!stagger.is_ready("second", at(5)));
        assert!(stagger.is_ready("second", at(6)));
        assert!(stagger.is_ready("second", at(54)));
        assert!(!stagger.is_ready("second", at(55)));
        assert!(!stagger.is_ready("second", at(60)));
    }

    #[test]
    fn launch_stagger_spaces_a_batch_across_the_period_boundary() {
        let epoch = Instant::now();
        let mut stagger = LaunchStagger::new(epoch, Duration::from_secs(6));
        let at = |seconds| epoch + Duration::from_secs(seconds);
        for (id, seconds) in [("a", 50), ("b", 56), ("c", 62), ("d", 68)] {
            if id != "a" {
                assert!(!stagger.is_ready(id, at(seconds - 1)));
            }
            assert!(stagger.is_ready(id, at(seconds)));
            stagger.record_launch(id.into(), at(seconds));
        }
        assert!(!stagger.is_ready("e", at(120)));
        assert!(stagger.is_ready("e", at(134)));
    }

    #[test]
    fn launch_stagger_releases_inactive_cards_and_replaces_a_restarted_card() {
        let epoch = Instant::now();
        let mut stagger = LaunchStagger::new(epoch, Duration::from_secs(6));
        let restart = epoch + Duration::from_secs(15);
        stagger.record_launch("a".into(), epoch);
        assert!(stagger.is_ready("a", epoch));
        stagger.record_launch("a".into(), restart);
        assert!(stagger.is_ready("b", epoch + PHASE_PERIOD));
        assert!(!stagger.is_ready("b", restart));
        stagger.remove("a");
        assert!(stagger.is_ready("b", restart));

        stagger.record_launch("a".into(), restart);
        stagger.record_launch("b".into(), restart + Duration::from_secs(10));
        stagger.retain_active(|id| id == "b");
        assert!(stagger.is_ready("c", restart));
        assert!(!stagger.is_ready("c", restart + Duration::from_secs(10)));
    }

    #[test]
    fn launch_stagger_opens_a_window_at_the_widest_gap_when_the_ring_is_full() {
        let epoch = Instant::now();
        let at = |ms| epoch + Duration::from_millis(ms);
        for (count, step) in [(10, 6_000), (12, 5_000)] {
            let mut stagger = LaunchStagger::new(epoch, Duration::from_secs(6));
            for index in 0..count {
                stagger.record_launch(index.to_string(), at(index * step));
            }
            // Verify both edges of the window across the wrap back to phase zero.
            let midpoint = 60_000 - step / 2;
            assert!(!stagger.is_ready("new", at(midpoint - 251)));
            assert!(stagger.is_ready("new", at(midpoint - 250)));
            assert!(stagger.is_ready("new", at(midpoint)));
            assert!(stagger.is_ready("new", at(midpoint + 250)));
            assert!(!stagger.is_ready("new", at(midpoint + 251)));
        }

        let mut stagger = LaunchStagger::new(epoch, Duration::from_secs(6));
        for index in 0..65 {
            stagger.record_launch(index.to_string(), at(index * 60_000 / 65));
        }
        assert!(stagger.is_ready("new", at(60_000)));
    }

    #[test]
    fn launch_stagger_keeps_a_window_when_the_gap_is_half_the_period() {
        let epoch = Instant::now();
        let mut stagger = LaunchStagger::new(epoch, Duration::from_secs(30));
        let at = |ms| epoch + Duration::from_millis(ms);
        stagger.record_launch("a".into(), epoch);
        assert!(!stagger.is_ready("b", at(29_749)));
        assert!(stagger.is_ready("b", at(29_750)));
        assert!(stagger.is_ready("b", at(30_000)));
        assert!(stagger.is_ready("b", at(30_250)));
        assert!(!stagger.is_ready("b", at(30_251)));
    }

    #[test]
    fn launch_stagger_parses_the_launch_environment() {
        assert_eq!(
            LaunchStagger::from_environment(&BTreeMap::new()).min_gap,
            DEFAULT_GAP
        );
        for (value, expected) in [
            ("0", Duration::ZERO),
            ("350", Duration::from_millis(350)),
            ("30000", Duration::from_secs(30)),
            ("30001", Duration::from_secs(30)),
            ("18446744073709551616", Duration::from_secs(30)),
            ("-1", DEFAULT_GAP),
            ("1.5", DEFAULT_GAP),
            ("", DEFAULT_GAP),
            ("invalid", DEFAULT_GAP),
        ] {
            let stagger =
                LaunchStagger::from_environment(&[(STAGGER_OVERRIDE.into(), value.into())].into());
            assert_eq!(stagger.min_gap, expected, "{value:?}");
            if expected.is_zero() {
                let mut stagger = stagger;
                stagger.record_launch("a".into(), stagger.epoch);
                assert!(stagger.is_ready("b", stagger.epoch));
            }
        }
    }

    #[test]
    fn prepare_environment_honors_the_plugin_directory_override() -> Result<()> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("shell-home");
        fs::create_dir(&home)?;
        let plugin = home.join("alternate-plugins/matou");
        let mut environment = BTreeMap::from([
            ("HOME".into(), root.path().as_os_str().to_owned()),
            (PLUGIN_OVERRIDE.into(), plugin.clone().into_os_string()),
        ]);

        prepare_environment(&home, &mut environment)?;

        assert_eq!(env_value(&environment, "HOME"), Some(home.as_os_str()));
        assert!(plugin.join("plugin.json").is_file());
        assert!(!home.join(".gemini").exists());
        Ok(())
    }
}
