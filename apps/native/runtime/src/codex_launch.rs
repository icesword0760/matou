//! Prepare a fresh Codex TUI for the owner to launch on an existing card.
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
#[cfg(unix)]
use matou_native_core::provider_launch::resolve_command_with_login_path;
use matou_native_core::{
    provider_launch::{EFFORTS, validate_cwd},
    pty::{LaunchProfile, ShellLaunch, get_var},
};
use serde::Deserialize;
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

pub(crate) fn shell_environment_path_arg(name: &str, path: &Path) -> Option<String> {
    let path = std::path::absolute(path).ok()?;
    let path = path.to_str()?;
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    Some(format!(
        "shell_environment_policy.set.{name}={}",
        json!(path)
    ))
}

pub(crate) fn finish_argv(
    argv: &mut Vec<String>,
    status_directory: Option<&Path>,
    roles_file: &Path,
    subcommand: Option<Vec<String>>,
) {
    if let Some(status_arg) =
        status_directory.and_then(|path| shell_environment_path_arg("MATOU_AGENT_STATUS_DIR", path))
    {
        argv.extend(["-c".into(), status_arg]);
        if roles_file.is_file()
            && let Some(roles_arg) =
                shell_environment_path_arg("MATOU_FOREMAN_ROLES_FILE", roles_file)
        {
            argv.extend(["-c".into(), roles_arg]);
        }
    }
    argv.extend(subcommand.into_iter().flatten());
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
    let command = get_var(&environment, "MATOU_CODEX_COMMAND")
        .filter(|value| !value.is_empty())
        .unwrap_or(OsStr::new("codex"))
        .to_owned();
    let path = Path::new(&command);
    let cwd = std::path::absolute(cwd)?;
    #[cfg(unix)]
    let executable = resolve_command_with_login_path(path, &cwd, &mut environment).ok();
    #[cfg(not(unix))]
    let executable = resolve_executable(path, &cwd, &environment);
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
    if let Some(status_line) = status_line_arg(&cwd, &environment) {
        argv.extend(["-c".into(), status_line]);
    }
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

pub(crate) fn config_root<'a>(get: impl Fn(&str) -> Option<&'a OsStr>) -> Option<PathBuf> {
    let get = |key| get(key).filter(|value| !value.is_empty());
    if let Some(root) = get("CODEX_HOME") {
        return Some(PathBuf::from(root));
    }
    #[cfg(windows)]
    let home = get("USERPROFILE").or_else(|| get("HOME"));
    #[cfg(not(windows))]
    let home = get("HOME");
    home.map(|home| Path::new(home).join(".codex"))
}

#[derive(Default, Deserialize)]
struct StatusLineConfig {
    tui: Option<CodexTuiConfig>,
    project_root_markers: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct CodexTuiConfig {
    status_line: Option<Vec<String>>,
}

fn read_status_line_config(root: &Path) -> Option<StatusLineConfig> {
    match fs::read_to_string(root.join("config.toml")) {
        Ok(contents) => toml::from_str(&contents).ok(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(StatusLineConfig::default()),
        Err(_) => None,
    }
}

fn status_line_arg(cwd: &Path, environment: &BTreeMap<OsString, OsString>) -> Option<String> {
    let root = config_root(|key| get_var(environment, key))
        .map(|root| cwd.join(root))
        .or_else(|| std::env::home_dir().map(|home| home.join(".codex")))?;
    let config = read_status_line_config(&root)?;
    let cwd = cwd.canonicalize().ok()?;
    let root = root.canonicalize().unwrap_or(root);
    let default_markers = [".git".to_owned()];
    let markers = config
        .project_root_markers
        .as_deref()
        .unwrap_or(&default_markers);
    let project_root = cwd
        .ancestors()
        .find(|dir| {
            markers.iter().any(|marker| {
                let path = dir.join(marker);
                path.exists() && (marker != ".git" || !path.is_dir() || path.join("HEAD").exists())
            })
        })
        .unwrap_or(&cwd);
    for dir in cwd
        .ancestors()
        .take_while(|dir| dir.starts_with(project_root))
    {
        let project_config = dir.join(".codex");
        if project_config != root
            && project_config.canonicalize().ok().as_ref() != Some(&root)
            && read_status_line_config(&project_config)?
                .tui
                .and_then(|tui| tui.status_line)
                .is_some()
        {
            // Codex decides whether to trust a project layer and apply its settings.
            return None;
        }
    }

    let items = config
        .tui
        .and_then(|tui| tui.status_line)
        .unwrap_or_else(|| vec!["fast-mode".into()]);
    let has_speed = items.iter().any(|item| item == "fast-mode");
    let mut added_speed = false;
    let items: Vec<_> = items
        .iter()
        .filter_map(|item| {
            let item = match item.as_str() {
                "model" | "model-name" | "current-dir" | "thread-name" | "git-branch"
                | "context-remaining" | "context-used" | "context-usage" => return None,
                "model-with-reasoning" if has_speed => return None,
                "model-with-reasoning" => "fast-mode",
                item => item,
            };
            if item == "fast-mode" && std::mem::replace(&mut added_speed, true) {
                return None;
            }
            Some(item)
        })
        .collect();
    Some(format!("tui.status_line={}", json!(items)))
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

pub(crate) fn resolve_executable(
    path: &Path,
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Option<PathBuf> {
    #[cfg(windows)]
    let check = |path: PathBuf| {
        if path.extension().is_some() {
            return executable_at(path);
        }
        let extensions = get_var(environment, "PATHEXT").unwrap_or(OsStr::new(".EXE;.CMD;.BAT"));
        std::env::split_paths(extensions)
            .filter_map(|extension| extension.to_str()?.strip_prefix('.').map(str::to_owned))
            .filter(|extension| ["exe", "cmd", "bat"].iter().any(|allowed| extension.eq_ignore_ascii_case(allowed)))
            .chain(["exe".to_owned(), "cmd".to_owned()])
            .find_map(|extension| executable_at(path.with_extension(extension)))
    };
    #[cfg(not(windows))]
    let check = executable_at;
    if path.file_name() != Some(path.as_os_str()) {
        return check(cwd.join(path));
    }
    get_var(environment, "PATH")
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| !dir.as_os_str().is_empty())
        .find_map(|dir| check(cwd.join(dir).join(path)))
}

fn executable_at(path: PathBuf) -> Option<PathBuf> {
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

#[cfg(windows)]
pub(crate) fn is_batch(path: &Path) -> bool {
    path.extension().and_then(OsStr::to_str).is_some_and(|extension| {
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
    })
}

#[cfg(windows)]
pub(crate) fn run_batch_agent(args: impl IntoIterator<Item = OsString>) -> i32 {
    let mut args = args.into_iter();
    let Some(path) = args.next().map(PathBuf::from) else { return 2 };
    if !path.is_absolute() || !is_batch(&path) || !path.is_file() {
        return 2;
    }
    match std::process::Command::new(path).args(args).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_status_line_preserves_custom_items_and_honors_project_layers() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let project = temp.path().join("project");
        let cwd = project.join("nested");
        let config_root = temp.path().join("codex-home");
        fs::create_dir_all(&cwd)?;
        fs::create_dir(&config_root)?;
        let environment = BTreeMap::from([("CODEX_HOME".into(), config_root.clone().into())]);
        let arg = || status_line_arg(&cwd, &environment);
        let config = config_root.join("config.toml");

        assert_eq!(arg().as_deref(), Some(r#"tui.status_line=["fast-mode"]"#));
        for (contents, expected) in [
            (
                r#"[tui]
status_line = ["approval-mode", "model-with-reasoning", "current-dir",
               "model-name", "thread-name", "git-branch", "context-usage", "codex-version"]"#,
                Some(r#"tui.status_line=["approval-mode","fast-mode","codex-version"]"#),
            ),
            (
                r#"tui.status_line = ["model-with-reasoning", "approval-mode", "fast-mode", "fast-mode", "model"]"#,
                Some(r#"tui.status_line=["approval-mode","fast-mode"]"#),
            ),
            ("tui.status_line = []", Some("tui.status_line=[]")),
            ("[tui", None),
        ] {
            fs::write(&config, contents)?;
            assert_eq!(arg().as_deref(), expected, "{contents}");
            assert_eq!(fs::read_to_string(&config)?, contents);
        }

        fs::write(&config, "")?;
        fs::write(project.join(".git"), "gitdir: /fixture/gitdir")?;
        fs::create_dir(project.join(".codex"))?;
        let project_config = project.join(".codex/config.toml");
        fs::write(&project_config, "tui.status_line_use_colors = true")?;
        assert_eq!(arg().as_deref(), Some(r#"tui.status_line=["fast-mode"]"#));
        fs::write(&project_config, "tui.status_line = []")?;
        assert_eq!(arg(), None);
        assert_eq!(fs::read_to_string(project_config)?, "tui.status_line = []");
        Ok(())
    }

    #[test]
    fn codex_foreman_argv_keeps_environment_overrides_before_resume_and_fork() -> Result<()> {
        let temp = tempfile::Builder::new().prefix("派活 argv ").tempdir()?;
        let status = temp.path().join("agent-status/session");
        let roles = temp.path().join("dispatch-roles.json");
        fs::create_dir_all(&status)?;
        fs::write(&roles, "{}")?;
        let config = |name: &str, path: &Path| {
            let path = path.to_str().unwrap();
            #[cfg(windows)]
            let path = path.replace('\\', "/");
            format!("shell_environment_policy.set.{name}={}", json!(path))
        };
        let status_arg = config("MATOU_AGENT_STATUS_DIR", &status);
        let roles_arg = config("MATOU_FOREMAN_ROLES_FILE", &roles);
        let base = vec!["--model".to_owned(), "fixture-model".to_owned()];
        for subcommand in [None, Some("resume"), Some("fork")] {
            let tail = subcommand.map(|name| vec![name.to_owned(), "session-id".into()]);
            let mut argv = base.clone();
            finish_argv(&mut argv, Some(&status), &roles, tail.clone());
            let mut expected = base.clone();
            expected.extend([
                "-c".into(),
                status_arg.clone(),
                "-c".into(),
                roles_arg.clone(),
            ]);
            expected.extend(tail.clone().into_iter().flatten());
            assert_eq!(argv, expected);
            println!("{} argv = {argv:?}", subcommand.unwrap_or("fresh"));

            let mut argv = base.clone();
            finish_argv(&mut argv, None, &roles, tail.clone());
            assert_eq!(
                argv,
                base.iter()
                    .cloned()
                    .chain(tail.clone().into_iter().flatten())
                    .collect::<Vec<_>>()
            );

            let mut argv = base.clone();
            finish_argv(
                &mut argv,
                Some(&status),
                &temp.path().join("missing.json"),
                tail.clone(),
            );
            let mut expected = base.clone();
            expected.extend(["-c".into(), status_arg.clone()]);
            expected.extend(tail.into_iter().flatten());
            assert_eq!(argv, expected);
        }
        Ok(())
    }
}
