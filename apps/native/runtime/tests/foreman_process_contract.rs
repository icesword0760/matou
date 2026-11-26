#![cfg(unix)]

use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

const RUNTIME: &str = env!("CARGO_BIN_EXE_matou-native-runtime");

struct Fixture {
    _temporary: tempfile::TempDir,
    base: PathBuf,
    cwd: PathBuf,
    runs: PathBuf,
    bin: PathBuf,
    brief: PathBuf,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().join("派活 Contract 中文");
        let cwd = base.join("旧工作区 with space");
        let runs = base.join("运行记录");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir(base.join("empty-path")).unwrap();
        fs::create_dir(base.join("agent-status")).unwrap();
        let bin = base.join("我的 CLI");
        fs::write(&bin, format!("#!/bin/sh\n{script}")).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let brief = base.join("真实 交代书.md");
        fs::write(&brief, "验证旧工作区，在现有权限内完成。\n").unwrap();
        Self {
            _temporary: temporary,
            base,
            cwd,
            runs,
            bin,
            brief,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(RUNTIME);
        command
            .env_clear()
            .args(["--mt-foreman"])
            .current_dir(&self.base)
            .env("HOME", &self.base)
            .env("PATH", self.base.join("empty-path"))
            .env("FOREMAN_RUN_DIR", &self.runs)
            .env("MATOU_AGENT_STATUS_DIR", self.base.join("agent-status"))
            .env("CODEX_BIN", &self.bin)
            .env("DSH_BIN", &self.bin)
            .env("AGY_BIN", &self.bin);
        command
    }

    fn start(&self, task: &str, flags: &[&str]) -> Command {
        let mut command = self.command();
        command
            .args(["start", task, "--cd"])
            .arg(&self.cwd)
            .arg("--brief")
            .arg(&self.brief)
            .args(flags);
        command
    }

    fn run(&self, argv: &[&str], code: i32) -> String {
        check(self.command().args(argv).output().unwrap(), code)
    }

    fn state(&self, task: &str) -> Value {
        serde_json::from_slice(&fs::read(self.runs.join(task).join("state.json")).unwrap()).unwrap()
    }

    fn wait_state(&self, task: &str, name: &str) -> Value {
        let started = Instant::now();
        loop {
            let state = self.state(task);
            if state["status"] == name {
                return state;
            }
            assert!(
                started.elapsed() < Duration::from_secs(8),
                "等待 {task} 的 {name} 状态超时：{state}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn check(out: Output, code: i32) -> String {
    assert_eq!(
        out.status.code(),
        Some(code),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn start_runs_codex_with_existing_scope_and_model_argv_and_persists_the_reply() {
    let fixture = Fixture::new(
        r#"
if [ "$1" = debug ]; then
  printf '%s\n' '{"models":[{"slug":"gpt-6.2-sol","visibility":"list","supported_in_api":true,"supported_reasoning_levels":[{"effort":"high"}]},{"slug":"gpt-6.1-terra","visibility":"list","supported_in_api":true,"supported_reasoning_levels":["high"]}]}'
  exit 0
fi
printf '%s\n' "$@" > actual-argv.txt
printf '%s\n' "$MATOU_FOREMAN_DEPTH/$MATOU_FOREMAN_PARENT_AGENT" > child-env.txt
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    reply_file=$1
  fi
  shift
done
while IFS= read -r line; do
  printf '%s\n' "$line"
done > actual-prompt.txt
printf '%s\n' '完成。' '我自己做的决定：无' > "$reply_file"
printf '%s\n' '{"type":"thread.started","thread_id":"fixture-codex-thread"}'
printf '%s\n' '{"type":"turn.completed"}'
printf '%s\n' 'fixture diagnostic' >&2
"#,
    );
    let task = "示例-codex";
    let extra = fixture.base.join("额外目录 with spaces");
    fs::create_dir(&extra).unwrap();
    let watch = check(
        fixture
            .start(
                task,
                &[
                    "--sandbox",
                    "--network",
                    "--no-project-doc",
                    "--speed",
                    "fast",
                ],
            )
            .arg("--add-dir")
            .arg(&extra)
            .output()
            .unwrap(),
        0,
    );
    assert!(watch.contains("sol → gpt-6.2-sol"));
    assert!(watch.contains("界面操作子代理 → gpt-6.1-terra"));
    assert!(watch.contains("本轮完成"));
    assert!(watch.contains("我自己做的决定：无"));
    let state = fixture.state(task);
    assert_eq!(state["status"], "completed");
    assert_eq!(state["agent"], "codex");
    assert_eq!(state["threadId"], "fixture-codex-thread");
    assert_eq!(state["options"]["gitRoot"], Value::Null);
    assert_eq!(state["options"]["fullAccess"], false);
    assert_eq!(state["turns"][0]["result"], "completed");
    let args = read(fixture.cwd.join("actual-argv.txt"));
    for expected in [
        "-s\nworkspace-write",
        "-m\ngpt-6.2-sol",
        "model_reasoning_effort=\"high\"",
        "service_tier=\"fast\"",
        "sandbox_workspace_write.network_access=true",
        "project_doc_max_bytes=0",
        extra.to_str().unwrap(),
        fixture.cwd.to_str().unwrap(),
    ] {
        assert!(args.contains(expected), "{expected}: {args}");
    }
    let prompt = read(fixture.cwd.join("actual-prompt.txt"));
    assert!(prompt.contains("mt-foreman inbox 示例-codex"));
    assert!(prompt.contains("mt-foreman ask 示例-codex --message \"问题.md\""));
    assert!(prompt.contains("验证旧工作区"));
    assert_eq!(read(fixture.cwd.join("child-env.txt")), "1/codex\n");
    assert!(
        read(state["turns"][0]["files"]["events"].as_str().unwrap())
            .contains("fixture-codex-thread")
    );
    assert!(
        read(state["turns"][0]["files"]["stderr"].as_str().unwrap()).contains("fixture diagnostic")
    );
    assert!(fixture.run(&["watch", task], 0).contains("Codex 回复"));
    let indicator = fs::read_dir(fixture.base.join("agent-status"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let indicator: Value = serde_json::from_str(&read(indicator)).unwrap();
    assert_eq!(indicator["state"], "completed");
    assert!(indicator["label"].as_str().unwrap().contains("快速"));
}

#[test]
fn dsh_retries_a_transient_error_and_passes_the_thread_to_the_next_worker() {
    let fixture = Fixture::new(
        r#"
printf '%s\n' "$@" > "actual-argv$([ -f attempt-first ] && printf '%s' '-second').txt"
while IFS= read -r line; do :; done
if [ ! -f attempt-first ]; then
  printf '%s' yes > attempt-first
  printf '%s\n' '{"type":"session","sessionId":"fixture-dsh-thread"}' '{"type":"error","message":"network disconnected"}'
  exit 1
fi
printf '%s\n' '{"type":"final","reason":{"kind":"completed"},"text":"DSH 已完成。 我自己做的决定：无。"}'
"#,
    );
    let task = "示例-dsh";
    let watch = check(
        fixture
            .start(
                task,
                &[
                    "--agent",
                    "dsh",
                    "--model",
                    "pro",
                    "--effort",
                    "max",
                    "--retries",
                    "1",
                    "--backoff",
                    "0",
                    "--sandbox",
                ],
            )
            .output()
            .unwrap(),
        0,
    );
    assert!(watch.contains("繁忙或断线：network disconnected"));
    assert!(watch.contains("DSH 已完成"));
    let state = fixture.state(task);
    assert_eq!(state["options"]["model"], "deepseek-v4-pro");
    assert_eq!(state["turns"][0]["result"], "retryable");
    assert_eq!(state["turns"][1]["result"], "completed");
    assert_eq!(state["turns"][1]["resume"], true);
    let args = read(fixture.cwd.join("actual-argv-second.txt"));
    assert!(args.contains("--profile\nheadless"));
    assert!(args.contains("--permission-mode\nworkspace-write"));
    assert!(args.contains("--session-id\nfixture-dsh-thread"));
    let patch = read(fixture.runs.join(task).join("dsh.patch.yml"));
    assert!(patch.contains("model: \"deepseek-v4-pro\""));
    assert!(patch.contains("reasoningEffort: \"max\""));
}

#[test]
fn antigravity_uses_dynamic_model_and_finishes_if_success_arrives_before_process_exit() {
    let fixture = Fixture::new(
        r#"
if [ "$1" = models ]; then
  printf '%s\n' 'gemini-3.1-hydrogen-high' 'gemini-3.5-hydrogen-high'
  exit 0
fi
printf '%s\n' "$@" > actual-argv.txt
printf '%s\n' '{"event":"init","conversation_id":"fixture-agy-conversation"}'
printf '%s\n' '{"event":"result","result":{"status":"SUCCESS","response":"我自己做的决定：无。"}}'
exec /bin/sleep 45
"#,
    );
    let task = "示例-agy";
    let output = fixture
        .start(
            task,
            &[
                "--agent",
                "agy",
                "--model",
                "hydrogen",
                "--effort",
                "high",
                "--sandbox",
            ],
        )
        .env("FOREMAN_AGY_RESULT_EXIT_MS", "30")
        .output()
        .unwrap();
    let watch = check(output, 0);
    assert!(watch.contains("hydrogen → gemini-3.5-hydrogen-high"));
    let state = fixture.state(task);
    assert_eq!(state["threadId"], "fixture-agy-conversation");
    assert_eq!(state["status"], "completed");
    let args = read(fixture.cwd.join("actual-argv.txt"));
    assert!(args.contains("--model\ngemini-3.5-hydrogen-high"));
    assert!(args.contains("--sandbox"));
    assert!(args.contains("--output-format\nstream-json"));
}

#[test]
fn start_is_serialized_and_stop_cancels_a_detached_worker() {
    let fixture = Fixture::new(
        r#"
printf '%s\n' '{"type":"session","sessionId":"fixture-stop-thread"}'
printf '%s\n' "$$" > actual-pid.txt
exec /bin/sleep 45
"#,
    );
    let task = "示例-stop";
    let launch = || {
        fixture
            .start(task, &["--agent", "dsh", "--no-watch"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let a = launch();
    let b = launch();
    let mut results = [a.wait_with_output().unwrap(), b.wait_with_output().unwrap()];
    results.sort_by_key(|out| out.status.code());
    assert_eq!(results[0].status.code(), Some(0));
    assert_eq!(results[1].status.code(), Some(1));
    assert!(String::from_utf8_lossy(&results[1].stderr).contains("already exists"));
    let state = fixture.wait_state(task, "running");
    assert_eq!(state["turns"].as_array().unwrap().len(), 1);
    assert!(state["supervisorPid"].as_u64().is_some());
    fixture.run(&["stop", task], 0);
    let stopped = fixture.state(task);
    assert_eq!(stopped["status"], "stopped");
    assert_eq!(stopped["turns"][0]["result"], "stopped");
    assert_eq!(stopped["pid"], Value::Null);
    assert!(fixture.run(&["watch", task], 5).contains("已手动停止"));
}

#[test]
fn start_reports_missing_agent_config_conflicts_and_watchdog_codes() {
    let fixture = Fixture::new(
        r#"
printf '%s\n' '{"type":"session","sessionId":"fixture-timeout-thread"}'
exec /bin/sleep 45
"#,
    );
    let missing = fixture
        .start("测试-缺少", &["--agent", "dsh"])
        .env("DSH_BIN", "missing-fixture-cli")
        .output()
        .unwrap();
    assert!(check(missing, 2).contains("未找到 dsh CLI"));
    assert_eq!(fixture.state("测试-缺少")["turns"][0]["result"], "fatal");
    fs::create_dir_all(fixture.base.join(".codex")).unwrap();
    fs::write(
        fixture.base.join(".codex/config.toml"),
        "model_instructions_file = \"test-override.md\"\n",
    )
    .unwrap();
    let rejected = fixture.start("测试-拒绝", &[]).output().unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("自定义指令文件"));
    assert!(!fixture.runs.join("测试-拒绝/state.json").exists());

    let timeout = check(
        fixture
            .start("测试-超时", &["--agent", "dsh", "--max-min", "0.005"])
            .output()
            .unwrap(),
        4,
    );
    assert!(timeout.contains("超时停下"));
    let stalled = check(
        fixture
            .start(
                "测试-卡住",
                &["--agent", "dsh", "--idle-min", "0.005", "--retries", "0"],
            )
            .output()
            .unwrap(),
        3,
    );
    assert!(stalled.contains("连续"));
    assert_eq!(fixture.state("测试-卡住")["turns"][0]["result"], "stalled");
}

#[test]
fn config_and_models_show_effective_values_and_reuse_the_model_cache() {
    let fixture = Fixture::new(
        r#"
printf '%s\n' '{"models":[{"slug":"gpt-6.2-sol","visibility":"list","supported_in_api":true,"supported_reasoning_levels":["high"]}]}'
"#,
    );
    let defaults = fixture.base.join("config.json");
    let empty: Value = serde_json::from_str(&fixture.run(&["config"], 0)).unwrap();
    assert_eq!(empty["exists"], false);
    assert_eq!(empty["defaults"]["agent"], "codex");

    fs::write(
        &defaults,
        r#"{"version":1,"defaults":{"agent":"dsh","effort":"xhigh","retries":2,"unknown":true}}"#,
    )
    .unwrap();
    let configured: Value = serde_json::from_str(&fixture.run(&["config"], 0)).unwrap();
    assert_eq!(configured["exists"], true);
    assert_eq!(configured["defaults"]["agent"], "dsh");
    assert_eq!(configured["defaults"]["effort"], "high");
    assert_eq!(configured["defaults"]["retries"], 2);
    let ignored = configured["ignored"].as_array().unwrap();
    assert!(
        ignored
            .iter()
            .any(|notice| notice.as_str().unwrap().contains("DSH 不支持"))
    );
    assert!(
        ignored
            .iter()
            .any(|notice| notice.as_str().unwrap().contains("unknown"))
    );

    let first = fixture.run(&["models", "--model", "SOL", "--effort", "high"], 0);
    assert!(first.contains("gpt-6.2-sol\t主力\t6.2\t是\tlist"));
    assert!(first.contains("刚刚解析"));
    let cached = check(
        fixture
            .command()
            .args(["models", "--model", "sol"])
            .env("CODEX_BIN", "no-codex-installed-here")
            .output()
            .unwrap(),
        0,
    );
    assert!(cached.contains("sol → gpt-6.2-sol"));
    assert!(cached.contains("来自缓存"));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_network_activity_prevents_a_false_stall_during_silent_generation() {
    let fixture = Fixture::new(
        r#"
printf '%s\n' '{"type":"session","sessionId":"fixture-network-thread"}'
/bin/sleep 7
printf '%s\n' '{"type":"final","reason":{"kind":"completed"},"text":"完成。 我自己做的决定：无。"}'
"#,
    );
    let nettop = fixture.base.join("系统 nettop 夹具");
    fs::write(
        &nettop,
        r#"#!/bin/sh
marker="$HOME/sample-published"
if [ -f "$marker" ]; then
  bytes=8192
else
  : > "$marker"
  bytes=0
fi
printf '%s\n' 'time,bytes_in,'
while [ "$#" -gt 0 ]; do
  if [ "$1" = -p ]; then
    shift
    printf 'fixture.%s,%s,\n' "$1" "$bytes"
  fi
  shift
done
"#,
    )
    .unwrap();
    fs::set_permissions(&nettop, fs::Permissions::from_mode(0o755)).unwrap();
    let output = fixture
        .start(
            "示例-静默输出",
            &[
                "--agent",
                "dsh",
                "--warn-min",
                "0.025",
                "--idle-min",
                "0.1",
                "--retries",
                "0",
            ],
        )
        .env("FOREMAN_NETTOP", nettop)
        .output()
        .unwrap();
    let watch = check(output, 0);
    assert!(watch.contains("没有动静（网络也没有数据）"));
    assert!(watch.contains("恢复活动：生成中"));
    assert!(watch.contains("本轮完成"));
}
