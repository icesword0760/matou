//! Prepare a fresh Codex TUI for the owner to launch on an existing card.
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use matou_native_core::{
    provider_launch::{EFFORTS, validate_cwd},
    pty::{LaunchProfile, ShellLaunch},
};
use serde_json::{Value, json};

use crate::codex_permissions;

pub(crate) const STARTUP_GRACE: Duration = Duration::from_millis(150);

pub(crate) struct CodexLaunch {
    pub launch: ShellLaunch,
    pub provider_config_id: String,
    pub model: String,
    pub permission_mode: &'static str,
    pub permission_warning: Option<&'static str>,
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
    permission_mode: Option<&Value>,
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
    let (permission_mode, permission_warning) =
        codex_permissions::append_args(&mut argv, permission_mode, &cwd, &environment);
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
        permission_warning,
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
