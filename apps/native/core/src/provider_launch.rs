//! Prepare a fresh Claude Code run from the frozen runtime's V1 settings.
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::Value;

use crate::pty::{
    CALLER_SESSION_MARKERS, LaunchProfile, ShellLaunch, get_var, remove_var, set_var,
};

const OFFICIAL_ID: &str = "anthropic-official";
const ROLES: [&str; 3] = ["opus", "sonnet", "haiku"];

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Provider {
    #[serde(rename = "id")]
    _id: String,
    endpoint: String,
    model: String,
    api_key: Option<String>,
    kind: Option<String>,
    subscription: Option<Value>,
    reasoning_effort: Option<String>,
    #[serde(default)]
    model_mapping: BTreeMap<String, String>,
    #[serde(default)]
    effort_mapping: BTreeMap<String, String>,
}

pub fn prepare_claude_launch(
    data_root: &Path,
    cwd: &Path,
    binding: &Value,
    mut environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<ShellLaunch> {
    validate_cwd(cwd)?;
    let provider = read_provider(data_root, binding)?;
    if provider.kind.as_deref() == Some("subscription")
        && let Some(subscription) = &provider.subscription
    {
        let opus = provider.model_mapping.get("opus").map(String::as_str);
        ensure!(
            opus.is_some()
                && subscription["modelMetadata"]
                    .as_array()
                    .is_some_and(|models| {
                        models.iter().any(|model| model["id"].as_str() == opus)
                    }),
            "Subscription Opus model metadata is missing"
        );
        bail!("Subscription launch service is unavailable");
    }

    let model = match binding["model"].as_str() {
        Some(bound)
            if bound.starts_with("matou-chatgpt/") || bound.starts_with("matou-antigravity/") =>
        {
            &provider.model
        }
        Some(bound) => match bound
            .strip_prefix("matou-api/")
            .filter(|role| ROLES.contains(role))
        {
            // The native process connects directly to the configured endpoint.
            // Translate a model ID previously used through the request gateway.
            Some(role) => provider.model_mapping.get(role).unwrap_or(&provider.model),
            None => bound,
        },
        None => &provider.model,
    };
    let argv = claude_argv(
        model,
        reasoning_effort(&provider, model),
        binding["permissionMode"].as_str(),
    );
    for key in CALLER_SESSION_MARKERS {
        remove_var(&mut environment, OsStr::new(key));
    }
    let executable = resolve_claude_executable(cwd, &mut environment)?;
    project_environment(&provider, &mut environment);
    #[cfg(unix)]
    set_var(&mut environment, "PWD".into(), cwd.as_os_str().into());

    Ok(ShellLaunch {
        profile: LaunchProfile::ClaudeCode,
        executable,
        argv,
        cwd: cwd.to_owned(),
        data_root: data_root.to_owned(),
        inherited_env: BTreeMap::new(),
        env: environment,
        cols,
        rows,
    })
}

pub fn validate_cwd(cwd: &Path) -> Result<()> {
    ensure!(
        !cwd.as_os_str().is_empty(),
        "Claude Code working directory is missing (stored cwd is empty)"
    );
    let metadata = fs::metadata(cwd)
        .with_context(|| format!("open Claude Code working directory {}", cwd.display()))?;
    ensure!(
        metadata.is_dir(),
        "Claude Code working directory {} is not a directory",
        cwd.display()
    );
    #[cfg(unix)]
    check_execute_access(cwd)
        .with_context(|| format!("access Claude Code working directory {}", cwd.display()))?;
    Ok(())
}

fn read_provider(data_root: &Path, binding: &Value) -> Result<Provider> {
    let path = data_root.join("provider-config.json");
    let config = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(|error| {
            anyhow::anyhow!(
                "read {}: invalid JSON at line {}, column {}",
                path.display(),
                error.line(),
                error.column()
            )
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    ensure!(
        !config.is_null(),
        "read {}: invalid provider configuration root",
        path.display()
    );

    let bound_id = binding["providerConfigId"]
        .as_str()
        .filter(|id| !id.trim().is_empty());
    // Frozen V1 normalization seeds an empty list and repairs a stale active ID
    // to the first entry. A binding is explicit and must still resolve exactly.
    let providers = (config["version"].as_u64() == Some(1)
        && !config["providers"].is_null()
        && !config["activeProviderIds"].is_null())
    .then(|| config["providers"]["claude-code"].as_array())
    .flatten()
    .filter(|providers| !providers.is_empty());

    let missing = "会话绑定的供应商配置不存在";
    let Some(providers) = providers else {
        ensure!(bound_id.is_none_or(|id| id == OFFICIAL_ID), "{missing}");
        return Ok(Provider {
            _id: OFFICIAL_ID.into(),
            endpoint: "https://api.anthropic.com".into(),
            ..Provider::default()
        });
    };
    let active_id = config["activeProviderIds"]["claude-code"].as_str();
    let find = |id| {
        providers
            .iter()
            .find(|provider| provider["id"].as_str() == Some(id))
    };
    let selected = match bound_id {
        Some(id) => find(id).context(missing)?,
        None => active_id.and_then(find).unwrap_or(&providers[0]),
    };
    // serde's field-type errors can include the offending value, including a
    // malformed credential object. Report the file and the failure without it.
    serde_json::from_value(selected.clone()).map_err(|_| {
        anyhow::anyhow!(
            "read {}: invalid Claude Code provider entry",
            path.display()
        )
    })
}

fn claude_argv(model: &str, effort: Option<&str>, permission: Option<&str>) -> Vec<String> {
    let mut argv = Vec::new();
    if !model.trim().is_empty() {
        argv.extend(["--model".into(), model.trim().into()]);
    }
    if let Some(effort) = effort.filter(|effort| !effort.is_empty() && *effort != "follow") {
        argv.extend(["--effort".into(), effort.into()]);
    }
    match permission {
        Some("bypassPermissions") => argv.push("--dangerously-skip-permissions".into()),
        Some(mode @ ("auto" | "acceptEdits" | "plan")) => {
            argv.extend(["--permission-mode".into(), mode.into()]);
        }
        _ => {}
    }
    argv
}

fn reasoning_effort<'a>(provider: &'a Provider, model: &str) -> Option<&'a str> {
    fn without_context_suffix(model: &str) -> &str {
        model
            .split_at_checked(model.len().saturating_sub(4))
            .filter(|(_, suffix)| suffix.eq_ignore_ascii_case("[1m]"))
            .map_or(model, |(base, _)| base)
    }
    let model = if model.is_empty() { "default" } else { model };
    let mapped = ROLES.into_iter().find(|role| {
        provider
            .model_mapping
            .get(*role)
            .is_some_and(|mapped| without_context_suffix(mapped) == without_context_suffix(model))
    });
    let lower_model = model.to_ascii_lowercase();
    let inferred = || {
        ROLES.into_iter().find(|role| {
            ["matou-", "claude-"].into_iter().any(|prefix| {
                let pattern = format!("{prefix}{role}");
                lower_model.match_indices(&pattern).any(|(index, _)| {
                    let suffix = &lower_model[index + pattern.len()..];
                    suffix.is_empty() || suffix.starts_with(['-', '/', '['])
                })
            })
        })
    };
    mapped
        .or_else(inferred)
        .and_then(|role| provider.effort_mapping.get(role))
        .map(String::as_str)
        .or(provider.reasoning_effort.as_deref())
}

fn project_environment(provider: &Provider, environment: &mut BTreeMap<OsString, OsString>) {
    if let Some(key) = provider.api_key.as_deref().filter(|key| !key.is_empty()) {
        set_var(environment, "ANTHROPIC_API_KEY".into(), key.into());
        set_var(environment, "ANTHROPIC_AUTH_TOKEN".into(), key.into());
    }
    let endpoint = provider
        .endpoint
        .strip_suffix('/')
        .unwrap_or(&provider.endpoint);
    if !endpoint.is_empty() {
        set_var(environment, "ANTHROPIC_BASE_URL".into(), endpoint.into());
    }
    for (role, model) in &provider.model_mapping {
        if !model.is_empty() {
            set_var(
                environment,
                format!("ANTHROPIC_DEFAULT_{}_MODEL", role.to_uppercase()).into(),
                model.into(),
            );
        }
    }
    remove_var(environment, OsStr::new("NO_COLOR"));
    for (key, value) in [
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("FORCE_COLOR", "1"),
    ] {
        set_var(environment, key.into(), value.into());
    }
    #[cfg(target_os = "macos")]
    if get_var(environment, "mimalloc_purge_holes_min_interval").is_none()
        && get_var(environment, "MIMALLOC_PURGE_HOLES_MIN_INTERVAL").is_none()
    {
        set_var(
            environment,
            "MIMALLOC_PURGE_HOLES_MIN_INTERVAL".into(),
            "1000".into(),
        );
    }
}

fn resolve_claude_executable(
    cwd: &Path,
    environment: &mut BTreeMap<OsString, OsString>,
) -> Result<PathBuf> {
    let command = get_var(environment, "MATOU_CLAUDE_COMMAND")
        .filter(|command| !command.is_empty())
        .unwrap_or(OsStr::new("claude"))
        .to_owned();
    let command = Path::new(&command);
    let cwd = std::path::absolute(cwd).context("resolve Claude Code working directory")?;
    let resolved = search_command(command, &cwd, environment);
    #[cfg(unix)]
    let resolved = if resolved.is_err()
        && command.file_name() == Some(command.as_os_str())
        && get_var(environment, "MATOU_DISABLE_LOGIN_PATH_PROBE") != Some(OsStr::new("1"))
        && let Some(path) = login_shell_path(environment)
    {
        set_var(environment, "PATH".into(), path.into());
        search_command(command, &cwd, environment).or(resolved)
    } else {
        resolved
    };

    match resolved {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let command = command.display();
            bail!(
                "未检测到 Claude Code（命令：{command}）。请先安装并确保 `{command}` 在 PATH 中，再重试。"
            );
        }
        result => result.with_context(|| format!("start Claude Code {}", command.display())),
    }
}

fn search_command(
    command: &Path,
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> io::Result<PathBuf> {
    let mut failure = io::Error::from(io::ErrorKind::NotFound);
    let mut check = |path: PathBuf| {
        let result = fs::metadata(&path).and_then(|metadata| {
            if metadata.is_dir() {
                return Err(io::Error::from(io::ErrorKind::IsADirectory));
            }
            #[cfg(unix)]
            check_execute_access(&path)?;
            Ok(())
        });
        match result {
            Ok(()) => Some(path),
            Err(error) => {
                if failure.kind() == io::ErrorKind::NotFound {
                    failure = error;
                }
                None
            }
        }
    };
    #[cfg(windows)]
    let mut check = |path: PathBuf| {
        check(path.clone()).or_else(|| {
            if path.extension().is_some() {
                return None;
            }
            let extensions = get_var(environment, "PATHEXT").unwrap_or(OsStr::new(".EXE"));
            std::env::split_paths(extensions)
                .filter_map(|ext| {
                    ext.to_str()
                        .and_then(|ext| ext.strip_prefix('.'))
                        .map(str::to_owned)
                })
                .find_map(|ext| check(path.with_extension(ext)))
        })
    };

    let found = if command.file_name() != Some(command.as_os_str()) {
        check(cwd.join(command))
    } else {
        get_var(environment, "PATH").and_then(|path| {
            std::env::split_paths(path)
                .filter(|dir| !dir.as_os_str().is_empty())
                .find_map(|dir| check(cwd.join(dir).join(command)))
        })
    };
    found.ok_or(failure)
}

#[cfg(unix)]
fn check_execute_access(path: &Path) -> io::Result<()> {
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
    if unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn login_shell_path(environment: &BTreeMap<OsString, OsString>) -> Option<String> {
    use std::{
        io::Read,
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    const MARKER: &str = "__MATOU_LOGIN_PATH__=";
    const LIMIT: usize = 256 * 1024;
    let shell = get_var(environment, "SHELL").unwrap_or(OsStr::new("/bin/zsh"));
    let mut child = Command::new(shell)
        .args(["-ilc", "printf '__MATOU_LOGIN_PATH__=%s\\n' \"$PATH\""])
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take().expect("piped login-shell output");
    let fd = stdout.as_raw_fd();
    let mut data = Vec::new();
    let mut eof = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } == 0 {
        loop {
            if !eof {
                let remaining = (LIMIT + 1 - data.len()) as u64;
                match stdout.by_ref().take(remaining).read_to_end(&mut data) {
                    Ok(_) => eof = true,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(_) => break,
                }
            }
            if data.len() > LIMIT {
                break;
            }
            if eof && let Some(status) = child.try_wait().ok().flatten() {
                return status
                    .success()
                    .then(|| {
                        String::from_utf8_lossy(&data)
                            .lines()
                            .rev()
                            .find_map(|line| line.strip_prefix(MARKER))
                            .map(str::trim)
                            .filter(|path| !path.is_empty())
                            .map(str::to_owned)
                    })
                    .flatten();
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    // The child still owns this process group ID until waited on.
    unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    let _ = child.wait();
    None
}
