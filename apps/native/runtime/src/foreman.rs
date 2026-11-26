//! Native reader for the Foreman run directories shared with existing supervisors.

use crate::agent_hud_projection::timestamp_ms;
use anyhow::{Context, Result, anyhow, bail, ensure};
use matou_native_core::dispatch_roles::{
    DispatchRolesConfig, DispatchRolesStore, ROLES_FILENAME, format_roles_markdown,
};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[path = "foreman_mail.rs"]
mod mail;
#[path = "foreman_process.rs"]
mod process;

const SKILL: &str = include_str!(
    "../control-assets/providers/claude-plugin/skills/agent-foreman/SKILL.md"
);
const BRIEF: &str = include_str!(
    "../control-assets/providers/claude-plugin/skills/agent-foreman/brief-template.md"
);
const PARALLEL: &str = include_str!(
    "../control-assets/providers/claude-plugin/skills/agent-foreman/parallel.md"
);
const REFERENCE: &str = include_str!(
    "../control-assets/providers/claude-plugin/skills/agent-foreman/reference.md"
);
const USAGE: &str = "Usage:
  mt-foreman guide [brief|parallel|reference]
  mt-foreman roles
  mt-foreman config
  mt-foreman models [--model M] [--effort E] [--refresh-models]
  mt-foreman status [任务名]
  mt-foreman watch <任务名> [--watch-min N]
  mt-foreman start <任务名> --cd <目录> [--brief <文件>] [选项]
  mt-foreman stop <任务名>
  mt-foreman send|resume <任务名> [--message <文件>] [选项]
  mt-foreman ask|inbox|answer|note <任务名>";
const DEFAULT_WATCH_MINUTES: f64 = 29.0;
const AGENTS: [&str; 3] = ["codex", "dsh", "agy"];

pub(crate) fn run(args: impl IntoIterator<Item = OsString>) -> i32 {
    let result =
        (|| {
            let args = args
                .into_iter()
                .map(|arg| {
                    arg.into_string()
                        .map_err(|_| anyhow!("参数必须是有效的 Unicode"))
                })
                .collect::<Result<Vec<_>>>()?;
            let args = args.iter().map(String::as_str).collect::<Vec<_>>();
            match args.as_slice() {
                [] | ["--help" | "-h" | "help"] => {
                    println!("{USAGE}");
                    Ok(0)
                }
                ["guide", "brief"] => emit(BRIEF),
                ["guide", "parallel"] => emit(PARALLEL),
                ["guide", "reference"] => emit(REFERENCE),
                ["guide"] => match Store::from_environment()?.roles() {
                    Some(roles) => emit(&format!("{}\n\n{roles}\n", SKILL.trim_end())),
                    None => emit(SKILL),
                },
                ["roles"] => {
                    println!("{}", Store::from_environment()?.roles().unwrap_or_else(|| {
                    "当前未配置 Matou 自定义 Agent 生态位，按内置 SKILL.md 默认分工执行。".into()
                }));
                    Ok(0)
                }
                ["config"] => process::config(&Store::from_environment()?),
                ["models", options @ ..] => process::models(&Store::from_environment()?, options),
                ["status"] => {
                    Store::from_environment()?.list()?;
                    Ok(0)
                }
                ["status", task] => {
                    Store::from_environment()?.status(task)?;
                    Ok(0)
                }
                ["watch", task, options @ ..] => {
                    let minutes = match options {
                        [] => DEFAULT_WATCH_MINUTES,
                        ["--watch-min", value] => value.parse().context("无效的 --watch-min")?,
                        _ => bail!("{USAGE}"),
                    };
                    Store::from_environment()?.watch(task, None, minutes)
                }
                ["start", task, options @ ..] => {
                    process::start(&Store::from_environment()?, task, options)
                }
                ["__supervise", task] => process::supervise(&Store::from_environment()?, task),
                ["stop", task] => process::stop(&Store::from_environment()?, task),
                [
                    "send" | "resume" | "ask" | "inbox" | "answer" | "note" | "__notify-owner",
                    ..,
                ] => mail::run(&Store::from_environment()?, &args),
                _ => bail!("{USAGE}"),
            }
        })();
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error:#}");
            1
        }
    }
}

fn emit(text: &str) -> Result<i32> {
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()?;
    Ok(0)
}

pub(crate) struct Store {
    root: PathBuf,
    has_override: bool,
}

impl Store {
    pub(crate) fn from_environment() -> Result<Self> {
        let default = std::env::home_dir().map(|home| home.join(".claude/agent-foreman/runs"));
        let override_dir = std::env::var_os("FOREMAN_RUN_DIR")
            .filter(|value| !value.is_empty())
            .map(std::path::absolute)
            .transpose()?;
        let has_override = override_dir.is_some() && override_dir != default;
        let root = override_dir
            .or(default)
            .context("无法确定 Foreman 任务目录")?;
        Ok(Self { root, has_override })
    }

    pub(crate) fn task_dir(&self, task: &str) -> Result<PathBuf> {
        let path = Path::new(task);
        ensure!(
            matches!(path.components().next(), Some(Component::Normal(_)))
                && path.components().count() == 1
                && !task.starts_with('.')
                && (!cfg!(windows) || !task.contains(':'))
                && task.chars().count() <= 40
                && !task
                    .chars()
                    .any(|ch| ch.is_whitespace()
                        || matches!(ch, '\'' | '"' | '`' | '/' | '\\' | '\0')),
            "任务名不能包含空格、引号或斜杠，最长 40 个字"
        );
        Ok(self.root.join(path))
    }

    pub(crate) fn load(&self, task: &str) -> Result<Value> {
        read_json(&self.task_dir(task)?.join("state.json"))
    }

    fn roles(&self) -> Option<String> {
        self.role_config().as_ref().map(format_roles_markdown)
    }

    fn role_config(&self) -> Option<DispatchRolesConfig> {
        let path = PathBuf::from(std::env::var_os("MATOU_FOREMAN_ROLES_FILE")?);
        let document = read_json(&path).ok()?;
        if document["version"].as_u64() != Some(1) || !document["agents"].is_array() {
            return None;
        }
        let has_default_agent = document.get("defaultAgent").is_some();
        let mut config: DispatchRolesConfig =
            if path.file_name().is_some_and(|name| name == ROLES_FILENAME) {
                let snapshot = DispatchRolesStore::for_data_root(path.parent()?)
                    .snapshot()
                    .ok()?;
                if !snapshot.exists {
                    return None;
                }
                snapshot.config
            } else {
                let mut config: DispatchRolesConfig = serde_json::from_value(document).ok()?;
                config.normalize_default_agent();
                config
            };
        if !has_default_agent && let Some(agent) = self.legacy_default_agent() {
            config.default_agent = agent;
        }
        Some(config)
    }

    fn legacy_default_agent(&self) -> Option<String> {
        let config = read_json(&self.root.parent()?.join("config.json")).ok()?;
        let agent = config["defaults"]["agent"].as_str()?;
        (config["version"].as_u64() == Some(1) && AGENTS.contains(&agent)).then(|| agent.into())
    }

    fn list(&self) -> Result<()> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries.collect::<io::Result<Vec<_>>>()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error).with_context(|| self.root.display().to_string()),
        };
        let mut entries = entries;
        entries.sort_by_cached_key(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name.to_lowercase(), name)
        });
        let mut rows = Vec::new();
        for entry in entries {
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let file = entry.path().join("state.json");
            let bytes = match fs::read(&file) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error).with_context(|| file.display().to_string()),
            };
            let state: Value =
                serde_json::from_slice(&bytes).with_context(|| file.display().to_string())?;
            rows.push(format!(
                "{}\t{}\t{}",
                entry.file_name().to_string_lossy(),
                state["status"].as_str().unwrap_or("undefined"),
                agent_id(&state)
            ));
        }
        if rows.is_empty() {
            println!("暂无任务");
        } else {
            println!("{}", rows.join("\n"));
        }
        Ok(())
    }

    fn status(&self, task: &str) -> Result<()> {
        let directory = self.task_dir(task)?;
        let state = self.load(task)?;
        let agent = agent_id(&state);
        let turns = turns(&state)
            .iter()
            .map(|turn| {
                let mut row = Map::new();
                copy_fields(turn, &mut row, &["n", "error"]);
                row.insert(
                    "result".into(),
                    turn.get("result")
                        .filter(|value| !value.is_null())
                        .cloned()
                        .unwrap_or(json!("running")),
                );
                row
            })
            .collect::<Vec<_>>();
        let idle = timestamp_ms(&state["lastActivityAt"])
            .map(|last| ((now_ms() - last) / 1000.0 + 0.5).floor() as i64);
        let mut view = json!({
            "task": task,
            "agent": agent,
            "alive": pid_alive(&state["pid"]),
            "idleSeconds": idle,
            "turns": turns,
            "lastExternalSend": state["lastExternalSend"],
            "pendingOwnerNotifications": owner_notifications(&directory)
        })
        .as_object()
        .expect("status view is a JSON object")
        .clone();
        copy_fields(&state, &mut view, &["status", "threadId", "error"]);
        copy_fields(&state["options"], &mut view, &["model"]);
        if agent == "agy" {
            let effort = state["options"]["model"].as_str().and_then(|model| {
                let (_, suffix) = model.rsplit_once('-')?;
                matches!(suffix, "low" | "medium" | "high" | "xhigh").then_some(suffix)
            });
            view.insert("effort".into(), json!(effort));
        } else {
            copy_fields(&state["options"], &mut view, &["effort"]);
        }
        if orphaned(&state) {
            view.insert("status".into(), json!("failed"));
            view.insert("error".into(), json!("监工进程意外退出（未留下结束状态）"));
        }
        println!("{}", serde_json::to_string_pretty(&view)?);
        let (_, reply) = last_reply(&state);
        if !reply.is_empty() {
            let reply =
                String::from_utf16_lossy(&reply.encode_utf16().take(2000).collect::<Vec<_>>());
            println!("\n--- last message ---\n{reply}");
        }
        Ok(())
    }

    pub(crate) fn watch(&self, task: &str, offset: Option<u64>, minutes: f64) -> Result<i32> {
        ensure!(minutes > 0.0, "无效的 --watch-min");
        let max = Duration::try_from_secs_f64(minutes * 60.0).context("无效的 --watch-min")?;
        let start = Instant::now();
        let directory = self.task_dir(task)?;
        let feed = directory.join("feed.log");
        let mut state = self.load(task)?;
        let mut out = io::stdout().lock();
        let log_path = directory.join("supervisor.log");
        let unexpected_exit = |out: &mut io::StdoutLock<'_>| {
            writeln!(
                out,
                "{} ❌ {task} 监督进程意外退出，详见 {}",
                clock(now_ms()),
                log_path.display()
            )?;
            Ok(6)
        };

        if offset.is_none() {
            for notice in owner_notifications(&directory) {
                let at = saved_clock(&notice["at"]);
                let round = shown(&notice["round"]);
                let caller = notice["caller"]["title"].as_str().unwrap_or_default();
                let caller = caller.replace(
                    |ch: char| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'),
                    " ",
                );
                let reason = shown(&notice["reason"]);
                writeln!(
                    out,
                    "{at} ✉ 第 {round} 轮由 {caller} 拉起（{reason}，未注入）"
                )?;
            }
        }
        if orphaned(&state) || (state["status"] == "failed" && state["exitCode"] == 6) {
            return unexpected_exit(&mut out);
        }
        let mut offset = match offset {
            Some(offset) => offset,
            None => {
                if let Some(code) = terminal_code(&state) {
                    out.write_all(final_text(task, &state).as_bytes())?;
                    return Ok(code);
                }
                if let Some(question) = blocking_question(&directory, &state) {
                    self.question_notice(&mut out, task, &directory, &question)?;
                    return Ok(7);
                }
                let offset = fs::metadata(&feed).map(|file| file.len()).unwrap_or(0);
                activity(&mut out, task, &directory, &state)?;
                out.flush()?;
                offset
            }
        };
        loop {
            state = self.load(task)?;
            drain_feed(&feed, &mut offset, &mut out)?;
            if let Some(code) = terminal_code(&state) {
                return if code == 6 {
                    unexpected_exit(&mut out)
                } else {
                    Ok(code)
                };
            }
            if let Some(question) = blocking_question(&directory, &state) {
                let seen = question["n"].as_u64().is_some_and(|number| {
                    state["mailboxSeen"][format!("q-{number}")]
                        .as_bool()
                        .unwrap_or(false)
                });
                if seen || start.elapsed() >= max {
                    self.question_notice(&mut out, task, &directory, &question)?;
                    return Ok(7);
                }
            }
            if !pid_alive(&state["supervisorPid"]) {
                // The old supervisor may have published its final state as it exited.
                state = self.load(task)?;
                drain_feed(&feed, &mut offset, &mut out)?;
                return match terminal_code(&state) {
                    Some(code) if code != 6 => Ok(code),
                    _ => unexpected_exit(&mut out),
                };
            }
            let remaining = max.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                writeln!(
                    out,
                    "⏳ {task} 仍在运行（跟踪满 {minutes} 分钟自动交回，任务不受影响）；继续跟踪：{}mt-foreman watch {}",
                    self.command_prefix(),
                    task_argument(task)
                )?;
                return Ok(8);
            }
            thread::sleep(remaining.min(Duration::from_millis(500)));
        }
    }

    fn question_notice(
        &self,
        out: &mut impl Write,
        task: &str,
        directory: &Path,
        question: &Value,
    ) -> Result<()> {
        let number = shown(&question["n"]);
        let prefix = self.command_prefix();
        let argument = task_argument(task);
        let path = mailbox_path(directory, "questions", &format!("q-{number}"));
        let answer = format!("{prefix}mt-foreman answer {argument} {number} --message \"回答.md\"");
        writeln!(
            out,
            "❓ {task} 正在等你回答 #{number}：{}\n完整问题：{}\n将回答保存为 UTF-8 文件后运行：{answer}\n回答后用 Monitor 运行 {prefix}mt-foreman watch {argument} 继续跟踪",
            short(question["text"].as_str().unwrap_or_default(), 300),
            path.display()
        )?;
        Ok(())
    }

    fn command_prefix(&self) -> String {
        if !self.has_override {
            return String::new();
        }
        let root = self.root.to_string_lossy();
        if cfg!(windows) {
            format!("$env:FOREMAN_RUN_DIR = '{}'; ", root.replace('\'', "''"))
        } else {
            format!("FOREMAN_RUN_DIR='{}' ", root.replace('\'', "'\"'\"'"))
        }
    }
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).with_context(|| path.display().to_string())?;
    serde_json::from_slice(&bytes).with_context(|| path.display().to_string())
}

fn copy_fields(source: &Value, target: &mut Map<String, Value>, keys: &[&str]) {
    for &key in keys {
        if let Some(value) = source.get(key) {
            target.insert(key.into(), value.clone());
        }
    }
}

fn agent_id(state: &Value) -> &str {
    state["agent"]
        .as_str()
        .filter(|agent| !agent.is_empty())
        .unwrap_or("codex")
}

fn agent_name(state: &Value) -> &str {
    match agent_id(state) {
        "dsh" => "DSH",
        "agy" => "Antigravity",
        _ => "Codex",
    }
}

fn turns(state: &Value) -> &[Value] {
    state["turns"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn last_reply(state: &Value) -> (Option<&str>, String) {
    let path = turns(state)
        .last()
        .and_then(|turn| turn["files"]["last"].as_str());
    let reply = path
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    (path, reply)
}

fn shown(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    if value.is_null() {
        return "undefined".into();
    }
    value.to_string()
}

fn short(text: &str, count: usize) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(count)
        .collect()
}

fn terminal_code(state: &Value) -> Option<i32> {
    let fallback = match state["status"].as_str()? {
        "completed" => 0,
        "failed" => 2,
        "timeout" => 4,
        "stopped" => 5,
        _ => return None,
    };
    Some(
        state["exitCode"]
            .as_i64()
            .and_then(|code| i32::try_from(code).ok())
            .unwrap_or(fallback),
    )
}

fn orphaned(state: &Value) -> bool {
    state["task"].as_str().is_some_and(|task| !task.is_empty())
        && matches!(state["status"].as_str(), Some("running" | "waiting-retry"))
        && !pid_alive(&state["supervisorPid"])
        && !pid_alive(&state["pid"])
}

fn final_text(task: &str, state: &Value) -> String {
    let status = &state["status"];
    let final_line = match status.as_str() {
        Some("completed") => {
            let last = turns(state).last().unwrap_or(&Value::Null);
            let elapsed = timestamp_ms(&last["endedAt"])
                .zip(timestamp_ms(&last["startedAt"]))
                .map(|(end, start)| end - start);
            format!(
                "✅ {task} 本轮完成（第 {} 轮，用时 {}）",
                shown(&last["n"]),
                duration(elapsed)
            )
        }
        Some("stopped") => format!("■ {task} 已手动停止"),
        Some("timeout") => {
            format!("⏱ {task} 超时停下，等待处理（不算失败；用 send 让它从断点接着做）")
        }
        _ => format!(
            "❌ {task} 失败：{}",
            short(state["error"].as_str().unwrap_or_default(), 1000)
        ),
    };
    let when = saved_clock(&state["endedAt"]);
    let mut result = format!("{when} {final_line}");
    if status != "stopped" {
        let (path, reply) = last_reply(state);
        if status == "completed" || !reply.trim().is_empty() {
            let name = agent_name(state);
            let reply = if let Some((at, _)) = reply.char_indices().nth(6000) {
                format!(
                    "{}…（完整内容见 {}）",
                    &reply[..at],
                    path.unwrap_or_default()
                )
            } else {
                reply
            };
            result.push_str(&format!("\n{when} --- {name} 回复 ---\n{reply}"));
        }
    }
    result.push('\n');
    result
}

fn mailbox_path(directory: &Path, kind: &str, file: &str) -> PathBuf {
    directory.join(kind).join(format!("{file}.json"))
}

fn numbered_mail(directory: &Path, kind: &str, prefix: &str) -> Vec<Value> {
    let Ok(entries) = fs::read_dir(directory.join(kind)) else {
        return Vec::new();
    };
    let mut files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let number = name
                .to_str()?
                .strip_prefix(prefix)?
                .strip_suffix(".json")?
                .parse::<u64>()
                .ok()?;
            Some((number, entry.path()))
        })
        .collect::<Vec<_>>();
    files.sort_unstable();
    files
        .into_iter()
        .filter_map(|(_, file)| read_json(&file).ok())
        .collect()
}

fn owner_notifications(directory: &Path) -> Vec<Value> {
    numbered_mail(directory, "outcomes", "owner-notification-")
        .into_iter()
        .filter(Value::is_object)
        .collect()
}

fn blocking_question(directory: &Path, state: &Value) -> Option<Value> {
    let turn = turns(state).len() as u64;
    numbered_mail(directory, "questions", "q-")
        .into_iter()
        .find(|question| {
            let Some(number) = question["n"].as_u64() else {
                return false;
            };
            !question["noWait"].as_bool().unwrap_or(false)
                && question["turn"].as_u64() == Some(turn)
                && !mailbox_path(directory, "answers", &format!("a-{number}")).exists()
                && !mailbox_path(directory, "outcomes", &format!("q-{number}")).exists()
                && timestamp_ms(&question["createdAt"])
                    .zip(question["waitMin"].as_f64())
                    .is_some_and(|(created, minutes)| now_ms() - created < minutes * 60_000.0)
        })
}

fn task_argument(task: &str) -> String {
    if task
        .chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        task.into()
    } else {
        format!("'{task}'")
    }
}

fn activity(out: &mut impl Write, task: &str, directory: &Path, state: &Value) -> Result<()> {
    let when = clock(now_ms());
    let waiting = state["waitingQuestions"].get(0).and_then(Value::as_u64);
    let mut detail = None;
    if let Some(number) = waiting {
        if !mailbox_path(directory, "answers", &format!("a-{number}")).exists() {
            writeln!(
                out,
                "{when} … {task} 等待回答 #{number}：{}",
                shown(&state["waitingQuestionText"])
            )?;
            return Ok(());
        }
        detail = Some(format!("已回答 #{number}，等待 {} 继续", agent_name(state)));
    }
    let detail = detail
        .as_deref()
        .or(state["activity"].as_str())
        .filter(|text| !text.is_empty())
        .unwrap_or("启动中");
    let elapsed = timestamp_ms(&state["runStartedAt"])
        .or_else(|| timestamp_ms(&state["createdAt"]))
        .map(|started| now_ms() - started);
    writeln!(
        out,
        "{when} … {task} 进行中：{detail}（已运行 {}）",
        duration(elapsed)
    )?;
    Ok(())
}

fn drain_feed(path: &Path, offset: &mut u64, out: &mut impl Write) -> Result<()> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| path.display().to_string()),
    };
    let available = file.metadata()?.len().saturating_sub(*offset);
    if available > 0 {
        file.seek(SeekFrom::Start(*offset))?;
        *offset += io::copy(&mut file.take(available), out)?;
        out.flush()?;
    }
    Ok(())
}

fn duration(elapsed: Option<f64>) -> String {
    let Some(elapsed) = elapsed else {
        return "NaN时NaN分".into();
    };
    let seconds = (elapsed / 1000.0).floor().max(0.0) as u64;
    match seconds {
        0..60 => format!("{seconds}秒"),
        60..3600 => format!("{}分{}秒", seconds / 60, seconds % 60),
        _ => format!("{}时{:02}分", seconds / 3600, seconds / 60 % 60),
    }
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0)
}

fn saved_clock(value: &Value) -> String {
    timestamp_ms(value).map_or_else(|| "Invalid Date".into(), clock)
}

fn clock(milliseconds: f64) -> String {
    let seconds = (milliseconds / 1000.0).floor() as i64;
    let (hour, minute, second) = platform::local_clock(seconds).unwrap_or_else(|| {
        let seconds = seconds.rem_euclid(86_400) as u16;
        (seconds / 3600, seconds / 60 % 60, seconds % 60)
    });
    format!("{hour:02}:{minute:02}:{second:02}")
}

fn pid_alive(pid: &Value) -> bool {
    pid.as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|&pid| pid != 0)
        .is_some_and(platform::pid_alive)
}

#[cfg(unix)]
mod platform {
    pub(super) fn pid_alive(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // Signal zero only queries liveness and does not send a signal.
        (unsafe { libc::kill(pid, 0) == 0 })
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    pub(super) fn local_clock(seconds: i64) -> Option<(u16, u16, u16)> {
        let seconds = seconds as libc::time_t;
        let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
        // The C library initializes the separate output buffer on success.
        unsafe {
            if libc::localtime_r(&seconds, local.as_mut_ptr()).is_null() {
                return None;
            }
            let local = local.assume_init();
            Some((
                local.tm_hour as u16,
                local.tm_min as u16,
                local.tm_sec as u16,
            ))
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, exit_code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn FileTimeToSystemTime(filetime: *const FileTime, time: *mut SystemTime) -> i32;
        fn SystemTimeToTzSpecificLocalTime(
            zone: *const c_void,
            universal: *const SystemTime,
            local: *mut SystemTime,
        ) -> i32;
    }

    pub(super) fn pid_alive(pid: u32) -> bool {
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        const STILL_ACTIVE: u32 = 259;
        const ERROR_ACCESS_DENIED: i32 = 5;
        // This handle has query-only rights and is closed before returning.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED);
            }
            let mut exit_code = 0;
            let active =
                GetExitCodeProcess(process, &mut exit_code) != 0 && exit_code == STILL_ACTIVE;
            CloseHandle(process);
            active
        }
    }

    pub(super) fn local_clock(seconds: i64) -> Option<(u16, u16, u16)> {
        let ticks = u64::try_from(seconds.checked_add(11_644_473_600)?)
            .ok()?
            .checked_mul(10_000_000)?;
        let filetime = FileTime {
            low: ticks as u32,
            high: (ticks >> 32) as u32,
        };
        let mut universal = SystemTime::default();
        let mut local = SystemTime::default();
        // The input and output structs match the Win32 ABI. A null zone uses the system time zone.
        unsafe {
            if FileTimeToSystemTime(&filetime, &mut universal) == 0
                || SystemTimeToTzSpecificLocalTime(std::ptr::null(), &universal, &mut local) == 0
            {
                return None;
            }
        }
        Some((local.hour, local.minute, local.second))
    }
}
