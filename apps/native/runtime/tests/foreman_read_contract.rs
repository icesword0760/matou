use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const BINARY: &str = env!("CARGO_BIN_EXE_matou-native-runtime");
const RUNNING: &str = "A-进行中";
const WAITING: &str = "B-等待回答";
const COMPLETED: &str = "C-已完成";
const FAILED: &str = "D-失败可续跑";

struct Fixture {
    _temporary: tempfile::TempDir,
    base: PathBuf,
    runs: PathBuf,
}

impl Fixture {
    fn empty() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().join("Foreman 中文目录 with spaces");
        fs::create_dir_all(base.join("empty path")).unwrap();
        let runs = base.join("旧任务 runs");
        Self {
            _temporary: temporary,
            base,
            runs,
        }
    }

    fn legacy() -> Self {
        let fixture = Self::empty();
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/foreman"),
            &fixture.runs,
        );
        let running = fixture.runs.join(RUNNING);
        fs::rename(running.join("feed.log.fixture"), running.join("feed.log")).unwrap();
        let pid = std::process::id();
        for task in [RUNNING, WAITING] {
            fixture.update_state(task, |state| {
                state["pid"] = json!(pid);
                state["supervisorPid"] = json!(pid);
            });
        }
        let date: String = rusqlite::Connection::open_in_memory()
            .unwrap()
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
                row.get(0)
            })
            .unwrap();
        let question_file = fixture.runs.join(WAITING).join("questions/q-1.json");
        let mut question = read_json(&question_file);
        question["pid"] = json!(pid);
        question["createdAt"] = json!(date);
        write_json(&question_file, &question);
        for task in [COMPLETED, FAILED] {
            fixture.update_state(task, |state| {
                let last = state["turns"].as_array_mut().unwrap().last_mut().unwrap();
                last["files"]["last"] = json!(
                    fixture
                        .runs
                        .join(task)
                        .join(last["files"]["last"].as_str().unwrap())
                );
            });
        }
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::new(BINARY);
        command
            .env_clear()
            .args(["--mt-foreman"])
            .current_dir(&self.base)
            .env("TZ", "UTC")
            .env("HOME", &self.base)
            .env("USERPROFILE", &self.base)
            .env("PATH", self.base.join("empty path"))
            .env("FOREMAN_RUN_DIR", self.runs.file_name().unwrap());
        #[cfg(windows)]
        for key in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
    }

    fn invoke(&self, args: &[&str], code: i32) -> String {
        stdout(self.command().args(args).output().unwrap(), code)
    }

    fn state_path(&self, task: &str) -> PathBuf {
        self.runs.join(task).join("state.json")
    }

    fn update_state(&self, task: &str, update: impl FnOnce(&mut Value)) {
        let file = self.state_path(task);
        let mut state = read_json(&file);
        update(&mut state);
        // Publish the same rename pattern used by the old supervisor.
        let staging = file.with_extension("tmp");
        write_json(&staging, &state);
        fs::rename(staging, file).unwrap();
    }
}

fn stdout(output: Output, code: i32) -> String {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if code != 1 {
        assert!(output.stderr.is_empty());
    }
    String::from_utf8(output.stdout).unwrap()
}

fn copy_dir(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn commands_work_with_empty_path_and_the_legacy_home_layout() {
    let fixture = Fixture::empty();
    assert_eq!(fixture.invoke(&["status"], 0), "暂无任务\n");
    assert!(!fixture.runs.exists());
    fs::create_dir(&fixture.runs).unwrap();
    assert_eq!(fixture.invoke(&["status"], 0), "暂无任务\n");

    let legacy = fixture.base.join(".claude/agent-foreman/runs/旧任务");
    fs::create_dir_all(&legacy).unwrap();
    write_json(&legacy.join("state.json"), &json!({ "status": "stopped" }));
    for run_dir in [None, Some("")] {
        let mut command = fixture.command();
        if let Some(value) = run_dir {
            command.env("FOREMAN_RUN_DIR", value);
        } else {
            command.env_remove("FOREMAN_RUN_DIR");
        }
        assert_eq!(
            stdout(command.arg("status").output().unwrap(), 0),
            "旧任务\tstopped\tcodex\n"
        );
    }

    assert!(fixture.invoke(&["guide"], 0).contains("## 派活颗粒度"));
    assert!(
        fixture
            .invoke(&["guide", "brief"], 0)
            .contains("一句话说清要交付什么")
    );
    assert!(
        fixture
            .invoke(&["guide", "parallel"], 0)
            .contains("怎么隔离")
    );
    assert!(
        fixture
            .invoke(&["guide", "reference"], 0)
            .contains("watch 退出码")
    );
    assert!(fixture.invoke(&["roles"], 0).contains("当前未配置"));
    assert!(fixture.invoke(&["--help"], 0).contains("mt-foreman watch"));
    assert!(fixture.invoke(&["guide", "nonexistent"], 1).is_empty());
    assert!(fixture.invoke(&["status", "../outside"], 1).is_empty());
}

#[test]
fn guide_and_roles_use_the_configured_matou_role_document() {
    let fixture = Fixture::empty();
    let roles = fixture.base.join("dispatch-roles.json");
    write_json(
        &roles,
        &json!({
            "version": 1,
            "allowSubDispatch": false,
            "agents": [
                { "id": "codex", "enabled": true, "model": "sol", "effort": "high", "mode": "auto", "speed": "fast", "role": "接口和测试" },
                { "id": "dsh", "enabled": true, "model": "deepseek-flash", "effort": "high", "mode": "auto", "role": "补产品文案" },
                { "id": "agy", "enabled": false, "model": "gemini-pro-orchestrator", "effort": "high", "mode": "explicit", "role": "" }
            ]
        }),
    );
    write_json(
        &fixture.base.join("config.json"),
        &json!({ "version": 1, "defaults": { "agent": "dsh" } }),
    );
    let invoke = |args: &[&str]| {
        stdout(
            fixture
                .command()
                .env("MATOU_FOREMAN_ROLES_FILE", &roles)
                .args(args)
                .output()
                .unwrap(),
            0,
        )
    };
    let role_text = invoke(&["roles"]);
    assert!(role_text.contains("**DSH**（默认兜底主力"));
    assert!(role_text.contains("补产品文案"));
    assert!(role_text.contains("速度: 快速"));
    assert!(role_text.contains("已停用（禁止派活）"));
    assert!(role_text.contains("二次转包策略**：已关闭"));
    assert!(invoke(&["guide"]).ends_with(&role_text));
    assert!(!invoke(&["guide", "brief"]).contains("默认兜底主力"));
}

#[test]
fn reads_legacy_history_and_owner_notifications_without_rewriting() {
    let fixture = Fixture::legacy();
    fs::create_dir(fixture.runs.join("未公布的任务")).unwrap();
    fs::write(fixture.runs.join("readme"), "").unwrap();
    assert_eq!(
        fixture.invoke(&["status"], 0),
        "A-进行中\trunning\tagy\nB-等待回答\trunning\tdsh\nC-已完成\tcompleted\tcodex\nD-失败可续跑\tfailed\tdsh\n"
    );

    let completed_before = fs::read(fixture.state_path(COMPLETED)).unwrap();
    let status = fixture.invoke(&["status", COMPLETED], 0);
    let (report, reply) = status.split_once("\n--- last message ---\n").unwrap();
    assert!(reply.contains("已完成两项夹具检查。"));
    let report: Value = serde_json::from_str(report).unwrap();
    assert_eq!(report["agent"], "codex");
    assert_eq!(
        report["turns"],
        json!([{ "n": 1, "result": "completed" }, { "n": 2, "result": "completed", "error": null }])
    );
    assert_eq!(report["threadId"], "fixture-completed-thread");
    assert_eq!(
        report["lastExternalSend"]["caller"]["ref"],
        "caller-fixture"
    );
    assert_eq!(
        report["pendingOwnerNotifications"],
        json!([{
            "round": 2,
            "caller": { "ref": "caller-fixture", "title": "测试调用会话", "provider": "dsh" },
            "at": "2026-11-24T00:03:00.000Z",
            "ownerWorkStatus": "needs-input",
            "reason": "所有者处于等待输入"
        }])
    );
    let watch = fixture.invoke(&["watch", COMPLETED], 0);
    assert!(watch.contains("第 2 轮由 测试调用会话 拉起（所有者处于等待输入，未注入）"));
    assert!(watch.contains("本轮完成（第 2 轮，用时 1分15秒）"));
    assert!(watch.contains("Codex 回复"));
    assert!(watch.contains("已完成两项夹具检查。"));
    assert_eq!(
        fs::read(fixture.state_path(COMPLETED)).unwrap(),
        completed_before
    );

    let failed_before = fs::read(fixture.state_path(FAILED)).unwrap();
    let watch = fixture.invoke(&["watch", FAILED], 2);
    assert!(watch.contains("失败：测试服务暂不可用"));
    assert!(watch.contains("已保存恢复点。"));
    assert!(!watch.contains("仅第一行"));
    let failed_status = fixture.invoke(&["status", FAILED], 0);
    assert!(failed_status.contains("fixture-resumable-thread"));
    assert_eq!(fs::read(fixture.state_path(FAILED)).unwrap(), failed_before);
    let running = fixture.invoke(&["status", RUNNING], 0);
    let running: Value = serde_json::from_str(&running).unwrap();
    assert_eq!(running["alive"], true);
    assert_eq!(running["effort"], "high");
}

#[test]
fn watch_uses_mailbox_before_stale_waiting_state_and_preserves_timeouts() {
    let fixture = Fixture::legacy();
    let before = fs::read(fixture.state_path(WAITING)).unwrap();
    let asking = fixture.invoke(&["watch", WAITING], 7);
    assert!(asking.contains("正在等你回答 #1：选择测试模式？"));
    assert!(asking.contains("Foreman 中文目录 with spaces"));
    assert!(asking.contains("q-1.json"));
    assert!(asking.contains("mt-foreman answer B-等待回答 1 --message \"回答.md\""));
    assert!(!asking.contains("这是已脱敏的第二行"));

    let answers = fixture.runs.join(WAITING).join("answers");
    fs::create_dir_all(&answers).unwrap();
    write_json(&answers.join("a-1.json"), &json!({ "text": "标准模式" }));
    let answered = fixture.invoke(&["watch", WAITING, "--watch-min", "0.0001"], 8);
    assert!(answered.contains("已回答 #1，等待 DSH 继续"));
    assert!(answered.contains("mt-foreman watch B-等待回答"));
    assert_eq!(fs::read(fixture.state_path(WAITING)).unwrap(), before);

    for (status, code, message) in [("timeout", 4, "超时停下"), ("stopped", 5, "已手动停止")]
    {
        fixture.update_state(FAILED, |state| {
            state["status"] = json!(status);
            state.as_object_mut().unwrap().remove("exitCode");
        });
        let watch = fixture.invoke(&["watch", FAILED], code);
        assert!(watch.contains(message));
        assert_eq!(watch.contains("已保存恢复点。"), status == "timeout");
    }
}

#[test]
fn watch_tails_only_new_feed_while_a_legacy_supervisor_publishes_completion() {
    let fixture = Fixture::legacy();
    let mut watch = fixture
        .command()
        .args(["watch", RUNNING, "--watch-min", "0.05"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(watch.stdout.take().unwrap());
    let mut text = String::new();
    output.read_line(&mut text).unwrap();
    assert!(text.contains("进行中：检查项目"), "{text}");
    let mut feed = fs::OpenOptions::new()
        .append(true)
        .open(fixture.runs.join(RUNNING).join("feed.log"))
        .unwrap();
    writeln!(feed, "08:02:00 新追加的旧监督进程事件：已完成").unwrap();
    drop(feed);
    fixture.update_state(RUNNING, |state| {
        state["status"] = json!("completed");
        state["exitCode"] = json!(0);
        state["pid"] = Value::Null;
        state["supervisorPid"] = Value::Null;
    });
    output.read_to_string(&mut text).unwrap();
    assert!(watch.wait_with_output().unwrap().status.success());
    assert!(text.contains("新追加的旧监督进程事件：已完成"));
    assert!(!text.contains("第 1 轮开始"));
}

#[test]
fn orphaned_old_task_is_reported_without_rewriting_or_creating_files() {
    let fixture = Fixture::legacy();
    fixture.update_state(RUNNING, |state| {
        state["pid"] = Value::Null;
        state["supervisorPid"] = Value::Null;
    });
    let before = fs::read(fixture.state_path(RUNNING)).unwrap();
    for _ in 0..2 {
        let status = fixture.invoke(&["status", RUNNING], 0);
        let view: Value = serde_json::from_str(&status).unwrap();
        assert_eq!(view["status"], "failed");
        assert_eq!(view["alive"], false);
        assert_eq!(view["error"], "监工进程意外退出（未留下结束状态）");
        assert!(
            fixture
                .invoke(&["watch", RUNNING], 6)
                .contains("监督进程意外退出，详见")
        );
        assert_eq!(fs::read(fixture.state_path(RUNNING)).unwrap(), before);
        assert!(!fixture.runs.join(RUNNING).join("supervisor.log").exists());
    }
}
