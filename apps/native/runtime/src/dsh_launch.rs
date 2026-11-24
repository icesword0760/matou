//! Build an independent DSH launch without starting a process or writing a profile.

use std::{
    collections::{BTreeMap, HashSet},
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::json;

use crate::dsh_events::{js_whitespace, valid_session_id};

const COMMAND_OVERRIDE: &str = "MATOU_DSH_COMMAND";
const TUI: &str = "dsh-tui";
const CONTROL_REQUIRED: [&str; 4] = [
    "MATOU_CONTROL_ENDPOINT",
    "MATOU_CONTROL_TOKEN",
    "MATOU_CONTROL_CALLER_SESSION",
    "MATOU_CONTROL_NODE_EXECUTABLE",
];
pub const FALLBACK_CONTROL_TOKEN: &str = "DSH_MATOU_CONTROL_TOKEN";
pub const DUMP_CONFIG_ARGS: [&str; 2] = [TUI, "--dump-config"];

pub fn matches_shell_command(command: &str) -> bool {
    let mut words = command.split(js_whitespace).filter(|word| !word.is_empty());
    if words.next() != Some("dsh") {
        return false;
    }
    match words.next() {
        None => true,
        Some(TUI) => words.next().is_none(),
        Some("--profile") => words.next() == Some(TUI) && words.next().is_none(),
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DshLaunchMode {
    Fresh,
    Resume { expected_session_id: String },
}

impl DshLaunchMode {
    pub fn expected_session_id(&self) -> Option<&str> {
        match self {
            Self::Fresh => None,
            Self::Resume {
                expected_session_id,
            } => Some(expected_session_id),
        }
    }
}

pub struct DshLaunchPlan {
    pub executable: PathBuf,
    pub argv: Vec<OsString>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<OsString, OsString>,
    pub mode: DshLaunchMode,
}

pub struct DshLaunchInput<'a> {
    pub cwd: &'a Path,
    /// Path reserved by the current run's hook registration.
    pub patch_path: Option<&'a Path>,
    /// An explicit identity from the selected persisted session or binding.
    pub resume_session_id: Option<&'a str>,
    pub inherited_environment: BTreeMap<OsString, OsString>,
    /// Current-run values, including the hook and control registration if available.
    pub run_environment: BTreeMap<OsString, OsString>,
}

/// The caller owns hook registration, writing the patch and spawning the PTY.
pub fn prepare(input: DshLaunchInput<'_>) -> Result<DshLaunchPlan> {
    let mode = if let Some(id) = input.resume_session_id {
        let id = id.trim_matches(js_whitespace);
        // The frozen history catalog exposes only IDs of up to 160 characters.
        ensure!(
            id.len() <= 160 && valid_session_id(id),
            "Invalid DSH session ID"
        );
        DshLaunchMode::Resume {
            expected_session_id: id.to_owned(),
        }
    } else {
        DshLaunchMode::Fresh
    };

    ensure!(
        !input.cwd.as_os_str().is_empty(),
        "DSH working directory is missing"
    );
    let cwd = std::path::absolute(input.cwd)?;
    ensure!(
        cwd.is_dir(),
        "DSH working directory is unavailable: {}",
        cwd.display()
    );

    let mut environment = input.inherited_environment;
    environment.retain(|key, _| {
        !CONTROL_REQUIRED
            .iter()
            .chain([
                &"MATOU_CONTROL_PROTOCOL",
                &"MATOU_CONTROL_CALLER_RUN",
                &"MATOU_CONTROL_ASSET_ROOT",
                &"MATOU_AGENT_STATUS_DIR",
                &"MATOU_FOREMAN_ROLES_FILE",
                &"MATOU_PROVIDER_HOOK_URL",
                &"MATOU_PROVIDER_HOOK_TOKEN",
                &"MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE",
            ])
            .any(|name| env_key_eq(key, name))
    });
    for (key, value) in input.run_environment {
        if cfg!(windows)
            && let Some(name) = key.to_str()
        {
            environment.retain(|old, _| !env_key_eq(old, name));
        }
        environment.insert(key, value);
    }
    // Caller-session markers are dropped for every independent agent. The DSH
    // bridge registers its backup token only inside the foreground shell.
    environment.retain(|key, _| {
        ![
            FALLBACK_CONTROL_TOKEN,
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
            "TERM",
        ]
        .iter()
        .any(|name| env_key_eq(key, name))
    });
    environment.insert("TERM".into(), "xterm-256color".into());
    // DSH has no Claude timezone preference. Inherited user TZ is left intact.
    #[cfg(unix)]
    environment.insert("PWD".into(), cwd.clone().into_os_string());

    let executable = discover_executable(&cwd, &environment)?;
    let mut argv = vec![TUI.into()];
    if let Some(path) = input.patch_path.filter(|path| !path.as_os_str().is_empty()) {
        argv.extend([OsString::from("--patch"), path.as_os_str().to_owned()]);
    }
    if let Some(id) = mode.expected_session_id() {
        argv.extend([OsString::from("--resume"), id.into()]);
    }
    Ok(DshLaunchPlan {
        executable,
        argv,
        cwd,
        environment,
        mode,
    })
}

fn env_key_eq(key: &OsStr, name: &str) -> bool {
    if cfg!(windows) {
        key.to_str()
            .is_some_and(|key| key.eq_ignore_ascii_case(name))
    } else {
        key == OsStr::new(name)
    }
}

fn env_value<'a>(environment: &'a BTreeMap<OsString, OsString>, name: &str) -> Option<&'a OsStr> {
    if cfg!(windows) {
        environment
            .iter()
            .find_map(|(key, value)| env_key_eq(key, name).then_some(value.as_os_str()))
    } else {
        environment.get(OsStr::new(name)).map(OsString::as_os_str)
    }
}

fn discover_executable(cwd: &Path, environment: &BTreeMap<OsString, OsString>) -> Result<PathBuf> {
    let command = env_value(environment, COMMAND_OVERRIDE)
        .filter(|value| !value.is_empty())
        .unwrap_or(OsStr::new("dsh"));
    let path = Path::new(command);
    let executable = if path.file_name() != Some(command) {
        executable_at(cwd.join(path))
    } else {
        env_value(environment, "PATH")
            .into_iter()
            .flat_map(std::env::split_paths)
            .filter(|dir| !dir.as_os_str().is_empty())
            .find_map(|dir| executable_at(cwd.join(dir).join(path)))
    };
    executable.with_context(|| {
        let command = command.to_string_lossy();
        if env_value(environment, "MATOU_LOCALE") == Some(OsStr::new("en")) {
            format!("DSH CLI ({command}) was not found. Install it and ensure `{command}` is on PATH, then try again.")
        } else {
            format!("未检测到 DSH（命令：{command}）。请先安装并确保 `{command}` 在 PATH 中，再重试。")
        }
    })
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

fn has_managed_control(environment: &BTreeMap<OsString, OsString>, run_id: &str) -> bool {
    !run_id.is_empty()
        && env_value(environment, "MATOU_CONTROL_PROTOCOL") == Some(OsStr::new("1"))
        && env_value(environment, "MATOU_CONTROL_CALLER_RUN") == Some(OsStr::new(run_id))
        && CONTROL_REQUIRED
            .iter()
            .all(|key| env_value(environment, key).is_some_and(|value| !value.is_empty()))
}

/// Static section used by the bridge for the exact foreground agent. It has no
/// interpolated run values. The bridge also waits for shell registration.
pub fn system_instructions(
    environment: &BTreeMap<OsString, OsString>,
    run_id: &str,
) -> Option<&'static str> {
    has_managed_control(environment, run_id).then_some(include_str!(
        "../../../runtime/control-assets/providers/dsh-system-message.md"
    ))
}

/// The native shell registration forwards only this alias after DSH scrubs
/// credentials. Foreground ownership is established by the bridge for each call.
pub fn foreground_shell_token<'a>(
    environment: &'a BTreeMap<OsString, OsString>,
    run_id: &str,
    owns_foreground: bool,
) -> Option<(&'static str, &'a OsStr)> {
    if !owns_foreground || !has_managed_control(environment, run_id) {
        return None;
    }
    env_value(environment, "MATOU_CONTROL_TOKEN").map(|token| (FALLBACK_CONTROL_TOKEN, token))
}

/// Resolve mt's caller token after DSH has optionally scrubbed the primary name.
pub fn control_token(environment: &BTreeMap<OsString, OsString>) -> Option<&OsStr> {
    ["MATOU_CONTROL_TOKEN", FALLBACK_CONTROL_TOKEN]
        .into_iter()
        .find_map(|key| env_value(environment, key).filter(|value| !value.is_empty()))
}

/// Render the per-run observation overlay from the CLI's literal config dump.
/// The bridge installs the scoped instruction section and shell registration.
pub fn observation_patch(
    dump: &str,
    bridge_path: &str,
    hook_url: &str,
    run_id: &str,
) -> Result<String> {
    let mut lines = dump
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line));
    ensure!(
        lines.any(|line| line == "- id: dsh-tui"),
        "DSH profile has no top-level dsh-tui entry"
    );
    let row: Vec<_> = lines.take_while(|line| !line.starts_with("- ")).collect();
    let config_at = row
        .iter()
        .position(|line| *line == "  config:")
        .context("DSH TUI config must be a literal mapping for a temporary overlay")?;
    let mut inject = Vec::new();
    if let Some(index) = row.iter().position(|line| line.starts_with("  inject:")) {
        ensure!(
            row[index] == "  inject:",
            "DSH TUI inject must be a literal service list"
        );
        for service in row[index + 1..]
            .iter()
            .map_while(|line| line.strip_prefix("    - "))
        {
            let service = service.trim_end_matches(js_whitespace);
            ensure!(
                service
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphabetic)
                    && service.bytes().all(config_key_byte),
                "DSH TUI inject must be a literal service list"
            );
            inject.push(service);
        }
    }

    let mut patch = vec![
        "- insert:".into(),
        "    - id: matou-dsh-observer".into(),
        format!("      name: {}", json!(bridge_path)),
        "      config:".into(),
        format!("        hookUrl: {}", json!(hook_url)),
        format!("        runId: {}", json!(run_id)),
        "- id: dsh-tui".into(),
        "  inject:".into(),
    ];
    let mut seen = HashSet::new();
    patch.extend(
        inject
            .into_iter()
            .chain(["matouDshBridge"])
            .filter(|name| seen.insert(*name))
            .map(|name| format!("    - {name}")),
    );
    patch.push("  config:".into());
    let mut dropping = false;
    for line in &row[config_at + 1..] {
        if let Some(rest) = line.strip_prefix("  ")
            && rest.chars().next().is_some_and(|c| !js_whitespace(c))
        {
            break;
        }
        if let Some((key, _)) = line
            .strip_prefix("    ")
            .and_then(|line| line.split_once(':'))
            && !key.is_empty()
            && key.bytes().all(config_key_byte)
        {
            dropping = matches!(key, "backend" | "taskNotifications" | "upstreamRetry");
        }
        if !dropping {
            patch.push((*line).into());
        }
    }
    patch.extend([
        "    backend: dsh".into(),
        "    taskNotifications: \"off\"".into(),
        "    upstreamRetry: false".into(),
        String::new(),
    ]);
    Ok(patch.join("\n"))
}

fn config_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}
