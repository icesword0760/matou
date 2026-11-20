//! Prepare a fresh Codex TUI for the owner to launch on an existing card.
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use matou_native_core::{
    provider_launch::{EFFORTS, validate_cwd},
    pty::{LaunchProfile, ShellLaunch},
};
use serde_json::{Value, json};

use crate::shell_input::CODEX_BYPASS;

pub(crate) const STARTUP_GRACE: Duration = Duration::from_millis(150);

pub(crate) struct CodexLaunch {
    pub launch: ShellLaunch,
    pub provider_config_id: String,
    pub model: String,
    pub permission_mode: &'static str,
}

pub(crate) fn first_screen_ready(elapsed: Duration, emitted_output: bool) -> bool {
    elapsed >= STARTUP_GRACE && emitted_output
}

#[derive(Debug)]
struct MissingCommand(String);

impl std::fmt::Display for MissingCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MissingCommand {}

pub(crate) fn failure_message(error: &anyhow::Error) -> String {
    if let Some(missing) = error.downcast_ref::<MissingCommand>() {
        return missing.to_string();
    }
    let reason = error.root_cause().to_string();
    if std::env::var("MATOU_LOCALE").as_deref() == Ok("en") {
        format!("Codex failed to start: {reason}")
    } else {
        format!("Codex 启动失败：{reason}")
    }
}

pub(crate) fn prepare(
    data_root: &Path,
    cwd: &Path,
    permission_mode: &'static str,
    mut environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<CodexLaunch> {
    validate_cwd(cwd)?;
    let command = environment
        .get(OsStr::new("MATOU_CODEX_COMMAND"))
        .filter(|value| !value.is_empty())
        .map_or(OsStr::new("codex"), OsString::as_os_str);
    let path = Path::new(command);
    let cwd = std::path::absolute(cwd)?;
    let executable = if path.file_name() != Some(command) {
        executable_at(cwd.join(path))
    } else {
        environment
            .get(OsStr::new("PATH"))
            .into_iter()
            .flat_map(std::env::split_paths)
            .filter(|dir| !dir.as_os_str().is_empty())
            .find_map(|dir| executable_at(cwd.join(dir).join(path)))
    };
    let executable = executable.with_context(|| {
        // Show the command name in the prompt without exposing a local path.
        let command = path.file_name().unwrap_or(OsStr::new("codex")).to_string_lossy();
        MissingCommand(if environment.get(OsStr::new("MATOU_LOCALE")).is_some_and(|value| value == "en") {
            format!("Codex CLI ({command}) was not found. Install it and ensure `{command}` is on PATH, then try again.")
        } else {
            format!("未检测到 Codex（命令：{command}）。请先安装并确保 `{command}` 在 PATH 中，再重试。")
        })
    })?;

    let provider = active_provider(data_root)?;
    ensure!(
        provider["kind"] != "subscription",
        "Subscription launch service is unavailable"
    );
    let field = |key: &str| provider[key].as_str().unwrap_or_default().trim();
    let model = field("model");
    let mut argv = vec!["-c".into(), "check_for_update_on_startup=false".into()];
    let effort = field("reasoningEffort");
    if effort != "follow" && EFFORTS.contains(&effort) {
        argv.extend([
            "-c".into(),
            format!("model_reasoning_effort={}", json!(effort)),
        ]);
    }
    if !model.is_empty() {
        argv.extend(["--model".into(), model.into()]);
    }
    match permission_mode {
        "default" => {
            let (approval, sandbox) = default_permissions(&environment);
            argv.extend([
                "--ask-for-approval".into(),
                approval.into(),
                "--sandbox".into(),
                sandbox.into(),
            ]);
        }
        "bypassPermissions" => argv.push(CODEX_BYPASS.into()),
        _ => bail!("Unsupported Codex launch permission mode"),
    }
    environment.insert("TERM".into(), "xterm-256color".into());
    for (name, value) in [
        ("OPENAI_API_KEY", field("apiKey")),
        ("OPENAI_BASE_URL", field("endpoint").trim_end_matches('/')),
    ] {
        if !value.is_empty() {
            environment.insert(name.into(), value.into());
        }
    }
    Ok(CodexLaunch {
        launch: ShellLaunch {
            profile: LaunchProfile::Codex,
            executable,
            argv,
            cwd,
            data_root: data_root.to_owned(),
            inherited_env: BTreeMap::new(),
            env: environment,
            cols,
            rows,
        },
        provider_config_id: field("id").to_owned(),
        model: model.to_owned(),
        permission_mode,
    })
}

fn active_provider(data_root: &Path) -> Result<Value> {
    let state: Value = match fs::read(data_root.join("provider-config.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid provider configuration JSON"))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(error).context("Read provider configuration"),
    };
    if state["version"] == 1
        && let Some(providers) = state["providers"]["codex"].as_array()
        && let Some(active_ids) = state["activeProviderIds"].as_object()
        && let Some(first) = providers.first()
    {
        let selected = active_ids.get("codex").and_then(Value::as_str);
        let provider = providers
            .iter()
            .find(|entry| entry["id"].as_str().is_some_and(|id| Some(id) == selected))
            .unwrap_or(first);
        ensure!(
            provider["id"].is_string(),
            "Invalid Codex provider configuration"
        );
        return Ok(provider.clone());
    }
    Ok(json!({"id": "openai-official", "endpoint": "https://api.openai.com/v1"}))
}

fn executable_at(path: PathBuf) -> Option<PathBuf> {
    #[cfg(windows)]
    if path.extension().is_none() {
        return executable_at(path.with_extension("exe"));
    }
    if !path.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
        if unsafe { libc::access(name.as_ptr(), libc::X_OK) } != 0 {
            return None;
        }
    }
    Some(path)
}

const DEFAULT_PERMISSIONS: (&str, &str) = ("on-request", "workspace-write");

fn default_permissions(environment: &BTreeMap<OsString, OsString>) -> (&'static str, &'static str) {
    let root = environment
        .get(OsStr::new("CODEX_HOME"))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let home_key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
            environment
                .get(OsStr::new(home_key))
                .map(|home| Path::new(home).join(".codex"))
        });
    root.and_then(|root| fs::read_to_string(root.join("config.toml")).ok())
        .as_deref()
        .and_then(parse_default_permissions)
        .unwrap_or(DEFAULT_PERMISSIONS)
}

fn parse_default_permissions(text: &str) -> Option<(&'static str, &'static str)> {
    let mut selected = DEFAULT_PERMISSIONS;
    let mut seen = [false; 2];
    let mut remaining = text.trim();
    // TOML root keys appear before the first table. Scan strings and containers
    // so directives inside documentation or arrays cannot become permissions.
    while !remaining.is_empty() && !remaining.starts_with('[') {
        let (line, rest) = next_root_statement(remaining)?;
        remaining = rest.trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (index, target, supported): (_, &mut &'static str, &'static [&'static str]) =
            match key.trim().trim_matches(['"', '\'']) {
                "approval_policy" => (
                    0,
                    &mut selected.0,
                    &["on-request", "never", "untrusted", "on-failure"],
                ),
                "sandbox_mode" => (
                    1,
                    &mut selected.1,
                    &["read-only", "workspace-write", "danger-full-access"],
                ),
                _ => continue,
            };
        if std::mem::replace(&mut seen[index], true) {
            return None;
        }
        let value = value.trim();
        let parsed = if value.starts_with('"') {
            serde_json::from_str::<String>(value).ok()
        } else {
            value
                .strip_prefix('\'')
                .and_then(|text| text.strip_suffix('\''))
                .map(str::to_owned)
        };
        if let Some(value) = supported
            .iter()
            .find(|value| parsed.as_deref() == Some(**value))
        {
            *target = value;
        }
    }
    Some(selected)
}

fn next_root_statement(text: &str) -> Option<(&str, &str)> {
    let mut cursor = 0;
    let mut depth = 0usize;
    while let Some(&byte) = text.as_bytes().get(cursor) {
        match byte {
            b'"' | b'\'' => {
                let bytes = text.as_bytes();
                let width = if bytes.get(cursor..cursor + 3) == Some(&[byte; 3]) {
                    3
                } else {
                    1
                };
                let delimiter = &bytes[cursor..cursor + width];
                cursor += width;
                loop {
                    if bytes.get(cursor..cursor + width) == Some(delimiter) {
                        cursor += width;
                        break;
                    }
                    match *bytes.get(cursor)? {
                        b'\\' if byte == b'"' => cursor += 2,
                        b'\n' | b'\r' if width == 1 => return None,
                        _ => cursor += 1,
                    }
                }
                continue;
            }
            b'#' => {
                let newline = text[cursor..]
                    .find('\n')
                    .map_or(text.len(), |offset| cursor + offset);
                if depth == 0 {
                    return Some((&text[..cursor], &text[newline..]));
                }
                cursor = newline;
                continue;
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth = depth.checked_sub(1)?,
            b'\n' if depth == 0 => return Some((&text[..cursor], &text[cursor + 1..])),
            _ => {}
        }
        cursor += 1;
    }
    (depth == 0).then_some((text, ""))
}
