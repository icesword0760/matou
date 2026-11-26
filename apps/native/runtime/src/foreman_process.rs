//! Start and supervise Foreman workers in the existing on-disk format.

use super::mail::{identify, notify_started, read_prompt, timestamp, with_lock, write_json};
use super::{
    AGENTS, DEFAULT_WATCH_MINUTES, Store, USAGE, agent_id, agent_name, clock, final_text,
    mailbox_path, now_ms, numbered_mail, pid_alive, read_json, short, task_argument, terminal_code,
    timestamp_ms, turns,
};
use crate::codex_launch::{config_root, resolve_executable};
use anyhow::{Context, Result, bail, ensure};
use matou_native_core::dispatch_roles::{DispatchRoleMode, DispatchRolesConfig, agy_effort_suffix};
use matou_native_core::git_repo::git_command;
use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{LazyLock, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SUPERVISION_FIELDS: [&str; 4] = ["idleMin", "maxMin", "askWaitMin", "retries"];
const STOP_POLL: Duration = Duration::from_millis(100);
const MODEL_TIMEOUT: Duration = Duration::from_secs(20);
const CODEX_TIERS: [&str; 5] = ["default", "fast", "ultrafast", "priority", "flex"];
const AGY_SERIES: [&str; 5] = ["hydrogen", "argon", "carbon", "neon", "flash"];
const AGY_FAMILIES: [&str; 2] = ["claude-opus-5-5", "claude-sonnet-5-5"];

pub(super) struct LaunchSettings {
    pub(super) follow: bool,
    pub(super) watch_minutes: f64,
    pub(super) prompt_file: Option<PathBuf>,
}

struct Arguments {
    settings: LaunchSettings,
    agent: Option<String>,
    values: Map<String, Value>,
}

fn parse(options: &[&str], resume: bool) -> Result<Arguments> {
    let mut settings = LaunchSettings {
        follow: true,
        watch_minutes: DEFAULT_WATCH_MINUTES,
        prompt_file: None,
    };
    let mut values = Map::new();
    let mut agent = None;
    let mut add_dirs = Vec::new();
    let mut flags = options.iter().copied();
    while let Some(flag) = flags.next() {
        let mut next = || {
            flags
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .with_context(|| format!("{flag} 需要参数"))
        };
        match flag {
            "--no-watch" => settings.follow = false,
            "--watch-min" => settings.watch_minutes = positive_minutes(next()?)?,
            "--brief" if !resume => settings.prompt_file = Some(std::path::absolute(next()?)?),
            "--message" if resume => settings.prompt_file = Some(std::path::absolute(next()?)?),
            "--cd" if !resume => {
                values.insert("cd".into(), json!(std::path::absolute(next()?)?));
            }
            "--add-dir" if !resume => add_dirs.push(std::path::absolute(next()?)?),
            "--network" if !resume => {
                values.insert("network".into(), json!(true));
            }
            "--no-project-doc" if !resume => {
                values.insert("noProjectDoc".into(), json!(true));
            }
            "--refresh-models" if !resume => {
                values.insert("refreshModels".into(), json!(true));
            }
            "--full-access" | "--sandbox" => {
                values.insert("fullAccess".into(), json!(flag == "--full-access"));
            }
            "--agent" => {
                let value = next()?;
                ensure!(AGENTS.contains(&value), "--agent must be codex, dsh or agy");
                agent = Some(value.into());
            }
            "--model" => {
                let model = next()?;
                ensure!(
                    !model.starts_with('-') && !model.contains('\0'),
                    "无效的 --model"
                );
                values.insert("model".into(), json!(model));
            }
            "--effort" => {
                values.insert("effort".into(), json!(next()?));
            }
            "--speed" => {
                let value = next()?;
                ensure!(CODEX_TIERS.contains(&value), "无效的 --speed");
                values.insert("speedOverride".into(), json!(value));
            }
            "--retries" => {
                let retries: u32 = next()?.parse().context("无效的 --retries")?;
                values.insert("retries".into(), json!(retries));
            }
            "--backoff" => {
                let backoff = next()?
                    .split(',')
                    .map(|value| {
                        let value: f64 = value.trim().parse().context("无效的 --backoff")?;
                        ensure!(
                            Duration::try_from_secs_f64(value * 60.0).is_ok(),
                            "无效的 --backoff"
                        );
                        Ok(value)
                    })
                    .collect::<Result<Vec<_>>>()?;
                values.insert("backoffMin".into(), json!(backoff));
            }
            "--idle-min" | "--warn-min" | "--max-min" | "--ask-wait-min" => {
                let field = match flag {
                    "--idle-min" => "idleMin",
                    "--warn-min" => "warnMin",
                    "--max-min" => "maxMin",
                    _ => "askWaitMin",
                };
                values.insert(field.into(), json!(positive_minutes(next()?)?));
            }
            _ => bail!("Unknown option {flag}\n{USAGE}"),
        }
    }
    if !resume {
        values.insert("addDirs".into(), json!(add_dirs));
    }
    Ok(Arguments {
        settings,
        agent,
        values,
    })
}

fn positive_minutes(input: &str) -> Result<f64> {
    let value: f64 = input.parse().context("必须指定有效分钟数")?;
    ensure!(
        value > 0.0 && Duration::try_from_secs_f64(value * 60.0).is_ok(),
        "必须指定有效分钟数"
    );
    Ok(value)
}

fn options_defaults(agent: &str) -> Map<String, Value> {
    let model = match agent {
        "agy" => "gemini-pro-orchestrator",
        "dsh" => "deepseek-flash",
        _ => "sol",
    };
    let Value::Object(options) = json!({
        "model": model, "effort": "high", "askWaitMin": 10, "idleMin": 10, "warnMin": 3,
        "maxMin": 45, "retries": 3, "backoffMin": [2, 5, 10], "addDirs": []
    }) else {
        unreachable!()
    };
    options
}

struct SavedOptions {
    path: PathBuf,
    exists: bool,
    ignored: Vec<String>,
    values: Map<String, Value>,
}

fn saved_options(store: &Store, agent: &str) -> SavedOptions {
    let path = store
        .root
        .parent()
        .unwrap_or(&store.root)
        .join("config.json");
    let mut report = SavedOptions {
        exists: path.exists(),
        path,
        ignored: Vec::new(),
        values: options_defaults(agent),
    };
    if !report.exists {
        return report;
    }
    let Ok(config) = read_json(&report.path) else {
        report
            .ignored
            .push("已忽略：config=\"无法读取或不是合法 JSON\"".into());
        return report;
    };
    if config["version"] != 1 {
        report.ignored.push(format!(
            "已忽略：version={}",
            super::shown(&config["version"])
        ));
        return report;
    }
    let Some(saved) = config["defaults"].as_object() else {
        report
            .ignored
            .push(format!("已忽略：defaults={}", config["defaults"]));
        return report;
    };
    for (key, value) in saved {
        let valid = match key.as_str() {
            "agent" if value.as_str().is_some_and(|agent| AGENTS.contains(&agent)) => continue,
            "model" => value.as_str().is_some_and(|model| {
                !model.is_empty()
                    && model.chars().count() <= 64
                    && !model.starts_with('-')
                    && !model.chars().any(char::is_whitespace)
            }),
            "effort" => value
                .as_str()
                .is_some_and(|effort| ["low", "medium", "high", "xhigh"].contains(&effort)),
            "idleMin" | "maxMin" | "askWaitMin" => value
                .as_f64()
                .is_some_and(|minutes| minutes > 0.0 && minutes <= 600.0),
            "retries" => value.as_u64().is_some_and(|retries| retries <= 5),
            _ => false,
        };
        if !valid {
            report.ignored.push(format!("已忽略：{key}={value}"));
        } else if agent == "dsh"
            && key == "effort"
            && !["low", "high"].contains(&value.as_str().unwrap_or_default())
        {
            report.ignored.push(format!(
                "已忽略：effort={value}（DSH 不支持，使用内置 high）"
            ));
        } else if agent != "dsh" || key != "model" {
            report.values.insert(key.clone(), value.clone());
        }
    }
    report
}

pub(super) fn config(store: &Store) -> Result<i32> {
    let roles = store.role_config();
    let agent = roles
        .as_ref()
        .map(|config| config.default_agent.clone())
        .or_else(|| store.legacy_default_agent())
        .unwrap_or_else(|| "codex".into());
    let SavedOptions {
        path,
        exists,
        ignored,
        mut values,
    } = saved_options(store, &agent);
    role_options(&agent, roles.as_ref(), &mut values, false);
    values.remove("addDirs");
    values.remove("allowDshSubDispatch");
    values.remove("subDispatchPeers");
    values.insert("agent".into(), json!(agent));
    let mut defaults = Value::Object(values);
    let mut report = json!({"path": path, "exists": exists, "ignored": ignored});
    if let Some(roles) = roles {
        defaults["allowSubDispatch"] = json!(roles.allow_sub_dispatch);
        if agent == "codex" {
            defaults["serviceTier"] = json!(codex_tier(&defaults));
        }
        if let Some(path) = std::env::var_os("MATOU_FOREMAN_ROLES_FILE") {
            report["matouRolesPath"] = json!(PathBuf::from(path));
        }
    }
    report["defaults"] = defaults;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(0)
}

fn role_options(
    agent: &str,
    roles: Option<&DispatchRolesConfig>,
    options: &mut Map<String, Value>,
    resume: bool,
) {
    let Some(roles) = roles else {
        options.remove("subDispatchPeers");
        if !resume {
            options.insert("allowDshSubDispatch".into(), json!(true));
        }
        return;
    };
    for (key, value) in SUPERVISION_FIELDS.into_iter().zip([
        json!(roles.idle_min),
        json!(roles.max_min),
        json!(roles.ask_wait_min),
        json!(roles.retries),
    ]) {
        options.insert(key.into(), value);
    }
    if let Some(role) = roles.agents.iter().find(|role| role.id == agent) {
        if !resume {
            if !role.model.trim().is_empty() {
                options.insert("model".into(), json!(role.model.trim()));
            }
            let efforts = if agent == "dsh" {
                ["off", "low", "high", "max"]
            } else {
                ["low", "medium", "high", "xhigh"]
            };
            if efforts.contains(&role.effort.as_str()) {
                options.insert("effort".into(), json!(role.effort));
            }
        }
        if agent == "codex" {
            options.remove("serviceTier");
            if let Some(speed) = role
                .speed
                .as_deref()
                .filter(|speed| CODEX_TIERS.contains(speed))
            {
                options.insert("serviceTier".into(), json!(speed));
            }
        }
    }
    let peers: Vec<_> = roles.agents.iter()
        .filter(|role| roles.allow_sub_dispatch && role.enabled && role.mode == DispatchRoleMode::Auto
            && role.id != agent && AGENTS.contains(&role.id.as_str()))
        .map(|role| json!({ "id": role.id, "role": role.role, "name": agent_name(&json!({"agent": role.id})) }))
        .collect();
    options.insert("subDispatchPeers".into(), json!(peers));
}

fn set_status_dir(options: &mut Map<String, Value>) {
    if let Some(dir) = std::env::var_os("MATOU_AGENT_STATUS_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
    {
        options.insert("agentStatusDir".into(), json!(dir));
    }
}

fn check_codex_config(dir: &Path) -> Result<()> {
    let environment = std::env::vars_os().collect::<BTreeMap<_, _>>();
    let config = config_root(|key| environment.get(OsStr::new(key)).map(OsString::as_os_str))
        .or_else(|| std::env::home_dir().map(|dir| dir.join(".codex")))
        .map(|dir| dir.join("config.toml"));
    let Some(config) = config else { return Ok(()) };
    let document = match fs::read_to_string(&config) {
        Ok(document) => document,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            fs::create_dir_all(dir)?;
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("supervisor.log"))?;
            writeln!(
                log,
                "{} 无法读取 Codex 配置 {}（{error}），跳过配置检查。",
                timestamp(),
                config.display()
            )?;
            return Ok(());
        }
    };
    for line in document
        .lines()
        .map(str::trim)
        .take_while(|line| !line.starts_with('['))
    {
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "model_instructions_file"
        {
            bail!(
                "你的 Codex 配置里设置了自定义指令文件（{}）。按约定，监工派的任务只使用 Codex 官方指令。请先从 {} 去掉这一行再派活。",
                value.trim(),
                config.display()
            );
        }
    }
    Ok(())
}

fn depth() -> u32 {
    std::env::var("MATOU_FOREMAN_DEPTH")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

pub(super) fn start(store: &Store, task: &str, args: &[&str]) -> Result<i32> {
    let dir = store.task_dir(task)?;
    let Arguments {
        settings,
        agent,
        mut values,
    } = parse(args, false)?;
    let roles = store.role_config();
    let agent = agent
        .or_else(|| roles.as_ref().map(|config| config.default_agent.clone()))
        .or_else(|| store.legacy_default_agent())
        .unwrap_or_else(|| "codex".into());
    let role = roles
        .as_ref()
        .and_then(|config| config.agents.iter().find(|role| role.id == agent));
    ensure!(
        !role.is_some_and(|role| !role.enabled),
        "Agent {agent} 已在 Matou 派活设置中停用，禁止派活。"
    );
    let parent_agent = std::env::var("MATOU_FOREMAN_PARENT_AGENT").unwrap_or_default();
    let allowed = if let Some(roles) = roles.as_ref() {
        roles.allow_sub_dispatch
            && !parent_agent.is_empty()
            && parent_agent != agent
            && role.is_some_and(|role| role.mode == DispatchRoleMode::Auto)
    } else {
        parent_agent == "codex" && agent == "dsh"
    };
    let depth = depth();
    ensure!(
        depth == 0 || depth == 1 && allowed,
        "当前会话是 foreman 派出的执行者，禁止再次派活（防止递归套娃或已关闭二次转包）。"
    );
    let cwd = values
        .get("cd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .with_context(|| USAGE)?;
    ensure!(cwd.is_dir(), "Work dir does not exist: {}", cwd.display());
    ensure!(
        !dir.join("state.json").exists(),
        "Task {task} already exists; use resume or pick a new name."
    );
    let explicit_model = values.contains_key("model");
    let explicit_effort = values.contains_key("effort");
    let mut options = saved_options(store, &agent).values;
    role_options(&agent, roles.as_ref(), &mut options, false);
    options.insert("fullAccess".into(), json!(true));
    options.append(&mut values);
    set_status_dir(&mut options);
    if agent == "agy" {
        check_agy_flags(&mut options, explicit_model, explicit_effort, false)?;
    }
    let prompt = read_prompt(settings.prompt_file.as_deref())?;
    options.insert("gitRoot".into(), json!(git_root(&cwd)));
    let mut state = json!({
        "task": task, "agent": agent, "createdAt": timestamp(), "status": "new", "threadId": null,
        "pid": null, "turns": [], "options": options, "ownerSession": identify()
    });
    fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
    let offset = with_lock(&dir, || {
        ensure!(
            !dir.join("state.json").exists(),
            "Task {task} already exists; use resume or pick a new name."
        );
        launch_turn_locked(store, task, &mut state, &prompt)
    })?;
    follow(store, task, settings, offset)
}

pub(super) fn prepare_resume(
    store: &Store,
    state: &mut Value,
    args: &[&str],
) -> Result<LaunchSettings> {
    let Arguments {
        settings,
        agent: requested,
        mut values,
    } = parse(args, true)?;
    let agent = agent_id(state).to_owned();
    ensure!(
        requested
            .as_deref()
            .is_none_or(|requested| requested == agent),
        "Task uses --agent {agent}; cannot switch agents on resume."
    );
    let options = state["options"]
        .as_object_mut()
        .context("任务缺少运行设置")?;
    role_options(&agent, store.role_config().as_ref(), options, true);
    let explicit_model = values.contains_key("model");
    let explicit_effort = values.contains_key("effort");
    if explicit_model {
        options.remove("modelRequest");
    }
    if agent == "agy" && !explicit_model && explicit_effort {
        let previous = options
            .get("modelRequest")
            .and_then(Value::as_str)
            .filter(|value| AGY_FAMILIES.contains(value))
            .or_else(|| {
                options
                    .get("model")
                    .and_then(Value::as_str)
                    .and_then(agy_series_parts)
                    .map(|parts| parts.1)
            })
            .map(str::to_owned)
            .context(
                "当前 Antigravity 模型没有可单独指定的推理档位，请同时指定支持档位的 --model",
            )?;
        values.insert("model".into(), json!(previous));
    }
    options.append(&mut values);
    if agent == "agy" {
        check_agy_flags(options, explicit_model, explicit_effort, true)?;
    } else if agent == "codex" {
        options.insert("refreshModels".into(), json!(true));
    }
    set_status_dir(options);
    Ok(settings)
}

fn follow(store: &Store, task: &str, settings: LaunchSettings, offset: u64) -> Result<i32> {
    if settings.follow {
        store.watch(task, Some(offset), settings.watch_minutes)
    } else {
        Ok(0)
    }
}

pub(super) fn launch_turn_locked(
    store: &Store,
    task: &str,
    state: &mut Value,
    prompt: &str,
) -> Result<u64> {
    let dir = store.task_dir(task)?;
    if agent_id(state) == "codex" {
        check_codex_config(&dir)?;
    }
    fs::create_dir_all(dir.join("logs"))?;
    let offset = fs::metadata(dir.join("feed.log"))
        .map(|file| file.len())
        .unwrap_or(0);
    let now = json!(timestamp());
    for field in ["runStartedAt", "lastActivityAt", "activityAt"] {
        state[field] = now.clone();
    }
    for field in [
        "pid",
        "supervisorPid",
        "nativeSupervisorPid",
        "endedAt",
        "exitCode",
        "error",
        "warningAt",
    ] {
        state[field] = Value::Null;
    }
    state["task"] = json!(task);
    state["status"] = json!("new");
    state["activity"] = json!("启动中");
    state["waitingQuestions"] = json!([]);
    state["waitingQuestionText"] = Value::Null;
    let prompt_file = dir.join(format!("run-{}.prompt.md", turns(state).len() + 1));
    fs::write(&prompt_file, prompt)?;
    state["pendingPrompt"] = json!(prompt_file);
    let state_path = dir.join("state.json");
    write_json(&state_path, state)?;

    let result = (|| -> Result<()> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("supervisor.log"))?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["--mt-foreman", "__supervise", task])
            .env("FOREMAN_RUN_DIR", &store.root)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        detach(&mut command);
        let mut child = command.spawn().context("无法启动 Foreman 监督进程")?;
        state["supervisorPid"] = json!(child.id());
        state["nativeSupervisorPid"] = json!(child.id());
        if let Err(error) = write_json(&state_path, state) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        // A caller with --no-watch can exit immediately; if it stays, reap the child.
        let _ = thread::Builder::new().spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    })();
    if let Err(error) = result {
        state["status"] = json!("failed");
        state["exitCode"] = json!(1);
        state["error"] = json!(format!("Supervisor launch failed: {error:#}"));
        state["endedAt"] = json!(timestamp());
        state["supervisorPid"] = Value::Null;
        state["nativeSupervisorPid"] = Value::Null;
        state.as_object_mut().unwrap().remove("pendingExternalSend");
        write_json(&state_path, state)?;
        return Err(error);
    }
    Ok(offset)
}

pub(super) fn supervise(store: &Store, task: &str) -> Result<i32> {
    let dir = store.task_dir(task)?;
    let pid = std::process::id();
    let started = Instant::now();
    let mut state = loop {
        let state = store.load(task)?;
        if state["status"] == "new"
            && state["nativeSupervisorPid"] == pid
            && state["supervisorPid"] == pid
        {
            break state;
        }
        ensure!(
            state["status"] == "new"
                && state["supervisorPid"].is_null()
                && started.elapsed() < Duration::from_secs(5),
            "无法取得任务 {task} 的监督权"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let stop_file = stop_path(&dir, pid);
    let prompt = required_str(&state, "pendingPrompt")
        .and_then(|file| fs::read_to_string(file).context("无法读取本轮交代书"));
    let result = prompt
        .and_then(|prompt| run_supervisor(store, &dir, &stop_file, task, &mut state, &prompt));
    let result = match result {
        Ok(code) => Ok(code),
        Err(error) => finish(
            &dir,
            task,
            &mut state,
            "failed",
            1,
            Some(format!("{error:#}")),
        ),
    };
    if result.is_ok() {
        let _ = fs::remove_file(stop_file);
    }
    result
}

fn finish(
    dir: &Path,
    task: &str,
    state: &mut Value,
    status: &str,
    code: i32,
    error: Option<String>,
) -> Result<i32> {
    state["status"] = json!(status);
    state["error"] = json!(error);
    state["endedAt"] = json!(timestamp());
    state["exitCode"] = json!(code);
    state["pid"] = Value::Null;
    state["waitingQuestions"] = json!([]);
    state["waitingQuestionText"] = Value::Null;
    state.as_object_mut().unwrap().remove("pendingExternalSend");

    let text = final_text(task, state);
    let mut fed = append_raw_feed(dir, &text);
    let mut saved = write_json(&dir.join("state.json"), state);
    // A detached worker may be the only process able to retry a temporarily locked file.
    for _ in 0..12 {
        if saved.is_ok() && fed.is_ok() {
            break;
        }
        thread::sleep(Duration::from_secs(5));
        if fed.is_err() {
            fed = append_raw_feed(dir, &text);
        }
        if saved.is_err() {
            saved = write_json(&dir.join("state.json"), state);
        }
    }
    saved?;
    fed?;
    publish_status(state);
    Ok(code)
}

fn stop_path(dir: &Path, supervisor: u32) -> PathBuf {
    dir.join(format!(".foreman-stop-{supervisor}"))
}

pub(super) fn stop(store: &Store, task: &str) -> Result<i32> {
    let dir = store.task_dir(task)?;
    let requested = with_lock(&dir, || {
        let mut state = store.load(task)?;
        let supervisor = state["supervisorPid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok());
        if let Some(pid) = supervisor {
            if terminal_code(&state).is_none()
                && state["nativeSupervisorPid"] == pid
                && pid_alive(&state["supervisorPid"])
            {
                fs::write(stop_path(&dir, pid), b"stop\n")?;
                return Ok(Some(pid));
            }
            if terminal_code(&state).is_none() {
                terminate_external(pid);
            }
        }
        if terminal_code(&state).is_none()
            && let Some(pid) = state["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
        {
            terminate_external(pid);
        }
        finish(&dir, task, &mut state, "stopped", 5, None)?;
        Ok(None)
    })?;
    if let Some(pid) = requested {
        let started = Instant::now();
        loop {
            let state = store.load(task)?;
            if terminal_code(&state).is_some() && state["supervisorPid"] == pid {
                break;
            }
            if !pid_alive(&json!(pid)) || started.elapsed() >= Duration::from_secs(75) {
                bail!(
                    "停止任务超时，请查看 {}",
                    dir.join("supervisor.log").display()
                );
            }
            thread::sleep(STOP_POLL);
        }
    }
    Ok(0)
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|text| !text.is_empty())
        .with_context(|| format!("任务缺少 {key}"))
}

fn append_raw_feed(dir: &Path, text: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("feed.log"))?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn feed(dir: &Path, message: &str) -> Result<()> {
    append_raw_feed(dir, &format!("{} {message}\n", clock(now_ms())))
}

fn git_root(cwd: &Path) -> Option<PathBuf> {
    let output = git_command(cwd)
        .ok()?
        .arg("rev-parse")
        .arg("--show-toplevel")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let path = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !path.trim().is_empty()).then(|| PathBuf::from(path.trim()))
}

fn agent_executable(agent: &str, cwd: &Path) -> Result<PathBuf> {
    agent_executable_with_path(agent, cwd).map(|(path, _)| path)
}

fn agent_executable_with_path(agent: &str, cwd: &Path) -> Result<(PathBuf, Option<OsString>)> {
    let (key, override_key, fallback) = match agent {
        "codex" => ("CODEX_BIN", "MATOU_CODEX_COMMAND", "codex"),
        "dsh" => ("DSH_BIN", "MATOU_DSH_COMMAND", "dsh"),
        "agy" => ("AGY_BIN", "MATOU_ANTIGRAVITY_COMMAND", "agy"),
        _ => bail!("未知 Agent：{agent}"),
    };
    let name = std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os(override_key).filter(|value| !value.is_empty()))
        .unwrap_or_else(|| fallback.into());
    let environment = std::env::vars_os().collect::<BTreeMap<_, _>>();
    let executable = resolve_executable(Path::new(&name), cwd, &environment);
    let resolved = executable.map(|path| (path, None));
    #[cfg(unix)]
    let resolved = resolved.or_else(|| {
        let mut environment = environment;
        matou_native_core::provider_launch::resolve_command_with_login_path(
            Path::new(&name),
            cwd,
            &mut environment,
        )
        .ok()
        .map(|path| (path, environment.remove(OsStr::new("PATH"))))
    });
    resolved.with_context(|| {
        format!(
            "未找到 {} CLI（{}）。请检查安装路径或 {key}。",
            fallback,
            name.to_string_lossy()
        )
    })
}

fn codex_tier(options: &Value) -> String {
    options["speedOverride"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            std::env::var("CODEX_SERVICE_TIER")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty() && value != "default")
        })
        .or_else(|| {
            options["serviceTier"]
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "default".into())
}

fn dsh_patch(dir: &Path) -> PathBuf {
    dir.join("dsh.patch.yml")
}

fn configure_dsh(dir: &Path, options: &mut Value) -> Result<()> {
    let model = match required_str(options, "model")? {
        "flash" => "deepseek-flash",
        "pro" => "deepseek-v4-pro",
        value => value,
    }
    .to_owned();
    let effort = required_str(options, "effort")?;
    ensure!(
        ["off", "low", "high", "max"].contains(&effort),
        "DSH --effort must be off, low, high or max"
    );
    let document = format!(
        "- id: agent-default-model\n  config:\n    provider: deepseek-official\n    model: {}\n    reasoningEffort: {}\n- id: bash-sandbox\n  config:\n    timeoutMs: 1800000\n",
        json!(model),
        json!(effort)
    );
    fs::write(dsh_patch(dir), document)?;
    options["model"] = json!(model);
    options.as_object_mut().unwrap().remove("modelRequest");
    options.as_object_mut().unwrap().remove("terraModel");
    Ok(())
}

fn agy_series_parts(model: &str) -> Option<(&str, &str, &str)> {
    let (version, model) = model.strip_prefix("gemini-")?.split_once('-')?;
    let (series, effort) = model.rsplit_once('-')?;
    if version.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && ["low", "medium", "high", "xhigh"].contains(&effort)
    {
        Some((version, series, effort))
    } else {
        None
    }
}

fn check_agy_flags(
    options: &mut Map<String, Value>,
    explicit_model: bool,
    explicit_effort: bool,
    resume: bool,
) -> Result<()> {
    let model = options
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let dynamic = AGY_SERIES.contains(&model.as_str()) || AGY_FAMILIES.contains(&model.as_str());
    ensure!(
        !explicit_effort || dynamic,
        "{model} 没有可单独指定的推理档位，请同时指定支持档位的 --model"
    );
    if (!resume || explicit_model || explicit_effort) && dynamic {
        options.insert("modelRequest".into(), json!(model));
        options.insert("refreshModels".into(), json!(true));
    }
    Ok(())
}

fn configure_agy(
    dir: &Path,
    bin: &Path,
    cwd: &Path,
    stop: &Path,
    options: &mut Value,
) -> Result<()> {
    let effort = required_str(options, "effort")?.to_owned();
    ensure!(
        ["low", "medium", "high", "xhigh"].contains(&effort.as_str()),
        "Antigravity --effort must be low, medium, high or xhigh"
    );
    let option_map = options.as_object_mut().unwrap();
    option_map.remove("terraModel");
    let refresh = option_map
        .remove("refreshModels")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if !refresh {
        return Ok(());
    }
    let Some(request) = option_map
        .get("modelRequest")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Ok(());
    };
    let listing = capture(bin, cwd, &["models"], Some(stop)).context("agy models 失败")?;
    let resolved = if AGY_FAMILIES.contains(&request.as_str()) {
        let model = format!("{request}-{effort}");
        listing
            .split(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '.'))
            .any(|token| token == model)
            .then_some(model)
    } else {
        listing
            .split(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '.'))
            .filter_map(|model| {
                let (version, series, model_effort) = agy_series_parts(model)?;
                (series == request && model_effort == effort)
                    .then(|| (version_key(version), model.to_owned()))
            })
            .max()
            .map(|(_, model)| model)
    }
    .with_context(|| format!("没有可用的 {request} 模型（推理档位 {effort}）"))?;
    options["model"] = json!(resolved);
    if !AGY_FAMILIES.contains(&request.as_str()) {
        options.as_object_mut().unwrap().remove("modelRequest");
    }
    feed(dir, &format!("{request} → {resolved}（推理档位 {effort}）"))?;
    Ok(())
}

fn version_key(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .collect()
}

fn codex_series(model: &str) -> Option<(&str, Vec<u32>)> {
    let (version, series) = model.strip_prefix("gpt-")?.split_once('-')?;
    Some((series, version_key(version)))
}

fn codex_tier_rank(series: &str) -> u8 {
    match series {
        "astra" => 4,
        "sol" => 3,
        "terra" => 2,
        "luna" => 1,
        _ => 0,
    }
}

fn codex_alias(model: &str) -> Option<&str> {
    (model == "best" || codex_tier_rank(model) != 0).then_some(model)
}

fn select_codex<'a>(models: &'a [Value], request: &str, effort: &str) -> Option<&'a str> {
    let mut candidates = models
        .iter()
        .filter_map(|model| {
            let slug = model["slug"].as_str()?;
            let (series, version) = codex_series(slug)?;
            let effort_ok = model["supported_reasoning_levels"]
                .as_array()?
                .iter()
                .any(|level| level.as_str().or_else(|| level["effort"].as_str()) == Some(effort));
            let rank = codex_tier_rank(series);
            (rank > 0
                && effort_ok
                && (request == "best" || request == series)
                && model["visibility"] == "list"
                && model["supported_in_api"] == true)
                .then_some((rank, version, slug, model))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| left.2.cmp(right.2))
    });
    let (_, _, slug, model) = candidates.first()?;
    let upgrade = model["upgrade"]
        .as_str()
        .or_else(|| model["upgrade"]["model"].as_str());
    Some(
        upgrade
            .filter(|upgrade| candidates.iter().any(|entry| entry.2 == *upgrade))
            .unwrap_or(slug),
    )
}

fn cached_codex(
    cache: &Value,
    key: &str,
    request: &str,
    effort: &str,
) -> Option<(ModelResolution, f64)> {
    let entry = cache
        .get("entries")
        .map(|entry| &entry[key])
        .or_else(|| (request == "best").then_some(cache))?;
    let model = entry["model"].as_str()?;
    let (series, _) = codex_series(model)?;
    let at = timestamp_ms(&entry["at"])?;
    if entry["effort"] != effort || (request != "best" && series != request) {
        return None;
    }
    Some((
        ModelResolution {
            model: model.to_owned(),
            from_cache: entry["at"].as_str().map(str::to_owned),
            catalog: entry["catalog"].as_array().cloned().unwrap_or_default(),
        },
        at,
    ))
}

fn configure_codex(
    store: &Store,
    dir: &Path,
    bin: &Path,
    cwd: &Path,
    stop: &Path,
    options: &mut Value,
) -> Result<()> {
    let effort = required_str(options, "effort")?.to_owned();
    ensure!(
        ["minimal", "low", "medium", "high", "xhigh"].contains(&effort.as_str()),
        "无效的 Codex --effort"
    );
    let request = options["modelRequest"]
        .as_str()
        .or_else(|| options["model"].as_str())
        .map(str::to_ascii_lowercase)
        .filter(|request| codex_alias(request).is_some());
    let refresh = options
        .as_object_mut()
        .unwrap()
        .remove("refreshModels")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let mut lookup = CodexModels::new(store, cwd, Some(bin), Some(stop));
    let mut report = |message: &str| feed(dir, message);
    if let Some(request) = request {
        let resolved = lookup.resolve(&request, &effort, refresh, &mut report)?;
        report(&format!(
            "{request} → {}（推理档位 {effort}）",
            resolved.model
        ))?;
        options["model"] = json!(resolved.model);
        options["modelRequest"] = json!(request);
    }
    options.as_object_mut().unwrap().remove("terraModel");
    if codex_series(required_str(options, "model")?).is_none_or(|(series, _)| series != "terra") {
        match lookup.resolve("terra", &effort, refresh, &mut report) {
            Ok(resolved) => {
                report(&format!(
                    "界面操作子代理 → {}（推理档位 {effort}）",
                    resolved.model
                ))?;
                options["terraModel"] = json!(resolved.model);
            }
            Err(_) => report("⚠ 没有可用的 terra 系列模型，本任务不派 terra 子代理")?,
        }
    }
    Ok(())
}

#[derive(Clone)]
struct ModelResolution {
    model: String,
    from_cache: Option<String>,
    catalog: Vec<Value>,
}

struct CodexModels<'a> {
    root: &'a Path,
    cwd: &'a Path,
    bin: Option<&'a Path>,
    stop: Option<&'a Path>,
    catalog: Option<Result<Vec<Value>>>,
}

impl<'a> CodexModels<'a> {
    fn new(store: &'a Store, cwd: &'a Path, bin: Option<&'a Path>, stop: Option<&'a Path>) -> Self {
        Self {
            root: &store.root,
            cwd,
            bin,
            stop,
            catalog: None,
        }
    }

    fn catalog(&mut self) -> &Result<Vec<Value>> {
        self.catalog.get_or_insert_with(|| {
            let binary = self
                .bin
                .map(Path::to_path_buf)
                .map_or_else(|| agent_executable("codex", self.cwd), Ok)?;
            let response = capture(&binary, self.cwd, &["debug", "models"], self.stop)?;
            let value: Value = serde_json::from_str(&response)?;
            let mut models = value["models"]
                .as_array()
                .context("无效的 Codex 模型目录")?
                .iter()
                .map(|model| {
                    let mut row = Map::new();
                    super::copy_fields(
                        model,
                        &mut row,
                        &[
                            "slug",
                            "visibility",
                            "supported_in_api",
                            "supported_reasoning_levels",
                            "upgrade",
                        ],
                    );
                    Value::Object(row)
                })
                .collect::<Vec<_>>();
            ensure!(
                models.iter().all(|model| model["slug"].is_string()),
                "无效的 Codex 模型目录"
            );
            models.sort_by(|left, right| {
                model_sort_key(right)
                    .cmp(&model_sort_key(left))
                    .then_with(|| left["slug"].as_str().cmp(&right["slug"].as_str()))
            });
            Ok(models)
        })
    }

    fn resolve(
        &mut self,
        request: &str,
        effort: &str,
        refresh: bool,
        report: &mut impl FnMut(&str) -> Result<()>,
    ) -> Result<ModelResolution> {
        let file = self.root.join(".model-resolution.json");
        let cache = read_json(&file).unwrap_or(Value::Null);
        let key = format!("{request}:{effort}");
        let cached = cached_codex(&cache, &key, request, effort);
        if let Some((resolved, at)) = &cached
            && !refresh
            && (0.0..86_400_000.0).contains(&(now_ms() - at))
        {
            return Ok(resolved.clone());
        }
        let root = self.root;
        let models = match self.catalog() {
            Ok(models) => models,
            Err(error) => {
                if let Some((resolved, _)) = cached {
                    report(&format!(
                        "⚠ 获取模型目录失败，沿用上次解析的 {}",
                        resolved.model
                    ))?;
                    return Ok(resolved);
                }
                bail!("无法获取 Codex 模型目录：{error:#}");
            }
        };
        let selected = select_codex(models, request, effort)
            .with_context(|| format!("没有可用的 {request} 系列模型（推理档位 {effort}）"))?;
        let selected_series = codex_series(selected)
            .map(|(series, _)| series)
            .unwrap_or_default();
        let listed = models
            .iter()
            .filter_map(|model| model["slug"].as_str())
            .collect::<HashSet<_>>();
        let recent = cache["recentModels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| {
                timestamp_ms(&model["lastSeenAt"])
                    .is_some_and(|at| (0.0..=604_800_000.0).contains(&(now_ms() - at)))
                    && model["slug"]
                        .as_str()
                        .is_some_and(|slug| !listed.contains(slug))
            })
            .cloned()
            .collect::<Vec<_>>();
        let newer = select_codex(&recent, selected_series, effort).filter(|recent| {
            codex_series(recent).map(|(_, version)| version)
                > codex_series(selected).map(|(_, version)| version)
        });
        let model = if let Some(newer) = newer {
            report(&format!(
                "⚠ 模型目录可能是旧的（缺 {newer}），沿用最近见过的 {newer}"
            ))?;
            newer
        } else {
            selected
        };
        let stored_entry =
            json!({"model": model, "effort": effort, "at": timestamp(), "catalog": models});
        let result = fs::create_dir_all(root).map_err(anyhow::Error::from).and_then(|_| with_lock(root, || {
            let previous = read_json(&file).unwrap_or(Value::Null);
            let mut previous_entries = previous["entries"].as_object().cloned().unwrap_or_default();
            if let (Some(previous_effort), Some(_)) =
                (previous["effort"].as_str(), previous["model"].as_str())
            {
                previous_entries
                    .entry(format!("best:{previous_effort}"))
                    .or_insert(previous.clone());
            }
            let now = timestamp();
            let mut seen = previous["recentModels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| {
                    let at = timestamp_ms(&entry["lastSeenAt"])?;
                    if !(0.0..=604_800_000.0).contains(&(now_ms() - at)) {
                        return None;
                    }
                    Some((entry["slug"].as_str()?.to_owned(), entry.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            for entry in models {
                if let Some(slug) = entry["slug"].as_str() {
                    let mut observed = entry.clone();
                    observed["lastSeenAt"] = json!(now);
                    seen.insert(slug.to_owned(), observed);
                }
            }
            previous_entries.insert(key, stored_entry);
            write_json(
                &file,
                &json!({"version": 2, "entries": previous_entries, "recentModels": seen.into_values().collect::<Vec<_>>()}),
            )
        }));
        if let Err(error) = result {
            eprintln!("无法保存 Foreman 模型解析缓存：{error:#}");
        }
        Ok(ModelResolution {
            model: model.into(),
            from_cache: None,
            catalog: models.clone(),
        })
    }
}

fn model_sort_key(model: &Value) -> (u8, Vec<u32>) {
    model["slug"]
        .as_str()
        .and_then(codex_series)
        .map(|(series, version)| (codex_tier_rank(series), version))
        .unwrap_or_default()
}

fn supports_codex_effort(model: &Value, effort: &str) -> bool {
    model["supported_reasoning_levels"]
        .as_array()
        .is_some_and(|levels| {
            levels
                .iter()
                .any(|level| level.as_str().or_else(|| level["effort"].as_str()) == Some(effort))
        })
}

fn print_models(models: &[Value], effort: &str) {
    println!("slug\t档位\t版本\t支持 {effort}\tvisibility");
    for model in models {
        let slug = model["slug"].as_str().unwrap_or_default();
        let version = slug
            .strip_prefix("gpt-")
            .and_then(|slug| slug.split_once('-'));
        let series = match version.map(|(_, series)| series) {
            Some("astra") => "前沿",
            Some("sol") => "主力",
            Some("terra") => "均衡",
            Some("luna") => "快速",
            _ => "未识别",
        };
        let number = version.map_or("-", |(version, _)| version);
        let support = if supports_codex_effort(model, effort) {
            "是"
        } else {
            "否"
        };
        println!(
            "{slug}\t{series}\t{number}\t{support}\t{}",
            super::shown(&model["visibility"])
        );
    }
}

pub(super) fn models(store: &Store, args: &[&str]) -> Result<i32> {
    let mut model = None;
    let mut effort = "high";
    let mut refresh = false;
    let mut agent = "codex";
    let mut args = args.iter().copied();
    while let Some(arg) = args.next() {
        let mut next = || {
            args.next()
                .filter(|arg| !arg.is_empty() && !arg.starts_with("--"))
                .context("命令缺少选项参数")
        };
        match arg {
            "--model" => model = Some(next()?),
            "--effort" => effort = next()?,
            "--agent" => agent = next()?,
            "--refresh-models" => refresh = true,
            _ => bail!("Unknown option {arg}\n{USAGE}"),
        }
    }
    match agent {
        "agy" => bail!("Antigravity 不使用 Codex 模型目录；请用 agy models 查看"),
        "dsh" => {
            bail!("DSH 不使用 Codex 模型目录；start --agent dsh --model 可用 flash、pro 或型号名")
        }
        "codex" => {}
        _ => bail!("--agent must be codex, dsh or agy"),
    }
    let cwd = std::env::current_dir()?;
    let mut lookup = CodexModels::new(store, &cwd, None, None);
    let requests = model.map_or_else(|| vec!["best", "sol"], |model| vec![model]);
    let mut report = |message: &str| {
        println!("{message}");
        Ok(())
    };
    for (i, request) in requests.into_iter().enumerate() {
        let alias = request.to_ascii_lowercase();
        if let Some(alias) = codex_alias(&alias) {
            let resolved = lookup.resolve(alias, effort, refresh, &mut report)?;
            if i == 0 {
                print_models(&resolved.catalog, effort);
            }
            let suffix = resolved.from_cache.map_or_else(
                || "（刚刚解析）".into(),
                |at| format!("（来自缓存，{at} 解析）"),
            );
            println!("{alias} → {}（推理档位 {effort}）{suffix}", resolved.model);
        } else {
            let catalog = lookup
                .catalog()
                .as_ref()
                .map_err(|error| anyhow::anyhow!("{error:#}"))?;
            print_models(catalog, effort);
            println!("{request} → {request}（推理档位 {effort}）（显式型号）");
        }
    }
    Ok(0)
}

fn capture(bin: &Path, cwd: &Path, args: &[&str], stop: Option<&Path>) -> Result<String> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut command = Command::new(bin);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    prepare_worker(&mut command);
    let mut child = Worker::new(command.spawn()?);
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.child.try_wait()? {
            break status;
        }
        if stop.is_some_and(Path::exists) || started.elapsed() >= MODEL_TIMEOUT {
            child.terminate();
            bail!("CLI 模型目录查询超时或已取消");
        }
        thread::sleep(STOP_POLL);
    };
    let read = |file: &mut File| -> io::Result<String> {
        file.seek(SeekFrom::Start(0))?;
        let mut data = Vec::new();
        file.take(32 * 1024 * 1024).read_to_end(&mut data)?;
        Ok(String::from_utf8_lossy(&data).into_owned())
    };
    ensure!(status.success(), "{}", read(&mut stderr)?.trim());
    Ok(read(&mut stdout)?)
}

#[derive(serde::Serialize)]
struct TurnFiles {
    prompt: PathBuf,
    events: PathBuf,
    stderr: PathBuf,
    last: PathBuf,
}

impl TurnFiles {
    fn new(dir: &Path, round: u64) -> Self {
        Self {
            prompt: dir.join(format!("turn-{round}.prompt.md")),
            events: dir.join(format!("turn-{round}.events.jsonl")),
            stderr: dir.join(format!("turn-{round}.stderr.log")),
            last: dir.join(format!("turn-{round}.last.txt")),
        }
    }
}

fn physical_path(path: &Path) -> PathBuf {
    let path = if let Ok(path) = path.canonicalize() {
        path
    } else if let Some((parent, name)) = path.parent().zip(path.file_name()) {
        parent
            .canonicalize()
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_owned())
    } else {
        path.to_owned()
    };
    #[cfg(windows)]
    if let Some(text) = path.to_str() {
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(drive) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(drive);
        }
    }
    path
}

fn worker_command(
    state: &Value,
    dir: &Path,
    files: &TurnFiles,
    bin: &Path,
    cwd: &Path,
    prompt: &str,
) -> Result<(Command, Option<Vec<u8>>)> {
    let agent = agent_id(state);
    let options = &state["options"];
    let model = required_str(options, "model")?;
    let effort = required_str(options, "effort")?;
    let full = options["fullAccess"].as_bool().unwrap_or(false);
    let permission = if full {
        "danger-full-access"
    } else {
        "workspace-write"
    };
    let resume = state["threadId"].as_str().filter(|id| !id.is_empty());
    let mut command = Command::new(bin);
    command
        .current_dir(cwd)
        .env("MATOU_FOREMAN_DEPTH", depth().saturating_add(1).to_string())
        .env("MATOU_FOREMAN_PARENT_AGENT", agent)
        .stdout(Stdio::piped())
        .stderr(File::create(&files.stderr)?);
    prepare_worker(&mut command);
    let input = match agent {
        "codex" => {
            command
                .args([
                    "exec",
                    "--skip-git-repo-check",
                    "--disable",
                    "unbounded_connection_retries",
                    "--json",
                    "-s",
                    permission,
                    "-C",
                ])
                .arg(cwd)
                .args(["-m", model, "-c"])
                .arg(format!("model_reasoning_effort={}", json!(effort)))
                .arg("-c")
                .arg(format!("service_tier={}", json!(codex_tier(options))))
                .args(["-c", "approval_policy=\"never\"", "--add-dir"])
                .arg(physical_path(dir))
                .arg("-o")
                .arg(physical_path(&files.last));
            if options["network"] == true {
                command.args(["-c", "sandbox_workspace_write.network_access=true"]);
            }
            if options["noProjectDoc"] == true {
                command.args(["-c", "project_doc_max_bytes=0"]);
            }
            append_add_dirs(&mut command, options);
            if let Some(thread) = resume {
                command.args(["resume", thread]);
            }
            command.arg("-");
            Some(prompt.as_bytes().to_vec())
        }
        "dsh" => {
            command
                .args(["--profile", "headless", "--patch"])
                .arg(dsh_patch(dir))
                .args([
                    "--json",
                    "--approval-policy",
                    "never",
                    "--permission-mode",
                    permission,
                    "--add-dir",
                ])
                .arg(dir);
            append_add_dirs(&mut command, options);
            if let Some(thread) = resume {
                command.args(["--session-id", thread]);
            }
            command.arg("-");
            Some(prompt.as_bytes().to_vec())
        }
        "agy" => {
            // cmd.exe cannot pass the multiline brief as a batch-file argument.
            // Stream one user event over stdin instead of flattening or shell-escaping it.
            let use_stdin = cfg!(windows)
                && (bin.extension().and_then(OsStr::to_str).is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat")
                }) || prompt.encode_utf16().count() >= 28_000);
            let input = agy_prompt(&mut command, prompt, use_stdin)?;
            command
                .args([
                    "--model",
                    model,
                    "--output-format",
                    "stream-json",
                    "--dangerously-skip-permissions",
                    "--add-dir",
                ])
                .arg(dir);
            append_add_dirs(&mut command, options);
            if !full {
                command.arg("--sandbox");
            }
            if let Some(thread) = resume {
                command.args(["--conversation", thread]);
            }
            input
        }
        _ => bail!("未知 Agent：{agent}"),
    };
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    Ok((command, input))
}

fn agy_prompt(command: &mut Command, prompt: &str, use_stdin: bool) -> Result<Option<Vec<u8>>> {
    if !use_stdin {
        command.args(["-p", prompt]);
        return Ok(None);
    }
    command.args(["--input-format", "stream-json"]);
    let mut line = serde_json::to_vec(&json!({"event": "user", "message": {"content": prompt}}))?;
    line.push(b'\n');
    Ok(Some(line))
}

fn append_add_dirs(command: &mut Command, options: &Value) {
    if let Some(dirs) = options["addDirs"].as_array() {
        for dir in dirs.iter().filter_map(Value::as_str) {
            command.arg("--add-dir").arg(dir);
        }
    }
}

fn protocol_footer(store: &Store, dir: &Path, state: &Value) -> String {
    let task = state["task"].as_str().unwrap_or_default();
    let argument = task_argument(task);
    let cwd = state["options"]["cd"].as_str().unwrap_or_default();
    let options = &state["options"];
    let extra = options["addDirs"]
        .as_array()
        .map(|dirs| dirs.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!("（另允许 {}）", extra.join("、"))
    };
    let ask = options["askWaitMin"].as_f64().unwrap_or(10.0);
    let prefix = store.command_prefix();
    let question = format!("{prefix}mt-foreman ask {argument} --message \"问题.md\"");
    let mut text = format!(
        r#"

---
## 运行约定（由 Foreman 附加）
- 你负责把交代目标落地。创意方向、产品取舍和审美以交代为准。影响用户体验、需要新增核心依赖或涉及不可逆迁移的未决事项，先提问。
- 工具刚启动时未出现，等 30 秒再检查。
- 写入仅限 {cwd}{extra}。不执行 git push。仅在交代明确要求时提交到指定分支。
- 将背景、可选做法和你的倾向写入 UTF-8 文件「问题.md」，再运行以下命令。命令超时至少设为 {} 分钟。它最多等待 {ask} 分钟。
  {question}
  遇到可以边做边问的情况，加 --no-wait。
- 每完成一步运行 {prefix}mt-foreman inbox {argument} 查看回答和留言，并向 {} 追加进度。
- 长命令输出记入 {}。自行做的取舍和原因记入 {}。
- 到交代规定的关卡就停下。最终回复依次写已完成、产物路径、自检结果、我自己做的决定、未解决问题。
"#,
        ask + 5.0,
        dir.join("progress.log").display(),
        dir.join("logs").display(),
        dir.join("decisions.md").display()
    );
    let mut peers = options["subDispatchPeers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !options["subDispatchPeers"].is_array()
        && agent_id(state) == "codex"
        && options["allowDshSubDispatch"] != false
    {
        peers.push(json!({"id":"dsh", "name":"DSH", "role":"负责前端 UI 设计与实现"}));
    }
    let agy_orchestrator = agent_id(state) == "agy"
        && options["model"]
            .as_str()
            .is_some_and(|model| model.to_ascii_lowercase().contains("orchestrator"));
    let terra = options["terraModel"]
        .as_str()
        .filter(|_| agent_id(state) == "codex");
    if let Some(terra) = terra {
        let effort = options["effort"].as_str().unwrap_or("high");
        text.push_str(&format!("- 点击软件界面或操作桌面的任务使用 spawn_agent，参数 model={terra}、reasoning_effort={effort}、fork_turns=none。其他内部子代理禁用。\n"));
    } else if !agy_orchestrator {
        text.push_str("- 不要使用 agent-foreman 技能，不派内部子代理。\n");
    }
    if agent_id(state) == "agy" {
        text.push_str(
            "- 通过上面的 ask 命令向人提问。不要使用 ask_question，监工无法在该界面回答。\n",
        );
    }
    if depth() == 0 {
        for peer in peers {
            let Some(id) = peer["id"]
                .as_str()
                .filter(|id| agent_executable(id, Path::new(cwd)).is_ok())
            else {
                continue;
            };
            let name = peer["name"].as_str().unwrap_or(id);
            let role = peer["role"]
                .as_str()
                .filter(|role| !role.trim().is_empty())
                .unwrap_or("按默认职责");
            let suffix = format!("-{id}-01");
            let mut peer_task = task
                .chars()
                .take(40 - suffix.chars().count())
                .collect::<String>();
            peer_task.push_str(&suffix);
            let peer_task = task_argument(&peer_task);
            let command = format!(
                "{prefix}mt-foreman start {peer_task} --agent {id} --cd \"工作目录\" --brief \"交代书.md\""
            );
            text.push_str(&format!("- {name} 的职责范围（{role}）用以下命令派活。交代它先读 {} 下按序号保存的全部原始交代，再执行。只允许跨执行者转派一层。\n  {command}\n", dir.display()));
        }
    }
    text
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Completed,
    Fatal,
    Retryable,
    Stalled,
    Timeout,
    Stopped,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Fatal => "fatal",
            Self::Retryable => "retryable",
            Self::Stalled => "stalled",
            Self::Timeout => "timeout",
            Self::Stopped => "stopped",
        }
    }
}

struct TurnResult {
    outcome: Outcome,
    error: Option<String>,
    exit: Value,
}

impl TurnResult {
    fn failed(outcome: Outcome, error: impl Into<String>, exit: Value) -> Self {
        Self {
            outcome,
            error: Some(error.into()),
            exit,
        }
    }

    fn ended(outcome: Outcome, exit: Value) -> Self {
        Self {
            outcome,
            error: None,
            exit,
        }
    }
}

fn run_supervisor(
    store: &Store,
    dir: &Path,
    stop: &Path,
    task: &str,
    state: &mut Value,
    prompt: &str,
) -> Result<i32> {
    publish_status(state);
    if stop.exists() {
        return finish(dir, task, state, "stopped", 5, None);
    }
    let cwd = PathBuf::from(required_str(&state["options"], "cd")?);
    let agent = agent_id(state).to_owned();
    let executable = agent_executable_with_path(&agent, &cwd);
    if let Ok((bin, _)) = &executable {
        let result = match agent.as_str() {
            "codex" => configure_codex(store, dir, bin, &cwd, stop, &mut state["options"]),
            "dsh" => configure_dsh(dir, &mut state["options"]),
            "agy" => configure_agy(dir, bin, &cwd, stop, &mut state["options"]),
            _ => unreachable!(),
        };
        if stop.exists() {
            return finish(dir, task, state, "stopped", 5, None);
        }
        if let Err(error) = result {
            return finish(dir, task, state, "failed", 1, Some(format!("{error:#}")));
        }
    }
    let retries = state["options"]["retries"].as_u64().unwrap_or(3);
    for attempt in 0..=retries {
        if stop.exists() {
            return finish(dir, task, state, "stopped", 5, None);
        }
        let round = turns(state).len() as u64 + 1;
        let files = TurnFiles::new(dir, round);
        let turn_prompt = if attempt == 0 {
            format!("{prompt}{}", protocol_footer(store, dir, state))
        } else {
            format!(
                "上一轮因连接中断或超时没有完成。请先检查工作目录和 progress.log 的现状，不要重做已经完成的部分，然后继续完成下面这条要求：\n\n{prompt}{}",
                protocol_footer(store, dir, state)
            )
        };
        fs::write(&files.prompt, &turn_prompt)?;
        let resumed = state["threadId"]
            .as_str()
            .is_some_and(|value| !value.is_empty());
        let cwd_title = if resumed { "接续会话" } else { "新会话" };
        let now = json!(timestamp());
        state["turns"]
            .as_array_mut()
            .context("任务轮次记录无效")?
            .push(json!({"n": round, "resume": resumed, "startedAt": now, "files": files}));
        state["activity"] = json!("启动中");
        state["lastActivityAt"] = now.clone();
        state["activityAt"] = now;
        state["intent"] = Value::Null;
        state["generatingSince"] = Value::Null;
        state["networkSampling"] = json!(false);
        let model = state["options"]["model"].as_str().unwrap_or_default();
        let effort = if agent == "agy" {
            agy_effort_suffix(model)
        } else {
            state["options"]["effort"].as_str()
        };
        let details = [
            Some(cwd_title),
            (agent != "codex").then(|| agent_name(state)),
            Some(model),
            effort,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        feed(dir, &format!("▶ {task} 第 {round} 轮开始（{details}）"))?;
        save_state(dir, state)?;
        let git_before = git_changes(state);
        let result = match &executable {
            Ok((bin, recovered_path)) => {
                let launch = worker_command(state, dir, &files, bin, &cwd, &turn_prompt).map(
                    |(mut command, input)| {
                        if let Some(path) = recovered_path {
                            command.env("PATH", path);
                        }
                        (command, input)
                    },
                );
                run_worker(dir, stop, state, &files, launch, round, attempt == 0)?
            }
            Err(error) => {
                File::create(&files.events)?;
                fs::write(&files.stderr, format!("{error:#}\n"))?;
                TurnResult::failed(
                    Outcome::Fatal,
                    format!("spawn failed: {error:#}"),
                    Value::Null,
                )
            }
        };
        if result.outcome == Outcome::Completed
            && !fs::read_to_string(&files.last)
                .unwrap_or_default()
                .contains("我自己做的决定")
        {
            feed(dir, &format!("⚠ {task} 的回复缺少「我自己做的决定」"))?;
        }
        let scope = out_of_scope(dir, state, git_before);
        if !scope.is_empty() {
            feed(
                dir,
                &format!(
                    "⚠ {task} 工作目录外出现新改动（也可能来自其他会话，请核对）：{}",
                    scope.join(" | ")
                ),
            )?;
        }
        let turn = state["turns"].as_array_mut().unwrap().last_mut().unwrap();
        turn["endedAt"] = json!(timestamp());
        turn["exit"] = result.exit;
        turn["result"] = json!(result.outcome.as_str());
        if let Some(error) = &result.error {
            turn["error"] = json!(error);
        }
        if !scope.is_empty() {
            turn["outOfScope"] = json!(scope);
        }
        eprintln!("turn {round} ended: {}", result.outcome.as_str());
        state["pid"] = Value::Null;
        save_state(dir, state)?;
        match result.outcome {
            Outcome::Completed => return finish(dir, task, state, "completed", 0, None),
            Outcome::Fatal => return finish(dir, task, state, "failed", 2, result.error),
            Outcome::Timeout => return finish(dir, task, state, "timeout", 4, result.error),
            Outcome::Stopped => return finish(dir, task, state, "stopped", 5, None),
            Outcome::Stalled | Outcome::Retryable if attempt == retries => {
                return finish(
                    dir,
                    task,
                    state,
                    "failed",
                    3,
                    Some(format!(
                        "gave up after {} attempts: {}",
                        attempt + 1,
                        result.error.unwrap_or_default()
                    )),
                );
            }
            Outcome::Stalled | Outcome::Retryable => {}
        }
        let wait_min = state["options"]["backoffMin"]
            .as_array()
            .and_then(|times| times.get(attempt as usize).or_else(|| times.last()))
            .and_then(Value::as_f64)
            .unwrap_or(2.0);
        let reason = if result.outcome == Outcome::Stalled {
            "卡住".to_owned()
        } else {
            format!(
                "繁忙或断线：{}",
                short(result.error.as_deref().unwrap_or_default(), 120)
            )
        };
        feed(
            dir,
            &format!(
                "⚠ {task} {reason}；{wait_min} 分钟后第 {}/{retries} 次重试",
                attempt + 1
            ),
        )?;
        state["status"] = json!("waiting-retry");
        state["activity"] = json!(format!(
            "等待重试 {}/{retries}（{wait_min} 分钟后）",
            attempt + 1
        ));
        save_state(dir, state)?;
        let delay = Duration::try_from_secs_f64(wait_min * 60.0).unwrap_or_default();
        let started = Instant::now();
        let mut status = Instant::now();
        while !stop.exists() {
            let elapsed = started.elapsed();
            if elapsed >= delay {
                break;
            }
            if status.elapsed() >= Duration::from_secs(5) {
                publish_status(state);
                status = Instant::now();
            }
            thread::sleep(STOP_POLL.min(delay - elapsed));
        }
    }
    unreachable!()
}

fn save_state(dir: &Path, state: &Value) -> Result<()> {
    write_json(&dir.join("state.json"), state)?;
    publish_status(state);
    Ok(())
}

fn publish_status(state: &Value) {
    let options = &state["options"];
    let Some(dir) = options["agentStatusDir"]
        .as_str()
        .map(Path::new)
        .filter(|dir| dir.is_absolute() && dir.is_dir())
    else {
        return;
    };
    let Some(task) = state["task"].as_str() else {
        return;
    };
    let agent = agent_name(state);
    let label_prefix = format!("{agent}：{task}（");
    let id = if task
        .bytes()
        .all(|ch| ch.is_ascii_alphanumeric() || b"._-".contains(&ch))
    {
        task.to_owned()
    } else {
        let hash = format!("{:x}", Sha256::digest(task.as_bytes()));
        hash[..12].to_owned()
    };
    let file = dir.join(format!("codex-{id}.json"));
    let existing = if file.exists() || task.is_ascii() {
        None
    } else {
        fs::read_dir(dir).ok().and_then(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| {
                    path.extension() == Some(OsStr::new("json"))
                        && path
                            .file_name()
                            .and_then(OsStr::to_str)
                            .is_some_and(|name| name.starts_with("codex-"))
                        && read_json(path)
                            .ok()
                            .and_then(|value| {
                                value["label"]
                                    .as_str()
                                    .map(|label| label.starts_with(&label_prefix))
                            })
                            .unwrap_or(false)
                })
        })
    };
    let activity = state["activity"].as_str().unwrap_or("启动中");
    let (status, detail) = match state["status"].as_str() {
        Some("completed") => ("completed", "本轮完成".into()),
        Some("stopped") => ("stopped", "已手动停止".into()),
        Some("failed") => (
            "error",
            state["error"].as_str().unwrap_or_default().to_owned(),
        ),
        Some("timeout") => ("error", "本轮超时".into()),
        Some("waiting-retry") => ("waiting", activity.to_owned()),
        Some("running") if state["waitingQuestionText"].is_string() => (
            "waiting",
            format!(
                "等待回答：{}",
                state["waitingQuestionText"].as_str().unwrap_or_default()
            ),
        ),
        Some("running") if state["warningAt"].is_string() => ("warning", activity.to_owned()),
        _ => ("running", activity.to_owned()),
    };
    let model = options["model"].as_str().unwrap_or_default();
    let effort = if agent_id(state) == "agy" {
        agy_effort_suffix(model)
    } else {
        options["effort"].as_str()
    };
    let effort = effort.map(|effort| {
        format!(
            "{}推理",
            matou_native_core::dispatch_roles::effort_label_zh(effort)
        )
    });
    let tier = (agent_id(state) == "codex")
        .then(|| codex_tier(options))
        .map(|tier| match tier.as_str() {
            "default" => "普通".into(),
            "fast" => "快速".into(),
            "ultrafast" => "超快".into(),
            "priority" => "优先".into(),
            "flex" => "弹性".into(),
            _ => tier,
        });
    let specs = [
        (model != "best" && !model.is_empty()).then_some(model),
        effort.as_deref(),
        tier.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let mut value = json!({
        "version": 1, "label": format!("{label_prefix}{specs}）"),
        "state": status, "detail": short(&detail, 120),
        "startedAt": state["runStartedAt"], "updatedAt": timestamp()
    });
    if state["supervisorPid"].as_u64().is_some_and(|pid| pid > 0) {
        value["pid"] = state["supervisorPid"].clone();
    }
    let _ = (|| -> Result<()> {
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut temp, &value)?;
        temp.flush()?;
        temp.persist(existing.unwrap_or(file))?;
        Ok(())
    })();
}

fn git_changes(state: &Value) -> Option<HashSet<String>> {
    if state["options"]["fullAccess"] != true {
        return None;
    }
    let root = state["options"]["gitRoot"].as_str()?;
    let output = git_command(Path::new(root))
        .ok()?
        .args([
            "status",
            "--porcelain",
            "-z",
            "--no-renames",
            "--untracked-files=all",
        ])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .split('\0')
            .filter(|line| line.len() > 3)
            .map(str::to_owned)
            .collect()
    })
}

fn out_of_scope(dir: &Path, state: &Value, before: Option<HashSet<String>>) -> Vec<String> {
    let Some((before, after)) = before.zip(git_changes(state)) else {
        return Vec::new();
    };
    let Some(root) = state["options"]["gitRoot"].as_str().map(Path::new) else {
        return Vec::new();
    };
    let cwd = state["options"]["cd"].as_str().map(Path::new);
    let extras = state["options"]["addDirs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(Path::new);
    let allowed = std::iter::once(dir)
        .chain(cwd)
        .chain(extras)
        .filter_map(|path| path.strip_prefix(root).ok())
        .collect::<Vec<_>>();
    let mut changed = after
        .difference(&before)
        .filter(|line| {
            !allowed
                .iter()
                .any(|allowed| Path::new(&line[3..]).starts_with(allowed))
        })
        .cloned()
        .collect::<Vec<_>>();
    changed.sort_unstable();
    changed
}

#[derive(Default)]
struct Events {
    result_seen: bool,
    completed: bool,
    agy_response: bool,
    response_end: Option<Instant>,
    error: String,
    final_error: String,
    latest_reply: String,
    agy_steps: BTreeMap<u64, String>,
}

fn handle_event(
    line: &[u8],
    state: &mut Value,
    files: &TurnFiles,
    events: &mut Events,
) -> Result<()> {
    let Ok(event) = serde_json::from_slice::<Value>(line) else {
        return Ok(());
    };
    let agent = agent_id(state).to_owned();
    let kind = event["type"].as_str().unwrap_or_default();
    let agy_event = event["event"].as_str().unwrap_or_default();
    let session_id = match (agent.as_str(), kind, agy_event) {
        ("codex", "thread.started", _) => event["thread_id"].as_str(),
        ("dsh", "session", _) => event["sessionId"].as_str(),
        ("agy", _, "init") => event["conversation_id"].as_str(),
        _ => None,
    };
    if let Some(id) = session_id
        && state["threadId"].as_str().is_none_or(str::is_empty)
    {
        state["threadId"] = json!(id);
    }
    let mut activity = None;
    match agent.as_str() {
        "codex" => match kind {
            "turn.completed" => events.completed = true,
            "turn.failed" => {
                events.final_error = event["error"]["message"]
                    .as_str()
                    .unwrap_or("turn.failed")
                    .into()
            }
            "error" => events.error = event["message"].as_str().unwrap_or_default().into(),
            "item.started" | "item.updated" | "item.completed" => {
                let item = &event["item"];
                let item_type = item["type"].as_str().unwrap_or_default();
                match item_type {
                    "agent_message" if kind == "item.completed" => {
                        if let Some(text) = item["text"].as_str() {
                            events.latest_reply = text.to_owned();
                            activity = Some(explain(state, text));
                        }
                    }
                    "command_execution" if kind != "item.updated" => {
                        let action = if kind == "item.started" {
                            "运行"
                        } else {
                            "跑完"
                        };
                        activity = Some(format!(
                            "{action} {}",
                            short(item["command"].as_str().unwrap_or_default(), 60)
                        ));
                    }
                    "reasoning" => activity = Some("思考中".into()),
                    "file_change" => activity = Some("修改文件".into()),
                    "web_search" => {
                        activity = Some(format!(
                            "搜索 {}",
                            short(item["query"].as_str().unwrap_or_default(), 32)
                        ))
                    }
                    "mcp_tool_call" => {
                        activity = Some(format!(
                            "调用工具 {}",
                            short(item["tool"].as_str().unwrap_or_default(), 48)
                        ))
                    }
                    _ => {}
                }
            }
            _ => {}
        },
        "dsh" => match kind {
            "text" => {
                if let Some(text) = event["text"].as_str() {
                    activity = Some(explain(state, text));
                }
            }
            "final" => {
                events.result_seen = true;
                events.completed = event["reason"]["kind"] == "completed";
                if events.completed {
                    let text = event["text"]
                        .as_str()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("（DSH 没有给出文字回复）");
                    fs::write(&files.last, text)?;
                } else {
                    let reason = event["reason"].get("error").unwrap_or(&event["reason"]);
                    events.final_error = reason
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| reason.to_string());
                }
            }
            "error" => events.error = event["message"].as_str().unwrap_or_default().into(),
            "tool_call" | "tool_result" => activity = Some("运行工具".into()),
            _ => {}
        },
        "agy" => match agy_event {
            "step_update" => {
                let step = &event["step_update"];
                if step["step_type"] == "agent_response"
                    && let Some(delta) = step["text_delta"].as_str()
                {
                    let text = events
                        .agy_steps
                        .entry(step["step_index"].as_u64().unwrap_or_default())
                        .or_default();
                    text.push_str(delta);
                    activity = Some(explain(state, text));
                }
            }
            "result" if !events.result_seen => {
                events.result_seen = true;
                events.response_end = Some(Instant::now());
                let result = &event["result"];
                let status = result["status"]
                    .as_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                events.completed = ["success", "completed", "ok"].contains(&status.as_str());
                let response = result["response"]
                    .as_str()
                    .filter(|value| !value.trim().is_empty());
                events.agy_response = response.is_some();
                if events.agy_response || events.completed {
                    fs::write(
                        &files.last,
                        response.unwrap_or("（Antigravity 没有给出文字回复）"),
                    )?;
                }
                if !events.completed {
                    events.final_error =
                        if let Some(error) = result.get("error").filter(|error| !error.is_null()) {
                            error
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| error.to_string())
                        } else if status.is_empty() {
                            "Antigravity result without status".into()
                        } else {
                            status
                        };
                }
                if state["threadId"].as_str().is_none_or(str::is_empty)
                    && let Some(id) = result["conversation_id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                {
                    state["threadId"] = json!(id);
                }
            }
            _ => {}
        },
        _ => {}
    }
    if let Some(activity) = activity {
        if state["waitingQuestionText"].is_null() {
            state["activity"] = json!(activity);
            state["activityAt"] = json!(timestamp());
        }
    }
    Ok(())
}

fn explain(state: &mut Value, text: &str) -> String {
    let summary = short(text, 100);
    state["intent"] = json!(summary);
    format!("说明：{summary}")
}

fn run_worker(
    dir: &Path,
    stop: &Path,
    state: &mut Value,
    files: &TurnFiles,
    launch: Result<(Command, Option<Vec<u8>>)>,
    round: u64,
    first_attempt: bool,
) -> Result<TurnResult> {
    let mut events_log = File::create(&files.events)?;
    let (mut command, input) = match launch {
        Ok(launch) => launch,
        Err(error) => {
            return Ok(TurnResult::failed(
                Outcome::Fatal,
                format!("spawn failed: {error:#}"),
                Value::Null,
            ));
        }
    };
    let spawn = command.spawn();
    let mut worker = match spawn {
        Ok(child) => Worker::new(child),
        Err(error) => {
            fs::write(&files.stderr, format!("{error}\n"))?;
            return Ok(TurnResult::failed(
                Outcome::Fatal,
                format!("spawn failed: {error}"),
                Value::Null,
            ));
        }
    };
    let pid = worker.child.id();
    #[cfg(target_os = "macos")]
    let mut network = mac_network::Probe::new(pid);
    #[cfg(target_os = "macos")]
    {
        state["networkSampling"] = json!(network.is_some());
    }
    eprintln!("turn {round} started (pid {pid})");
    state["pid"] = json!(pid);
    state["status"] = json!("running");
    save_state(dir, state)?;
    if first_attempt {
        notify_started(dir, state, round);
        save_state(dir, state)?;
    }
    if let Some((mut stdin, input)) = worker.child.stdin.take().zip(input) {
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let mut stdout = worker.child.stdout.take().context("无法读取 Agent 输出")?;
    let (tx, rx) = mpsc::sync_channel(32);
    thread::spawn(move || {
        let mut chunk = [0u8; 16_384];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    if tx.send(chunk[..count].to_vec()).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });

    let started = Instant::now();
    let warn = period(&state["options"], "warnMin", 3.0);
    let idle = period(&state["options"], "idleMin", 10.0);
    let limit = period(&state["options"], "maxMin", 45.0);
    let result_grace = Duration::from_millis(
        std::env::var("FOREMAN_AGY_RESULT_EXIT_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|&value| value > 0)
            .unwrap_or(10_000),
    );
    let task = state["task"].as_str().unwrap_or_default().to_owned();
    let mut activity = LastActivity::new();
    let mut last_mail_scan = None;
    let mut warning = false;
    let mut killed = None;
    let mut dispatch = None;
    let mut closed = false;
    let mut exit = None;
    let mut exit_time = None;
    let mut buffer = Vec::new();
    let mut events = Events::default();
    let mut defer_status = Instant::now();
    #[cfg(target_os = "macos")]
    let mut output_version = 0u64;

    loop {
        let mut updated = false;
        if !closed {
            let wait = if exit.is_some() {
                Duration::from_millis(25)
            } else {
                STOP_POLL
            };
            match rx.recv_timeout(wait) {
                Ok(chunk) => {
                    events_log.write_all(&chunk)?;
                    activity.observe(SystemTime::now());
                    #[cfg(target_os = "macos")]
                    {
                        output_version = output_version.wrapping_add(1);
                        if let Some(probe) = &mut network {
                            probe.end_generation(state);
                        }
                    }
                    consume_events(&mut buffer, &chunk, state, files, &mut events)?;
                    updated = true;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => closed = true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        } else if exit.is_none() {
            thread::sleep(STOP_POLL);
        }
        if exit.is_none() {
            exit = worker.child.try_wait()?;
            if exit.is_some() {
                exit_time = Some(Instant::now());
            }
        }
        if exit.is_some()
            && (closed
                || exit_time.is_some_and(|ended| ended.elapsed() >= Duration::from_millis(500)))
        {
            break;
        }

        if last_mail_scan.is_none_or(|last: Instant| last.elapsed() >= Duration::from_millis(750)) {
            let (mail_changed, heartbeat) = scan_mailbox(dir, state)?;
            updated |= mail_changed;
            if let Some(heartbeat) = heartbeat {
                activity.observe(heartbeat);
            }
            if let Some(mtime) = newest_progress(dir) {
                activity.observe(mtime);
            }
            last_mail_scan = Some(Instant::now());
        }
        if exit.is_none() && stop.exists() {
            killed = Some(Outcome::Stopped);
        }
        #[cfg(target_os = "macos")]
        if exit.is_none()
            && killed.is_none()
            && !events.result_seen
            && let Some(probe) = &mut network
        {
            updated |= probe.poll(output_version, state, &mut activity);
        }
        if exit.is_none() && killed.is_none() && dispatch.is_none() {
            if let Some(result) = events.response_end {
                if result.elapsed() >= result_grace {
                    dispatch = Some(());
                }
            } else {
                let quiet = activity.quiet();
                let network_note = if state["networkSampling"] == true {
                    "（网络也没有数据）"
                } else {
                    ""
                };
                if warning && quiet < warn {
                    let current = state["activity"].as_str().unwrap_or("启动中");
                    feed(dir, &format!("↺ {task} 恢复活动：{current}"))?;
                    warning = false;
                    state["warningAt"] = Value::Null;
                    updated = true;
                } else if !warning && quiet > warn {
                    feed(
                        dir,
                        &format!(
                            "⏸ {task} 已 {}没有动静{network_note}",
                            quiet_description(quiet)
                        ),
                    )?;
                    warning = true;
                    state["warningAt"] = json!(timestamp());
                    updated = true;
                }
                if started.elapsed() >= limit {
                    feed(
                        dir,
                        &format!(
                            "⏱ {task} 本轮超过 {} 分钟，已停下，等待处理",
                            state["options"]["maxMin"]
                        ),
                    )?;
                    killed = Some(Outcome::Timeout);
                } else if quiet >= idle {
                    feed(
                        dir,
                        &format!(
                            "✖ {task} 连续 {}没有动静{network_note}，已结束本轮",
                            quiet_description(quiet)
                        ),
                    )?;
                    killed = Some(Outcome::Stalled);
                }
            }
        }
        if exit.is_none() && (killed.is_some() || dispatch.is_some()) {
            worker.terminate();
        }
        if updated || defer_status.elapsed() >= Duration::from_secs(1) {
            if activity.quiet() <= Duration::from_secs(1) {
                state["lastActivityAt"] = json!(timestamp());
            }
            save_state(dir, state)?;
            defer_status = Instant::now();
        }
    }
    if !buffer.is_empty() {
        handle_event(&buffer, state, files, &mut events)?;
    }
    #[cfg(target_os = "macos")]
    if let Some(mut probe) = network {
        probe.end_generation(state);
    }
    let _ = scan_mailbox(dir, state)?;
    state["waitingQuestions"] = json!([]);
    state["waitingQuestionText"] = Value::Null;
    state["warningAt"] = Value::Null;
    let exit = exit.context("Agent 未返回退出状态")?;
    if agent_id(state) == "codex" && !files.last.exists() && !events.latest_reply.is_empty() {
        fs::write(&files.last, &events.latest_reply)?;
    }
    let result = classify(state, dir, files, events, killed, exit)?;
    Ok(result)
}

fn consume_events(
    buffer: &mut Vec<u8>,
    chunk: &[u8],
    state: &mut Value,
    files: &TurnFiles,
    events: &mut Events,
) -> Result<()> {
    let mut lines = chunk.split_inclusive(|&byte| byte == b'\n');
    if let Some(first) = lines.next() {
        buffer.extend_from_slice(first);
        if first.ends_with(b"\n") {
            handle_event(buffer, state, files, events)?;
            buffer.clear();
        }
    }
    for line in lines {
        if line.ends_with(b"\n") {
            handle_event(line, state, files, events)?;
        } else {
            buffer.extend_from_slice(line);
        }
    }
    Ok(())
}

fn period(options: &Value, key: &str, fallback: f64) -> Duration {
    options[key]
        .as_f64()
        .filter(|&value| value > 0.0)
        .and_then(|value| Duration::try_from_secs_f64(value * 60.0).ok())
        .unwrap_or_else(|| Duration::from_secs_f64(fallback * 60.0))
}

fn quiet_description(quiet: Duration) -> String {
    let seconds = quiet.as_secs();
    if seconds < 60 {
        format!("{seconds} 秒")
    } else {
        format!("{} 分钟", seconds / 60)
    }
}

struct LastActivity(SystemTime);

impl LastActivity {
    fn new() -> Self {
        Self(SystemTime::now())
    }

    fn observe(&mut self, observed: SystemTime) {
        self.0 = self.0.max(observed.min(SystemTime::now()));
    }

    fn quiet(&self) -> Duration {
        self.0.elapsed().unwrap_or_default()
    }
}

fn newest_progress(dir: &Path) -> Option<SystemTime> {
    let latest = |path: &Path| path.metadata().and_then(|info| info.modified()).ok();
    let logs = dir.join("logs");
    let nested = fs::read_dir(&logs)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| latest(&entry.path()));
    [latest(&dir.join("progress.log")), latest(&logs)]
        .into_iter()
        .flatten()
        .chain(nested)
        .max()
}

fn mark_mail(state: &mut Value, key: &str) -> bool {
    if state["mailboxSeen"][key] == true {
        return false;
    }
    state["mailboxSeen"][key] = json!(true);
    true
}

fn scan_mailbox(dir: &Path, state: &mut Value) -> Result<(bool, Option<SystemTime>)> {
    let task = state["task"].as_str().unwrap_or_default().to_owned();
    let mut changed = false;
    let round = turns(state).len() as u64;
    let mut waiting = Vec::new();
    let mut waiting_text = Value::Null;
    let mut newest = None;
    for question in numbered_mail(dir, "questions", "q-") {
        let Some(number) = question["n"].as_u64() else {
            continue;
        };
        let qid = format!("q-{number}");
        let aid = format!("a-{number}");
        let text = question["text"].as_str().unwrap_or_default();
        if mark_mail(state, &qid) {
            let preview = if let Some((at, _)) = text.char_indices().nth(3000) {
                format!(
                    "{}…（完整内容见 {}）",
                    &text[..at],
                    mailbox_path(dir, "questions", &qid).display()
                )
            } else {
                text.to_owned()
            };
            let note = if question["noWait"] == true {
                "（不阻塞）"
            } else {
                ""
            };
            feed(dir, &format!("❓ {task} 提问 #{number}{note}：{preview}"))?;
            changed = true;
        }
        let outcome = read_json(&mailbox_path(dir, "outcomes", &qid)).ok();
        if let Some(outcome) = &outcome
            && outcome["status"] == "timedout"
            && state["mailboxSeen"][format!("timeout-{qid}")].is_null()
            && mark_mail(state, &format!("timeout-{number}"))
        {
            feed(
                dir,
                &format!(
                    "⏳ {task} #{number} 等了 {} 分钟没有回答，{} 按约定处理",
                    question["waitMin"],
                    agent_name(state)
                ),
            )?;
            changed = true;
        }
        let answered = mailbox_path(dir, "answers", &aid).exists();
        if answered && mark_mail(state, &aid) {
            feed(dir, &format!("💬 {task} #{number} 已收到回答"))?;
            changed = true;
        }
        if answered
            || outcome.is_some()
            || question["noWait"] == true
            || question["turn"].as_u64() != Some(round)
        {
            continue;
        }
        let Some(heartbeat) = read_json(&mailbox_path(dir, "heartbeats", &qid)).ok() else {
            continue;
        };
        let Some(at) =
            timestamp_ms(&heartbeat["at"]).filter(|&at| now_ms() - at < 7_500.0 && at >= 0.0)
        else {
            continue;
        };
        if !pid_alive(&heartbeat["pid"]) {
            continue;
        }
        let observed = UNIX_EPOCH + Duration::from_millis(at as u64);
        newest = Some(newest.map_or(observed, |last: SystemTime| last.max(observed)));
        if waiting.is_empty() {
            waiting_text = json!(short(text, 100));
        }
        waiting.push(number);
    }
    if let Ok(notes) = fs::read_dir(dir.join("notes")) {
        for note in notes.filter_map(Result::ok) {
            let name = note.file_name();
            let Some(name) = name
                .to_str()
                .filter(|name| name.starts_with("n-") && name.ends_with(".json"))
            else {
                continue;
            };
            if mark_mail(state, &format!("note-{name}")) {
                feed(dir, &format!("✉ {task} 已收到留言"))?;
                changed = true;
            }
        }
    }
    let waiting = json!(waiting);
    if state["waitingQuestions"] != waiting || state["waitingQuestionText"] != waiting_text {
        if let Some(number) = waiting.get(0).and_then(Value::as_u64) {
            state["activity"] = json!(format!("等待回答 #{number}"));
        } else if state["waitingQuestionText"].is_string() {
            state["activity"] = json!("继续工作");
        }
        state["waitingQuestions"] = waiting;
        state["waitingQuestionText"] = waiting_text;
        changed = true;
    }
    Ok((changed, newest))
}

fn stderr_tail(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let size = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(size.saturating_sub(32_768)));
    let mut bytes = Vec::new();
    let _ = file.take(32_768).read_to_end(&mut bytes);
    let decoded = String::from_utf8_lossy(&bytes);
    let mut lines = decoded
        .lines()
        .rev()
        .filter(|line| !line.contains("failed to load skill"))
        .take(15)
        .collect::<Vec<_>>();
    lines.reverse();
    lines.join("\n")
}

fn classify(
    state: &Value,
    dir: &Path,
    files: &TurnFiles,
    events: Events,
    killed: Option<Outcome>,
    exit: ExitStatus,
) -> Result<TurnResult> {
    static FATAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b40[13]\b|unauthori[sz]ed|not logged in|authentication|required credentials|please (log|sign) in|invalid api key|sandbox.*denied").unwrap()
    });
    static RETRYABLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)capacity|\b429\b|\b5\d\d\b|overloaded|rate.?limit|reconnect|stream disconnected|timed? ?out|AI 专线|ECONN|ETIMEDOUT|network|connection").unwrap()
    });
    static QUOTA: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)RESOURCE_EXHAUSTED|quota|Resets in|额度").unwrap());
    let exit_value = worker_exit(exit);
    let exit_label = exit_value
        .as_str()
        .map_or_else(|| exit_value.to_string(), str::to_owned);
    let agent = agent_id(state);
    let stderr = stderr_tail(&files.stderr);
    if killed == Some(Outcome::Stopped) {
        return Ok(TurnResult::ended(Outcome::Stopped, exit_value));
    }
    if agent == "agy" && events.result_seen && events.completed
        || agent != "agy" && events.completed && exit.success() && killed.is_none()
    {
        return Ok(TurnResult::ended(Outcome::Completed, exit_value));
    }
    if agent == "agy" {
        let quota = format!("{}\n{stderr}", events.final_error);
        if QUOTA.is_match(&quota) {
            return Ok(TurnResult::failed(
                Outcome::Fatal,
                format!(
                    "Antigravity 额度用完（{}）：{}",
                    state["options"]["model"].as_str().unwrap_or_default(),
                    quota.trim()
                ),
                exit_value,
            ));
        }
    }
    if let Some(outcome) = killed {
        return Ok(TurnResult::failed(
            outcome,
            format!("{} (watchdog)", outcome.as_str()),
            exit_value,
        ));
    }
    if agent == "agy"
        && events.result_seen
        && events
            .final_error
            .to_ascii_lowercase()
            .contains("malformed function call")
    {
        if events.agy_response {
            feed(
                dir,
                &format!(
                    "⚠ {} Antigravity 回复末尾的工具调用格式出错，已收下它写好的回复按完成处理；如果回复显示还没做完，用 send 让它接着做",
                    state["task"].as_str().unwrap_or_default()
                ),
            )?;
            return Ok(TurnResult::ended(Outcome::Completed, exit_value));
        }
        return Ok(TurnResult::failed(
            Outcome::Retryable,
            events.final_error.chars().take(1000).collect::<String>(),
            exit_value,
        ));
    }
    let fallback = format!("exit {exit_label}");
    let error = [
        events.final_error.as_str(),
        events.error.as_str(),
        stderr.as_str(),
        &fallback,
    ]
    .into_iter()
    .find(|error| !error.trim().is_empty())
    .unwrap_or(&fallback);
    let no_agy_result = agent == "agy" && !events.result_seen;
    let no_dsh_final = agent == "dsh"
        && !events.result_seen
        && events.final_error.is_empty()
        && events.error.is_empty();
    let fatal = FATAL.is_match(error)
        || agent == "dsh" && error.contains("MISSING_CREDENTIAL")
        || agent == "agy"
            && error
                .to_ascii_lowercase()
                .contains("invalid model selection");
    let outcome = if !fatal && (RETRYABLE.is_match(error) || no_agy_result || no_dsh_final) {
        Outcome::Retryable
    } else {
        Outcome::Fatal
    };
    let message = if no_agy_result {
        format!("Antigravity 异常退出（未收到 result，exit {exit_label}）：{stderr}")
    } else if no_dsh_final {
        format!("DSH 异常退出（未收到 final，exit {exit_label}）：{error}")
    } else {
        error.to_owned()
    };
    Ok(TurnResult::failed(
        outcome,
        message.chars().take(1000).collect::<String>(),
        exit_value,
    ))
}

fn worker_exit(exit: ExitStatus) -> Value {
    if let Some(code) = exit.code() {
        return json!(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = exit.signal() {
            return json!(match signal {
                libc::SIGTERM => "SIGTERM".into(),
                libc::SIGINT => "SIGINT".into(),
                libc::SIGKILL => "SIGKILL".into(),
                other => format!("signal {other}"),
            });
        }
    }
    Value::Null
}

struct Worker {
    child: Child,
    #[cfg(windows)]
    job: Option<windows::Job>,
}

impl Worker {
    fn new(child: Child) -> Self {
        #[cfg(windows)]
        let job = windows::Job::attach(&child);
        Self {
            child,
            #[cfg(windows)]
            job,
        }
    }

    fn terminate(&mut self) {
        let pid = self.child.id();
        #[cfg(unix)]
        {
            let tree = unix::descendants(pid);
            unix::signal_group(pid, libc::SIGTERM);
            unix::signal_tree(&tree, libc::SIGTERM);
            let started = Instant::now();
            while started.elapsed() < Duration::from_millis(750) {
                if self.child.try_wait().ok().flatten().is_some() && !unix::any_alive(&tree[1..]) {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
            unix::signal_group(pid, libc::SIGKILL);
            unix::signal_tree(&tree, libc::SIGKILL);
        }
        #[cfg(windows)]
        if !self.job.as_ref().is_some_and(windows::Job::terminate) {
            windows::terminate_external(pid);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.terminate();
        }
    }
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // The supervisor must survive the launcher shell and have no controlling terminal.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

#[cfg(unix)]
fn prepare_worker(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn prepare_worker(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
}

fn terminate_external(pid: u32) {
    if pid < 2 || !pid_alive(&json!(pid)) {
        return;
    }
    #[cfg(unix)]
    {
        let tree = unix::descendants(pid);
        unix::signal_tree(&tree, libc::SIGTERM);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) && unix::any_alive(&tree) {
            thread::sleep(STOP_POLL);
        }
        unix::signal_tree(&tree, libc::SIGKILL);
    }
    #[cfg(windows)]
    windows::terminate_external(pid);
}

#[cfg(unix)]
mod unix {
    pub(super) fn descendants(root: u32) -> Vec<u32> {
        let output = ["/bin/ps", "/usr/bin/ps"].into_iter().find_map(|program| {
            std::process::Command::new(program)
                .args(["-A", "-o", "pid=,ppid="])
                .output()
                .ok()
                .filter(|output| output.status.success())
        });
        let Some(output) = output else {
            return vec![root];
        };
        tree_from_listing(root, &String::from_utf8_lossy(&output.stdout))
    }

    pub(super) fn tree_from_listing(root: u32, listing: &str) -> Vec<u32> {
        let mut tree = vec![root];
        let mut children = std::collections::HashMap::<u32, Vec<u32>>::new();
        for line in listing.lines() {
            let mut words = line
                .split_ascii_whitespace()
                .filter_map(|value| value.parse().ok());
            if let (Some(pid), Some(parent)) = (words.next(), words.next())
                && pid > 1
                && pid != parent
            {
                children.entry(parent).or_default().push(pid);
            }
        }
        let mut index = 0;
        while let Some(pid) = tree.get(index) {
            if let Some(mut pids) = children.remove(pid) {
                tree.append(&mut pids);
            }
            index += 1;
        }
        tree
    }

    pub(super) fn signal_group(pid: u32, signal: i32) {
        if let Ok(pid) = libc::pid_t::try_from(pid)
            && pid > 1
        {
            // The worker was created as the leader of its own process group.
            unsafe { libc::kill(-pid, signal) };
        }
    }

    pub(super) fn signal_tree(tree: &[u32], signal: i32) {
        for &pid in tree.iter().rev() {
            if let Ok(pid) = libc::pid_t::try_from(pid)
                && pid > 1
            {
                // Snapshot callers only pass the current worker and its descendants.
                unsafe { libc::kill(pid, signal) };
            }
        }
    }

    pub(super) fn any_alive(tree: &[u32]) -> bool {
        tree.iter()
            .any(|&pid| super::super::platform::pid_alive(pid))
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, code: u32) -> i32;
    }

    pub(super) struct Job(OwnedHandle);

    impl Job {
        pub(super) fn attach(child: &Child) -> Option<Self> {
            // The anonymous handle stays owned until the worker exits.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = OwnedHandle::from_raw_handle(handle);
                (AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) != 0)
                    .then_some(Self(job))
            }
        }

        pub(super) fn terminate(&self) -> bool {
            // The job contains only this Foreman worker and its descendants.
            unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) != 0 }
        }
    }

    pub(super) fn terminate_external(pid: u32) {
        let system = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let _ = Command::new(system.join("System32/taskkill.exe"))
            .args(["/F", "/T", "/PID"])
            .arg(pid.to_string())
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(target_os = "macos")]
mod mac_network {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
    const SAMPLE_TIMEOUT: Duration = Duration::from_secs(3);
    const MIN_GROWTH: u64 = 2048;

    pub(super) struct Probe {
        pid: u32,
        nettop: PathBuf,
        last_launch: Option<Instant>,
        pending: Option<(u64, thread::JoinHandle<Option<u64>>)>,
        previous_total: Option<u64>,
        generating: Option<(Instant, String)>,
        cancel: Arc<AtomicBool>,
    }

    impl Probe {
        pub(super) fn new(pid: u32) -> Option<Self> {
            let path = std::env::var_os("FOREMAN_NETTOP")
                .filter(|path| !path.is_empty())
                .unwrap_or_else(|| "/usr/bin/nettop".into());
            let cwd = std::env::current_dir().ok()?;
            let candidate = cwd.join(path);
            let nettop = resolve_executable(&candidate, &cwd, &BTreeMap::new())?;
            Some(Self {
                pid,
                nettop,
                last_launch: None,
                pending: None,
                previous_total: None,
                generating: None,
                cancel: Arc::new(AtomicBool::new(false)),
            })
        }

        pub(super) fn poll(
            &mut self,
            output_version: u64,
            state: &mut Value,
            activity: &mut LastActivity,
        ) -> bool {
            let mut updated = false;
            if self
                .pending
                .as_ref()
                .is_some_and(|(_, probe)| probe.is_finished())
            {
                let (captured_version, pending) = self.pending.take().unwrap();
                let is_current = captured_version == output_version;
                let total = pending.join().ok().flatten();
                let grown = match total {
                    Some(bytes) => self.previous_total.replace(bytes).is_some_and(|previous| {
                        let delta = bytes.checked_sub(previous).unwrap_or(bytes);
                        delta >= MIN_GROWTH
                    }),
                    None => false,
                };
                if grown && is_current {
                    activity.observe(SystemTime::now());
                    let (since, _) = self.generating.get_or_insert_with(|| {
                        state["generatingSince"] = json!(timestamp());
                        (
                            Instant::now(),
                            state["activity"].as_str().unwrap_or("启动中").to_owned(),
                        )
                    });
                    if state["waitingQuestionText"].is_null() {
                        let duration =
                            super::super::duration(Some(since.elapsed().as_secs_f64() * 1000.0));
                        let intent = state["intent"]
                            .as_str()
                            .filter(|intent| !intent.is_empty())
                            .map(|intent| format!(" · {intent}"))
                            .unwrap_or_default();
                        state["activity"] = json!(format!("生成中 {duration}{intent}"));
                        state["activityAt"] = json!(timestamp());
                    }
                    updated = true;
                } else {
                    updated |= self.end_generation(state);
                }
            }
            if self.pending.is_none()
                && self
                    .last_launch
                    .is_none_or(|last| last.elapsed() >= SAMPLE_INTERVAL)
            {
                let pid = self.pid;
                let nettop = self.nettop.clone();
                let cancel = Arc::clone(&self.cancel);
                self.last_launch = Some(Instant::now());
                if let Ok(thread) =
                    thread::Builder::new().spawn(move || sample(pid, &nettop, &cancel))
                {
                    self.pending = Some((output_version, thread));
                }
            }
            updated
        }

        pub(super) fn end_generation(&mut self, state: &mut Value) -> bool {
            if let Some((_, previous)) = self.generating.take() {
                state["generatingSince"] = Value::Null;
                if state["activity"]
                    .as_str()
                    .is_some_and(|activity| activity.starts_with("生成中 "))
                {
                    state["activity"] = json!(previous);
                    state["activityAt"] = json!(timestamp());
                }
                return true;
            }
            false
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            self.cancel.store(true, Ordering::Relaxed);
            if let Some((_, probe)) = self.pending.take() {
                let _ = probe.join();
            }
        }
    }

    fn sample(pid: u32, nettop: &Path, cancel: &AtomicBool) -> Option<u64> {
        let deadline = Instant::now() + SAMPLE_TIMEOUT;
        let mut ps = Command::new("/bin/ps");
        ps.args(["-A", "-o", "pid=,ppid="]);
        let processes = bounded_output(ps, deadline, cancel)?;
        let tree = unix::tree_from_listing(pid, &processes);
        let mut command = Command::new(nettop);
        command.args(["-P", "-L", "1", "-x", "-J", "bytes_in"]);
        for pid in &tree {
            command.arg("-p").arg(pid.to_string());
        }
        let data = bounded_output(command, deadline, cancel)?;
        parse_counters(&data, &tree)
    }

    fn bounded_output(
        mut command: Command,
        deadline: Instant,
        cancel: &AtomicBool,
    ) -> Option<String> {
        if Instant::now() >= deadline || cancel.load(Ordering::Relaxed) {
            return None;
        }
        let mut stdout = tempfile::tempfile().ok()?;
        command
            .stdin(Stdio::null())
            .stdout(stdout.try_clone().ok()?)
            .stderr(Stdio::null());
        prepare_worker(&mut command);
        let mut child = command.spawn().ok()?;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(_)) => return None,
                Ok(None) if Instant::now() < deadline && !cancel.load(Ordering::Relaxed) => {
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(None) | Err(_) => {
                    // The sample has its own group, including any helper started by nettop.
                    unix::signal_group(child.id(), libc::SIGKILL);
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        stdout.seek(SeekFrom::Start(0)).ok()?;
        let mut data = Vec::new();
        stdout.take(1024 * 1024).read_to_end(&mut data).ok()?;
        Some(String::from_utf8_lossy(&data).into_owned())
    }

    fn parse_counters(data: &str, tree: &[u32]) -> Option<u64> {
        let mut named_column = None;
        let mut total = None;
        for line in data.lines() {
            let cells = line.split(',').map(str::trim).collect::<Vec<_>>();
            if let Some(column) = cells.iter().position(|cell| *cell == "bytes_in") {
                named_column = Some(column);
                continue;
            }
            let Some(column) = named_column else { continue };
            let pid = cells
                .first()
                .and_then(|cell| cell.rsplit('.').next())
                .and_then(|pid| pid.parse::<u32>().ok());
            if pid.is_some_and(|pid| tree.contains(&pid))
                && let Some(bytes) = cells
                    .get(column)
                    .and_then(|bytes| bytes.parse::<u64>().ok())
            {
                total = Some(total.unwrap_or(0u64).saturating_add(bytes));
            }
        }
        total
    }
}
