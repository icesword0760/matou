use super::*;
use matou_native_core::{
    dispatch_roles::{DispatchRolesConfig, DispatchRolesStore, format_roles_markdown},
    sessions::{get_session, list_runs},
};
use serde_json::{Value, json};

pub(super) fn fixture_foreman_assets(root: &Path) -> Result<PathBuf> {
    let assets = root.join("control assets");
    let script = assets.join("providers/claude-plugin/skills/agent-foreman/scripts/foreman.mjs");
    fs::create_dir_all(script.parent().unwrap())?;
    fs::write(script, "// Offline Foreman fixture")?;
    Ok(assets)
}

fn fixture(root: &Path, executable: &str, exit: Option<u8>) -> Result<(RuntimeChild, PathBuf)> {
    fixture_with_events(root, executable, exit, false)
}

pub(super) fn fixture_with_events(
    root: &Path,
    executable: &str,
    exit: Option<u8>,
    events: bool,
) -> Result<(RuntimeChild, PathBuf)> {
    let bin = root.join("fixture bin");
    let codex_home = root.join("隔离 Codex home");
    fs::create_dir(&bin)?;
    fs::create_dir(&codex_home)?;
    let assets = fixture_foreman_assets(root)?;
    let node = bin.join("node");
    fs::write(&node, "#!/bin/sh\nexit 0")?;
    fs::set_permissions(&node, fs::Permissions::from_mode(0o700))?;
    let stub = bin.join(executable);
    if executable != "missing-codex" {
        fs::write(&stub, include_bytes!("../fixtures/codex-launch.sh"))?;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    }
    let zshrc = format!(
        r#"PROMPT='{PROMPT} '
codex() {{
    printf '%s\0' "$@" >> "$HOME/shell-codex-argv"
    printf 'SHELL CODEX ARGUMENTS <%s>\n' "$1"
}}
claude() {{ printf 'SHELL CLAUDE <%s>\n' "$*"; }}
alias cc='claude --dangerously-skip-permissions'
"#
    );
    let mut child = RuntimeChild::spawn_configured_with(
        root,
        &zshrc,
        Path::new("/bin/zsh"),
        None,
        |command| {
            command
                .env("CODEX_HOME", &codex_home)
                .env("MATOU_CONTROL_ASSET_ROOT", &assets)
                .env("MATOU_FOREMAN_ROLES_FILE", "/stale/dispatch-roles.json")
                .env("MATOU_LOCALE", "zh-CN")
                .env("MATOU_PROVIDER_HOOK_TOKEN", "old-fixture-token")
                .env("MATOU_AGENT_STATUS_DIR", "/old-fixture/status")
                .env("CLAUDECODE", "1")
                .env_remove("OPENAI_API_KEY")
                .env_remove("CODEX_FIXTURE_EXIT_CODE")
                .env("CODEX_FIXTURE_EVENTS", if events { "1" } else { "0" });
            if executable == "codex" {
                // A desktop launcher can omit /usr/sbin and inherit the C locale.
                let path =
                    std::env::join_paths([bin.as_path(), Path::new("/usr/bin"), Path::new("/bin")])
                        .unwrap();
                command
                    .env_remove("MATOU_CODEX_COMMAND")
                    .env("PATH", path)
                    .env("LC_ALL", "C");
            } else {
                command.env("MATOU_CODEX_COMMAND", &stub);
            }
            if let Some(code) = exit {
                command.env("CODEX_FIXTURE_EXIT_CODE", code.to_string());
            }
        },
    )?;
    child.hello()?;
    Ok((child, codex_home))
}

fn graph_for(child: &mut RuntimeChild, profile: &str) -> Result<Value> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.event(deadline, true)? {
            Event::Graph(graph) if graph["nodes"][0]["currentMode"] == profile => return Ok(graph),
            Event::Diagnostic(error) => bail!("{error}"),
            _ => {}
        }
    }
}

fn assert_hook_argv(actual: &[String], expected: &[&str]) {
    assert_eq!(&actual[..actual.len() - 5], expected);
    let [dev_flag, dev_instructions, flag, hooks, trust] = &actual[actual.len() - 5..] else {
        unreachable!()
    };
    assert_eq!(dev_flag, "-c");
    assert!(dev_instructions.starts_with("developer_instructions="));
    assert!(dev_instructions.contains("mt identify --json"));
    assert_eq!(flag, "-c");
    assert_eq!(trust, "--dangerously-bypass-hook-trust");
    assert!(hooks.starts_with("hooks={ "));
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PermissionRequest",
        "PostToolUse",
        "Stop",
        "SessionEnd",
    ] {
        assert!(hooks.contains(&format!("{event} = [")), "{event}");
    }
}

fn mount(db: &Connection, session: &str) -> Result<(String, Option<String>)> {
    Ok(db.query_row(
        "SELECT id, scene_node_id FROM session_mounts WHERE session_id=?",
        [session],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

fn assert_same_card(child: &RuntimeChild, graph: &Value) {
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(graph["nodes"][0]["sessionId"], child.session);
    assert_eq!(graph["nodes"][0]["sceneId"], child.scene);
    assert_eq!(graph["focusedSessionId"], child.session);
}

fn assert_no_binding(db: &Connection, session: &str) -> Result<()> {
    let rows: i64 = db.query_row(
        "SELECT COUNT(*) FROM provider_bindings WHERE session_id=? AND provider='codex'",
        [session],
        |row| row.get(0),
    )?;
    assert_eq!(rows, 0, "PTY output cannot confirm provider identity");
    Ok(())
}

fn argv(path: &Path) -> Result<Vec<String>> {
    let data = fs::read_to_string(path)?;
    let data = data.strip_suffix('\0').context("terminated argv fixture")?;
    Ok(data.split('\0').map(str::to_owned).collect())
}

#[test]
fn codex_launch_promotes_exact_commands_on_the_same_card() -> Result<()> {
    for (executable, mode, parts, permission_flags) in [
        (
            "codex",
            "default",
            [&b"  co"[..], b"dex  ", b"\r"],
            &["--ask-for-approval", "on-request", "--sandbox", "read-only"][..],
        ),
        (
            "Codex fixture",
            "bypassPermissions",
            [
                &b" codex   --dangerously-bypass-"[..],
                b"approvals-and-sandbox  ",
                b"\r",
            ],
            &["--dangerously-bypass-approvals-and-sandbox"][..],
        ),
    ] {
        let root = tempfile::tempdir()?;
        let (mut child, codex_home) = fixture(root.path(), executable, None)?;
        fs::write(
            codex_home.join("config.toml"),
            "sandbox_mode = 'read-only' # root choice\n[profiles.test]\nsandbox_mode = 'danger-full-access'\n",
        )?;
        let roles = DispatchRolesStore::for_data_root(&child.data);
        let mut config = DispatchRolesConfig {
            default_agent: "dsh".into(),
            ..Default::default()
        };
        config.agents[1].role = "离线启动职责夹具".into();
        let saved = roles.save(config)?;
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        db.busy_timeout(TIMEOUT)?;
        let before_mount = mount(&db, &child.session)?;
        let old_pid = child.shell_pid()?;
        let cwd = child.workspace.join("项目 路径");
        fs::create_dir(&cwd)?;
        let cwd = cwd.canonicalize()?;
        child.input_bytes("cd '项目 路径'".as_bytes())?;
        child.input_bytes(b"\r")?;
        child.input("printf 'SHELL_CWD_%s\\n' VERIFIED")?;
        child.frame(|rows, _, _| rows.iter().any(|row| row == "SHELL_CWD_VERIFIED"))?;
        let deadline = Instant::now() + TIMEOUT;
        while get_session(&db, &child.session)?.unwrap().cwd != cwd.to_str().unwrap() {
            ensure!(
                Instant::now() < deadline,
                "Shell did not record the changed cwd"
            );
            thread::sleep(Duration::from_millis(10));
        }
        child.send(Command::Resize {
            session_id: child.session.clone(),
            cols: 76,
            rows: 20,
            cell_width_px: 7,
            cell_height_px: 14,
        })?;
        child.frame(|_, screen, _| screen.cols == 76 && screen.rows == 20)?;
        child.send(Command::Refresh)?;
        let before = graph_for(&mut child, "shell")?;
        assert_same_card(&child, &before);
        if mode == "bypassPermissions" {
            db.execute(
                "INSERT INTO provider_bindings (
                     id, session_id, provider, provider_session_id, resume_state,
                     metadata_json, created_at, updated_at
                 ) VALUES ('legacy-fixture', ?, 'claude-code', 'legacy-conversation',
                           'unknown', '{}', 1, 1)",
                [&child.session],
            )?;
            fs::write(
                child.data.join("provider-config.json"),
                serde_json::to_vec(&json!({
                    "version": 1,
                    "activeProviderIds": {"codex": "local-fixture"},
                    "providers": {"codex": [{
                        "id": "local-fixture", "endpoint": "http://127.0.0.1:9/v1/",
                        "model": "fixture-model", "reasoningEffort": "high"
                    }]}
                }))?,
            )?;
        }
        for bytes in parts {
            child.input_bytes(bytes)?;
        }
        child.input("offline input")?;
        let deadline = Instant::now() + TIMEOUT;
        let mut graph = None;
        let mut first_screen = false;
        let mut accepted_input = false;
        while !(graph.is_some() && first_screen && accepted_input) {
            match child.event(deadline, true)? {
                Event::Graph(value) if value["nodes"][0]["currentMode"] == "codex" => {
                    graph = Some(value)
                }
                Event::Frame {
                    session_id, screen, ..
                } => {
                    assert_eq!(session_id, child.session);
                    if screen_has_line(&screen, "FIXTURE CODEX TUI") {
                        assert_eq!((screen.cols, screen.rows), (76, 20));
                        first_screen = true;
                    }
                    accepted_input |=
                        screen_has_line(&screen, "FIXTURE CODEX INPUT <offline input>");
                }
                Event::Diagnostic(error) => bail!("{error}"),
                _ => {}
            }
        }
        let graph = graph.unwrap();
        assert_same_card(&child, &graph);
        child.send(Command::GetEgressStatus { request_id: 90 })?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Event::Graph(reply) = child.event(deadline, true)?
                && reply["type"] == "egressStatusResult"
                && reply["requestId"] == 90
            {
                assert_eq!(
                    reply["result"]["sessionFamilies"][&child.session],
                    if mode == "default" {
                        json!(["openai"])
                    } else {
                        Value::Null
                    }
                );
                break;
            }
        }
        assert_eq!(graph["nodes"][0]["workStatus"], "starting");
        assert_eq!(graph["nodes"][0]["canFork"], false);
        assert_eq!(mount(&db, &child.session)?, before_mount);
        assert_eq!(get_session(&db, &child.session)?.unwrap().kind, "codex");
        let runs = list_runs(&db, &child.session)?;
        assert_eq!(runs.len(), 2);
        assert_eq!(
            (runs[0].profile.as_str(), runs[0].status.as_str()),
            ("shell", "exited")
        );
        assert_eq!(
            (runs[1].profile.as_str(), runs[1].status.as_str()),
            ("codex", "running")
        );
        let new_pid = runs[1].pid.context("Codex PID")?;
        assert_ne!(new_pid, i64::from(old_pid));
        assert_shell_gone(old_pid)?;
        assert!(!root.path().join("Shell 家目录/shell-codex-argv").exists());
        let actual = argv(&codex_home.join("argv"))?;
        let mut expected = vec!["-c", "check_for_update_on_startup=false"];
        let endpoint = if mode == "bypassPermissions" {
            expected.extend([
                "-c",
                "model_reasoning_effort=\"high\"",
                "--model",
                "fixture-model",
            ]);
            "http://127.0.0.1:9/v1"
        } else {
            "https://api.openai.com/v1"
        };
        expected.extend_from_slice(permission_flags);
        assert_hook_argv(&actual, &expected);
        let injected: String = serde_json::from_str(
            actual[actual.len() - 4]
                .strip_prefix("developer_instructions=")
                .unwrap(),
        )?;
        assert!(injected.ends_with(&format_roles_markdown(&saved.config)));
        let dispatch_env = fs::read_to_string(codex_home.join("dispatch-environment"))?;
        let mut dispatch_env = dispatch_env.lines();
        assert_eq!(
            Path::new(dispatch_env.next().unwrap()).canonicalize()?,
            roles.path().canonicalize()?,
        );
        if executable == "codex" {
            assert_eq!(
                Path::new(dispatch_env.next().unwrap()).canonicalize()?,
                root.path().join("fixture bin/node").canonicalize()?
            );
        }
        assert_eq!(
            fs::read_to_string(codex_home.join("cwd"))?.trim_end(),
            cwd.to_str().unwrap()
        );
        let env = fs::read_to_string(codex_home.join("environment"))?;
        assert_eq!(
            env,
            format!("{}\nxterm-256color\n{endpoint}\n", codex_home.display())
        );
        assert_no_binding(&db, &child.session)?;
        if mode == "bypassPermissions" {
            let previous_binding = db.query_row(
                "SELECT resume_state, updated_at FROM provider_bindings WHERE id='legacy-fixture'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )?;
            assert_eq!(previous_binding, ("unknown".to_owned(), 1));
        }
        println!(
            "exact {mode}: old_pid={old_pid}, new_pid={new_pid}, cwd suffix=项目 路径, launch args={expected:?}, seven run hooks, screen=FIXTURE CODEX TUI, same card/node/grid, binding pending"
        );
        child.send(Command::Shutdown)?;
        assert!(child.closed()?.is_empty());
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "{stderr}");
    }
    Ok(())
}

#[test]
fn codex_launch_leaves_parameterized_commands_in_the_shell() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (mut child, codex_home) = fixture(root.path(), "Codex fixture", None)?;
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    db.busy_timeout(TIMEOUT)?;
    let initial_pid = child.shell_pid()?;
    for (command, output) in [
        (
            "codex --model '模型 有空格'",
            "SHELL CODEX ARGUMENTS <--model>",
        ),
        (
            "codex --dangerously-bypass-approvals-and-sandbox extra",
            "SHELL CODEX ARGUMENTS <--dangerously-bypass-approvals-and-sandbox>",
        ),
        ("printf '<%s>\\n' codex", "<codex>"),
        (
            "cc --print",
            "SHELL CLAUDE <--dangerously-skip-permissions --print>",
        ),
    ] {
        child.input(command)?;
        child.frame(|rows, _, _| rows.iter().any(|row| row == output))?;
        println!("ordinary command {command:?}: {output}, Shell unchanged");
    }
    assert_eq!(
        argv(&root.path().join("Shell 家目录/shell-codex-argv"))?,
        [
            "--model",
            "模型 有空格",
            "--dangerously-bypass-approvals-and-sandbox",
            "extra"
        ]
    );
    assert_eq!(list_runs(&db, &child.session)?.len(), 1);
    assert_eq!(child.shell_pid()?, initial_pid);
    assert!(!codex_home.join("argv").exists());
    assert_no_binding(&db, &child.session)?;
    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "{stderr}");
    Ok(())
}

#[test]
fn codex_launch_recovers_the_login_path_in_the_child() -> Result<()> {
    let root = tempfile::tempdir()?;
    let login_bin = root.path().join("login bin");
    fs::create_dir(&login_bin)?;
    let codex = login_bin.join("codex");
    fs::write(
        &codex,
        "#!/bin/sh\ncase \":$PATH:\" in *\":$LOGIN_BIN:\"*) ;; *) exit 96 ;; esac\nprintf 'CODEX LOGIN PATH READY\\n'\nIFS= read -r line\n",
    )?;
    fs::set_permissions(codex, fs::Permissions::from_mode(0o700))?;
    let mut child = RuntimeChild::spawn_configured_with(
        root.path(),
        &format!("PROMPT='{PROMPT} '\nexport PATH=\"$LOGIN_BIN:$PATH\"\n"),
        Path::new("/bin/zsh"),
        None,
        |command| {
            command
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("SHELL", "/bin/zsh")
                .env("LOGIN_BIN", &login_bin)
                .env_remove("MATOU_CODEX_COMMAND")
                .env_remove("MATOU_DISABLE_LOGIN_PATH_PROBE")
                .env_remove("ZDOTDIR");
        },
    )?;
    child.hello()?;
    child.input("codex")?;
    child.frame(|rows, _, _| rows.iter().any(|row| row == "CODEX LOGIN PATH READY"))?;
    child.send(Command::Refresh)?;
    let graph = graph_for(&mut child, "codex")?;
    assert_same_card(&child, &graph);
    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "{stderr}");
    Ok(())
}

#[test]
fn codex_launch_rolls_back_missing_and_immediately_exiting_commands() -> Result<()> {
    for (executable, exit, expected) in [
        (
            "missing-codex",
            None,
            "未检测到 Codex（命令：missing-codex）。请先安装并确保 `missing-codex` 在 PATH 中，再重试。",
        ),
        (
            "Codex fixture",
            Some(23),
            "Codex 启动失败：process exited with code 23",
        ),
    ] {
        let root = tempfile::tempdir()?;
        // The PTY reports a physical cwd. Start with that same spelling even
        // when TMPDIR is a symlink, before the runtime's first cwd observation.
        let (mut child, codex_home) = fixture(&root.path().canonicalize()?, executable, exit)?;
        let db = Connection::open(child.data.join("matou.sqlite"))?;
        db.busy_timeout(TIMEOUT)?;
        let session = child.session.clone();
        let before_mount = mount(&db, &session)?;
        let old_pid = child.shell_pid()?;
        let before = get_session(&db, &session)?.unwrap().cwd;
        child.input("codex")?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Event::Diagnostic(message) = child.event(deadline, true)? {
                assert_eq!(message, expected);
                break;
            }
        }
        child.frame(|rows, _, _| rows.iter().any(|row| row == PROMPT))?;
        child.input("printf 'FALLBACK_SHELL_%s\\n' VERIFIED")?;
        child.frame(|rows, _, _| rows.iter().any(|row| row == "FALLBACK_SHELL_VERIFIED"))?;
        child.input("printf 'FALLBACK_PID=%s\\n' \"$$\"")?;
        let mut new_pid: Option<u32> = None;
        child.frame(|rows, _, _| {
            new_pid = rows
                .iter()
                .find_map(|row| row.strip_prefix("FALLBACK_PID=")?.parse().ok());
            new_pid.is_some()
        })?;
        let new_pid = new_pid.context("replacement Shell PID")?;
        assert_ne!(old_pid, new_pid);
        assert_shell_gone(old_pid)?;
        child.send(Command::Refresh)?;
        let graph = graph_for(&mut child, "shell")?;
        assert_same_card(&child, &graph);
        assert_eq!(mount(&db, &session)?, before_mount);
        let card = get_session(&db, &session)?.unwrap();
        assert_eq!((card.kind.as_str(), card.cwd), ("shell", before));
        let runs = list_runs(&db, &session)?;
        assert_eq!(runs.len(), if exit.is_some() { 3 } else { 2 });
        assert_eq!(
            (runs[0].profile.as_str(), runs[0].status.as_str()),
            ("shell", "exited")
        );
        if let Some(code) = exit {
            assert_eq!(
                (
                    runs[1].profile.as_str(),
                    runs[1].status.as_str(),
                    runs[1].exit_code
                ),
                ("codex", "exited", Some(i64::from(code)))
            );
            assert_shell_gone(u32::try_from(runs[1].pid.context("early Codex PID")?)?)?;
            assert_hook_argv(
                &argv(&codex_home.join("argv"))?,
                &[
                    "-c",
                    "check_for_update_on_startup=false",
                    "--ask-for-approval",
                    "on-request",
                    "--sandbox",
                    "workspace-write",
                ],
            );
        }
        let current = runs.last().unwrap();
        assert_eq!(
            (
                current.profile.as_str(),
                current.status.as_str(),
                current.pid
            ),
            ("shell", "running", Some(i64::from(new_pid)))
        );
        assert_eq!(codex_home.join("argv").exists(), exit.is_some());
        assert_no_binding(&db, &session)?;
        println!(
            "fallback {executable:?} exit={exit:?}: {expected}, same card/node/cwd, old_pid={old_pid}, new_pid={new_pid}, Shell input verified"
        );
        child.send(Command::Shutdown)?;
        assert!(child.closed()?.is_empty());
        let (status, stderr) = child.wait()?;
        ensure!(status.success(), "{stderr}");
    }
    Ok(())
}
