//! Prepare an Antigravity CLI launch and maintain Matou's lifecycle plugin.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, ensure};
#[cfg(unix)]
use matou_native_core::provider_launch::resolve_command_with_login_path;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::dsh_events::{js_whitespace, valid_session_id};

const COMMAND_OVERRIDE: &str = "MATOU_ANTIGRAVITY_COMMAND";
const PLUGIN_OVERRIDE: &str = "MATOU_ANTIGRAVITY_PLUGIN_DIR";
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
    let executable = if command.is_empty() {
        None
    } else if command_path.file_name() != Some(&command) {
        executable_at(cwd.join(command_path))
    } else {
        env_value(&environment, "PATH")
            .into_iter()
            .flat_map(std::env::split_paths)
            .filter(|dir| !dir.as_os_str().is_empty())
            .find_map(|dir| executable_at(cwd.join(dir).join(command_path)))
    };
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

#[cfg(not(unix))]
fn executable_at(path: PathBuf) -> Option<PathBuf> {
    #[cfg(windows)]
    if path.extension().is_none() {
        return executable_at(path.with_extension("exe"));
    }
    if !path.is_file() {
        return None;
    }
    Some(path)
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
    // The frozen plugin uses curl's environment to bypass inherited proxies
    // on loopback requests. Keep any existing exclusions.
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

/// Install only the five Matou-managed files. All files are replaced atomically,
/// and the completion stamp is written after the plugin content.
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
    for (relative, _, _) in
        PLUGIN_FILES
            .iter()
            .chain(std::iter::once(&(STAMP_PATH, PLUGIN_STAMP, 0o644)))
    {
        check_owned_path(&directory.join(relative), false)?;
    }
    if let Ok(bytes) = fs::read(&manifest)
        && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && let Some(name) = value["name"].as_str()
    {
        ensure!(name == "matou", "Antigravity plugin belongs to {name}");
    }

    if file_matches(&directory.join(STAMP_PATH), PLUGIN_STAMP, 0o644)
        && PLUGIN_FILES
            .iter()
            .all(|(path, contents, mode)| file_matches(&directory.join(path), contents, *mode))
    {
        return Ok(());
    }
    let mut directories = fs::DirBuilder::new();
    directories.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directories.mode(0o755);
    }
    directories.create(directory.join("hooks"))?;
    for (path, contents, mode) in PLUGIN_FILES {
        write_atomic(&directory.join(path), contents, mode)?;
    }
    write_atomic(&directory.join(STAMP_PATH), PLUGIN_STAMP, 0o644)
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

const HOOKS: &str = r#"{
  "matou": {
    "SessionStart": [
      {
        "command": "./hooks/session-start.sh"
      }
    ],
    "PreInvocation": [
      {
        "command": "./hooks/matou-hook.sh"
      }
    ],
    "PostInvocation": [
      {
        "command": "./hooks/matou-hook.sh"
      }
    ],
    "PreToolUse": [
      {
        "hooks": [
          {
            "command": "./hooks/matou-hook.sh pre-tool-use"
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "command": "./hooks/matou-hook.sh post-tool-use"
          }
        ]
      }
    ],
    "Stop": [
      {
        "command": "./hooks/matou-hook.sh"
      }
    ]
  }
}
"#;

const MATOU_HOOK: &str = r#"#!/bin/sh
[ -n "${MATOU_PROVIDER_HOOK_URL:-}" ] || exit 0
[ -n "${MATOU_PROVIDER_HOOK_TOKEN:-}" ] || exit 0
payload=$(cat; printf '.')
payload=${payload%.}
event=${1:-}
if [ -z "$event" ] && printf '%s' "$payload" | grep -Eq '"hookEventName"[[:space:]]*:[[:space:]]*"PreToolUse"'; then
  event=pre-tool-use
fi
url=$MATOU_PROVIDER_HOOK_URL
case "$event" in
  pre-tool-use|post-tool-use)
    case "$url" in
      *\?*) url="$url&kind=$event" ;;
      *) url="$url?kind=$event" ;;
    esac ;;
esac
if [ "$event" = pre-tool-use ]; then
  decision=$(printf '%s' "$payload" | /usr/bin/curl --disable --noproxy '*' --silent --fail --max-time 1 \
    --request POST --header 'content-type: application/json' \
    --data-binary @- "$url" 2>/dev/null && printf '.') || exit 0
  printf '%s' "${decision%.}"
  exit 0
fi
(
  printf '%s' "$payload" | /usr/bin/curl --disable --noproxy '*' --silent --max-time 2 \
    --request POST --header 'content-type: application/json' \
    --data-binary @- "$url" >/dev/null 2>&1
) </dev/null >/dev/null 2>&1 &
exit 0
"#;

const SESSION_START: &str = r#"#!/bin/sh
[ -n "${MATOU_PROVIDER_HOOK_URL:-}" ] || exit 0
[ -n "${MATOU_PROVIDER_HOOK_TOKEN:-}" ] || exit 0
payload=$(cat)
(
  printf '%s' "$payload" | /usr/bin/curl --silent --show-error --max-time 2 \
    --request POST --header 'content-type: application/json' \
    --data-binary @- "${MATOU_PROVIDER_HOOK_URL}" >/dev/null 2>&1
) </dev/null >/dev/null 2>&1 &
if [ -n "${MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE:-}" ] && [ -r "$MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE" ]; then
  cat "$MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE"
fi
exit 0
"#;

const PLUGIN_FILES: [(&str, &str, u32); 4] = [
    ("plugin.json", PLUGIN_MANIFEST, 0o644),
    ("hooks.json", HOOKS, 0o644),
    ("hooks/matou-hook.sh", MATOU_HOOK, 0o755),
    ("hooks/session-start.sh", SESSION_START, 0o755),
];
const PLUGIN_STAMP: &str =
    "1.0.0:5ece83e5428063a43b3c3d52c4afed7a0bb6a55d263b9afa4e73386f86399dd6\n";

#[cfg(test)]
mod tests {
    use super::*;

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
