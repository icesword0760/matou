//! Mailbox and local session notifications for Foreman runs.

use super::{Store, mailbox_path, numbered_mail, pid_alive, read_json, terminal_code};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{self, IsTerminal, Read, Write},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_PROMPT: u64 = 4 * 1024 * 1024;

pub(super) fn run(store: &Store, args: &[&str]) -> Result<i32> {
    let [command, task, args @ ..] = args else {
        bail!("缺少 Foreman 任务名")
    };
    let directory = store.task_dir(task)?;
    match *command {
        "inbox" if args.is_empty() => inbox(store, task, &directory),
        "ask" | "answer" | "note" => message(store, task, &directory, command, args),
        "send" | "resume" => resume(store, task, &directory, args),
        "__notify-owner" if args.is_empty() => notify_owner(&directory),
        _ => bail!("无效的 Foreman 邮箱命令"),
    }
}

fn resume(store: &Store, task: &str, directory: &Path, args: &[&str]) -> Result<i32> {
    let mut snapshot = resumable(store, task)?;
    let previous_run = snapshot["runStartedAt"].clone();
    let previous_rounds = snapshot["turns"].as_array().map_or(0, Vec::len);
    let settings = super::process::prepare_resume(store, &mut snapshot, args)?;
    let prompt = read_prompt(settings.prompt_file.as_deref())?;
    let caller = identify();
    let offset = with_lock(directory, || {
        let mut state = resumable(store, task)?;
        ensure!(
            state["runStartedAt"] == previous_run
                && state["turns"].as_array().map_or(0, Vec::len) == previous_rounds,
            "任务状态已更新，请查看最新一轮后再续跑"
        );
        super::process::prepare_resume(store, &mut state, args)?;
        let owner = state["ownerSession"]["ref"].as_str().map(str::to_owned);
        state["pendingExternalSend"] = Value::Null;
        if let Some(caller) = &caller {
            match owner.as_deref() {
                None => state["ownerSession"] = caller.clone(),
                Some(owner) if caller["ref"] != owner => {
                    state["pendingExternalSend"] = json!({"caller": caller, "at": timestamp()});
                }
                _ => {}
            }
        }
        super::process::launch_turn_locked(store, task, &mut state, &prompt)
    })?;
    if settings.follow {
        store.watch(task, Some(offset), settings.watch_minutes)
    } else {
        Ok(0)
    }
}

fn resumable(store: &Store, task: &str) -> Result<Value> {
    let state = store.load(task)?;
    ensure!(
        !pid_alive(&state["supervisorPid"]) && !pid_alive(&state["pid"]),
        "Task {task} 仍在运行 (still running)."
    );
    ensure!(
        state["threadId"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "Task {task} has no thread id yet; start it again under a new name."
    );
    Ok(state)
}

pub(super) fn with_lock<T>(directory: &Path, run: impl FnOnce() -> Result<T>) -> Result<T> {
    let path = directory.join(".native-mailbox.lock");
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        ensure!(metadata.is_file(), "无效的 Foreman 邮箱锁");
    }
    // A persistent file lets the OS release the lock on every death path.
    // Neither a current owner nor another waiter ever removes the file.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .context("打开 Foreman 邮箱锁")?;
    let legacy_owner = directory.join(".mailbox-lock/owner.json");
    let started = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) => {}
            Err(fs::TryLockError::Error(error)) => return Err(error).context("锁定 Foreman 邮箱"),
        }
        if started.elapsed() >= Duration::from_secs(20) {
            bail!("Foreman 正在处理此任务，请稍后重试");
        }
        thread::sleep(Duration::from_millis(25));
    }
    // An earlier supervisor can still be finishing a mailbox write after an
    // upgrade. Wait for a live legacy owner without trying to retire its lock.
    while read_json(&legacy_owner).is_ok_and(|owner| pid_alive(&owner["pid"])) {
        if started.elapsed() >= Duration::from_secs(20) {
            bail!("Foreman 正在处理此任务，请稍后重试");
        }
        thread::sleep(Duration::from_millis(25));
    }
    run()
}

pub(super) fn write_json(path: &Path, value: &Value) -> Result<()> {
    let directory = path.parent().context("无效的 Foreman 写入路径")?;
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut temporary, value)?;
    temporary.flush()?;
    temporary.persist(path)?;
    Ok(())
}

pub(super) fn timestamp() -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = elapsed.as_secs();
    let days = (seconds / 86_400) as i64 + 719_468;
    // Convert Unix days to Gregorian civil dates, with March as month zero.
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = (shifted_month + 2) % 12 + 1;
    let year = era * 400 + year_of_era + i64::from(month < 3);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
        elapsed.subsec_millis()
    )
}

pub(super) fn read_prompt(path: Option<&Path>) -> Result<String> {
    let mut text = String::new();
    if let Some(path) = path {
        fs::File::open(path)?
            .take(MAX_PROMPT + 1)
            .read_to_string(&mut text)?;
    } else {
        ensure!(!io::stdin().is_terminal(), "请通过文件或标准输入提供正文");
        io::stdin()
            .lock()
            .take(MAX_PROMPT + 1)
            .read_to_string(&mut text)?;
    }
    ensure!(text.len() as u64 <= MAX_PROMPT, "Foreman 正文太长");
    let trimmed = text.trim();
    ensure!(!trimmed.is_empty(), "Foreman 正文为空");
    if !trimmed.contains('\n')
        && trimmed.len() < 500
        && ((trimmed.starts_with("$(") && trimmed.ends_with(')'))
            || (trimmed.starts_with('`') && trimmed.ends_with('`')))
    {
        bail!("交代书内的命令没有展开。请使用文件参数，或直接提供正文。");
    }
    Ok(text)
}

fn open_state(store: &Store, task: &str) -> Result<Value> {
    let state = store.load(task)?;
    ensure!(
        terminal_code(&state).is_none(),
        "任务已结束；请用 send 把回答连同后续要求发给它。"
    );
    Ok(state)
}

fn inbox(store: &Store, task: &str, directory: &Path) -> Result<i32> {
    store.load(task)?;
    with_lock(directory, || {
        let mut messages = Vec::new();
        for (kind, prefix, receipt) in [("answers", "a-", "answer-"), ("notes", "n-", "note-")] {
            for value in numbered_mail(directory, kind, prefix) {
                if let Some(n) = value["n"].as_u64() {
                    let marker = mailbox_path(directory, "read", &format!("{receipt}{n}"));
                    if !marker.exists() {
                        messages.push((kind, n, value, marker));
                    }
                }
            }
        }
        messages.sort_by(|a, b| {
            a.2["createdAt"]
                .as_str()
                .cmp(&b.2["createdAt"].as_str())
                .then(a.1.cmp(&b.1))
        });
        if messages.is_empty() {
            println!("没有新的回答或留言。");
        }
        for (kind, n, value, receipt) in messages {
            if let Some(text) = value["text"].as_str() {
                let time = value["createdAt"].as_str().unwrap_or("");
                let header = if kind == "answers" {
                    format!("回答 #{n}")
                } else {
                    format!("留言 {time}")
                };
                println!("【{header}】\n{text}");
                write_json(&receipt, &json!({"at": timestamp()}))?;
            }
        }
        Ok(0)
    })
}

fn message(
    store: &Store,
    task: &str,
    directory: &Path,
    command: &str,
    args: &[&str],
) -> Result<i32> {
    open_state(store, task)?;
    let mut no_wait = false;
    let mut wait_min = None;
    let mut number = None;
    let mut message = None;
    let mut args = args.iter().copied();
    while let Some(argument) = args.next() {
        match argument {
            "--message" if message.is_none() => {
                message = Some(Path::new(args.next().context("缺少消息文件路径")?));
            }
            "--no-wait" if command == "ask" => no_wait = true,
            "--wait-min" if command == "ask" => {
                let minutes: f64 = args.next().context("缺少等待时间")?.parse()?;
                ensure!(
                    minutes.is_finite() && (0. ..=600.).contains(&minutes) && minutes > 0.,
                    "无效的等待时间"
                );
                wait_min = Some(minutes);
            }
            value if command == "answer" && number.is_none() => {
                number = Some(value.parse::<u64>()?)
            }
            _ => bail!("无效的参数 {argument}"),
        }
    }
    let text = read_prompt(message)?;
    let (n, wait) = with_lock(directory, || {
        let state = open_state(store, task)?;
        if command == "answer" {
            let question = numbered_mail(directory, "questions", "q-")
                .into_iter()
                .find_map(|question| {
                    let n = question["n"].as_u64()?;
                    if number == Some(n)
                        || number.is_none()
                            && !mailbox_path(directory, "answers", &format!("a-{n}")).exists()
                    {
                        Some(n)
                    } else {
                        None
                    }
                })
                .context("没有匹配的未回答问题")?;
            let answer = mailbox_path(directory, "answers", &format!("a-{question}"));
            ensure!(!answer.exists(), "问题 #{question} 已回答");
            write_json(
                &answer,
                &json!({"n": question, "text": text, "createdAt": timestamp()}),
            )?;
            println!("已回答 #{question}");
            return Ok((question, 0.0));
        }
        let (kind, prefix) = if command == "ask" {
            ("questions", "q-")
        } else {
            ("notes", "n-")
        };
        let n = numbered_mail(directory, kind, prefix)
            .iter()
            .filter_map(|value| value["n"].as_u64())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .context("邮箱序号超出范围")?;
        let wait = wait_min
            .or_else(|| state["options"]["askWaitMin"].as_f64())
            .filter(|minutes| minutes.is_finite() && *minutes > 0. && *minutes <= 600.0)
            .unwrap_or(10.0);
        if command == "ask" && !no_wait {
            write_heartbeat(directory, n)?;
        }
        write_json(
            &mailbox_path(directory, kind, &format!("{prefix}{n}")),
            &json!({
                "n": n, "text": text, "createdAt": timestamp(),
                "turn": state["turns"].as_array().map_or(0, Vec::len),
                "pid": std::process::id(), "noWait": no_wait, "waitMin": wait
            }),
        )?;
        Ok((n, wait))
    })?;
    match command {
        "answer" => Ok(0),
        "note" => {
            println!("已留言");
            Ok(0)
        }
        _ if no_wait => {
            println!("已记录，先按你写的倾向继续；回答会出现在 inbox 里。");
            Ok(0)
        }
        _ => wait_for_answer(directory, n, wait),
    }
}

fn write_heartbeat(directory: &Path, n: u64) -> Result<()> {
    write_json(
        &mailbox_path(directory, "heartbeats", &format!("q-{n}")),
        &json!({"at": timestamp(), "pid": std::process::id()}),
    )
}

fn wait_for_answer(directory: &Path, n: u64, minutes: f64) -> Result<i32> {
    let started = Instant::now();
    let until = Duration::from_secs_f64(minutes * 60.0);
    let mut heartbeat = started;
    loop {
        let done = with_lock(directory, || {
            let outcome = mailbox_path(directory, "outcomes", &format!("q-{n}"));
            if let Ok(answer) = read_json(&mailbox_path(directory, "answers", &format!("a-{n}")))
                && let Some(text) = answer["text"].as_str()
            {
                io::stdout().write_all(text.as_bytes())?;
                write_json(
                    &mailbox_path(directory, "read", &format!("answer-{n}")),
                    &json!({"at": timestamp()}),
                )?;
                write_json(&outcome, &json!({"status": "answered"}))?;
                return Ok(true);
            }
            if started.elapsed() >= until {
                write_json(&outcome, &json!({"status": "timedout"}))?;
                println!(
                    "等了 {minutes} 分钟没有收到回答。\n涉及产品范围、用户可见取舍、删除数据或不可逆操作时，请停下并在最终回复列出这个问题。其他问题按提问时的倾向处理，之后用 inbox 查看迟到的回答。"
                );
                return Ok(true);
            }
            if heartbeat.elapsed() >= Duration::from_secs(5) {
                write_heartbeat(directory, n)?;
                heartbeat = Instant::now();
            }
            Ok(false)
        })?;
        if done {
            return Ok(0);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn mt(args: &[&str]) -> Result<Vec<u8>> {
    let arguments = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let exit = crate::mt_cli::run_with(
        &arguments,
        || {
            (
                std::env::var_os("MATOU_CONTROL_ENDPOINT"),
                std::env::var("MATOU_CONTROL_TOKEN")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .or_else(|| std::env::var("DSH_MATOU_CONTROL_TOKEN").ok()),
            )
        },
        &mut stdout,
        &mut stderr,
    );
    ensure!(exit == 0, "{}", String::from_utf8_lossy(&stderr).trim());
    Ok(stdout)
}

pub(super) fn identify() -> Option<Value> {
    let value: Value = serde_json::from_slice(&mt(&["identify", "--json"]).ok()?).ok()?;
    let target = &value["target"];
    let reference = target["ref"].as_str().filter(|value| !value.is_empty())?;
    let title = target["title"].as_str()?;
    Some(json!({"ref": reference, "title": title}))
}

pub(super) fn notify_started(directory: &Path, state: &mut Value, round: u64) {
    let pending = state["pendingExternalSend"].take();
    let Some(caller) = pending["caller"].as_object() else {
        return;
    };
    state["lastExternalSend"] = json!({"caller": caller, "at": pending["at"]});
    let Some(owner) = state["ownerSession"]["ref"].as_str() else {
        return;
    };
    let (Some(task), Some(caller_ref)) =
        (state["task"].as_str(), pending["caller"]["ref"].as_str())
    else {
        return;
    };
    if owner == caller_ref {
        return;
    }
    let result = (|| -> Result<()> {
        let body = json!({"ownerRef": owner, "caller": caller, "at": pending["at"], "round": round, "task": task});
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("supervisor.log"))?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["--mt-foreman", "__notify-owner", task])
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0200);
        }
        let mut child = command.spawn()?;
        serde_json::to_writer(child.stdin.take().context("通知输入不可用")?, &body)?;
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("第 {round} 轮无法通知发起会话：{error:#}");
    }
}

fn notify_owner(directory: &Path) -> Result<i32> {
    let body: Value = serde_json::from_str(&read_prompt(None)?)?;
    let owner = body["ownerRef"].as_str().context("缺少发起会话")?;
    let round = body["round"].as_u64().context("缺少任务轮次")?;
    let query = || -> Option<String> {
        let data: Value = serde_json::from_slice(&mt(&["list", "--all", "--json"]).ok()?).ok()?;
        Some(
            data["targets"]
                .as_array()?
                .iter()
                .find(|target| target["ref"] == owner)
                .map_or("not-found", |value| {
                    value["workStatus"].as_str().unwrap_or("")
                })
                .to_owned(),
        )
    };
    let mut status = query();
    let interval = std::env::var("FOREMAN_NOTIFY_RETRY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|&value| value <= 20_000)
        .unwrap_or(20_000);
    for _ in 0..6 {
        if !matches!(status.as_deref(), None | Some("needs-input" | "starting")) {
            break;
        }
        thread::sleep(Duration::from_millis(interval));
        status = query();
    }
    let reason = match status.as_deref() {
        Some("running" | "idle") => {
            let task = body["task"].as_str().unwrap_or("");
            let title = body["caller"]["title"]
                .as_str()
                .unwrap_or("")
                .chars()
                .filter(|ch| !ch.is_control())
                .take(200)
                .collect::<String>();
            let message = format!("任务 {task} 被 {title} 用 send 重新拉起一轮（第 {round} 轮）");
            if mt(&["send", owner, &message, "--enter"]).is_ok() {
                return Ok(0);
            }
            "未能向发起会话发送"
        }
        Some("needs-input" | "starting") => "所有者处于等待输入",
        Some("exited") => "所有者会话已退出",
        Some("error") => "所有者会话异常",
        Some("not-found") => "找不到所有者会话",
        _ => "所有者状态无法确认",
    };
    write_json(
        &mailbox_path(
            directory,
            "outcomes",
            &format!("owner-notification-{round}"),
        ),
        &json!({
            "round": round, "caller": body["caller"], "at": body["at"], "ownerWorkStatus": status, "reason": reason,
        }),
    )?;
    writeln!(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("feed.log"))?,
        "{} ✉ 外部 send（{reason}，未注入）",
        timestamp()
    )?;
    Ok(0)
}
