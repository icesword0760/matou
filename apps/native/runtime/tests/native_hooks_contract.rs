use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::Result;
use serde_json::{Value, json};

fn runtime_binary() -> PathBuf {
    std::env::var_os("MATOU_TEST_RUNTIME_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_matou-native-runtime").into())
}

fn hook(root: &Path, input: &[u8], worker: bool) -> Result<Value> {
    let mut child = Command::new(runtime_binary())
        .args(["--hook", "claude-session-start"])
        .env("PATH", root)
        .env("MATOU_DATA_ROOT", root)
        .env_remove("MATOU_PROVIDER_HOOK_URL")
        .env_remove("MATOU_PROVIDER_HOOK_TOKEN")
        .env("MATOU_FOREMAN_DEPTH", if worker { "1" } else { "" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(input)?;
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn claude_session_start_runs_with_an_empty_path_on_all_lifecycle_sources() -> Result<()> {
    use matou_native_core::dispatch_roles::{DispatchRolesConfig, DispatchRolesStore};
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("空格 码头");
    fs::create_dir(&root)?;
    let mut roles = DispatchRolesConfig::default();
    roles.default_agent = "agy".into();
    roles.agents[0].role = "用户保存的角色".into();
    DispatchRolesStore::for_data_root(&root).save(roles)?;
    for source in ["startup", "resume", "clear", "compact"] {
        let output = hook(
            &root,
            &serde_json::to_vec(&json!({"hook_event_name": "SessionStart", "source": source}))?,
            false,
        )?;
        let hook = &output["hookSpecificOutput"];
        assert_eq!(hook["hookEventName"], "SessionStart");
        let context = hook["additionalContext"].as_str().unwrap();
        assert!(context.contains("mt-foreman roles"));
        assert!(context.contains("用户保存的角色"));
        assert!(context.contains("反重力）**（默认兜底主力"));
        assert!(context.contains("mt ctx resume"));
    }

    let output = hook(&root, b"{}", true)?;
    let context = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(!context.contains("mt-foreman"));
    assert!(context.contains("mt ctx resume"));

    fs::write(root.join("dispatch-roles.json"), "{bad")?;
    fs::write(
        root.join("context-optimizer.json"),
        r#"{"agents":{"claude-code":{"cliTools":false}}}"#,
    )?;
    let output = hook(&root, b"malformed input", false)?;
    let context = output["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("默认兜底主力"));
    assert!(!context.contains("用户保存的角色"));
    assert!(!context.contains("mt ctx resume"));
    fs::write(root.join("context-optimizer.json"), "{broken")?;
    let output = hook(&root, b"{}", false)?;
    assert!(
        !output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("mt ctx resume")
    );
    Ok(())
}

#[cfg(windows)]
#[test]
fn native_batch_entry_accepts_an_installed_agent_in_a_chinese_path() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let directory = temp.path().join("Agent 可执行文件");
    fs::create_dir(&directory)?;
    let cli = directory.join("codex.cmd");
    fs::write(&cli, b"@echo off\r\necho %~1\r\nexit /b 19\r\n")?;
    let result = Command::new(runtime_binary())
        .arg("--agent-batch")
        .arg(&cli)
        .arg("one argument")
        .env("PATH", temp.path())
        .output()?;
    assert_eq!(result.status.code(), Some(19));
    assert_eq!(String::from_utf8(result.stdout)?.trim(), "one argument");
    Ok(())
}

#[test]
fn antigravity_native_hook_forwards_a_tool_payload_without_external_commands() -> Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let endpoint = listener.local_addr()?;
    let token = "A".repeat(43);
    let expected_request = format!("POST /hooks/{token}?kind=pre-tool-use HTTP/1.1\r\n");
    let payload = br#"{"hookEventName":"PreToolUse","toolCall":{"name":"view_file"}}"#;
    let decision = br#"{"decision":"allow"}"#;
    let worker = thread::spawn(move || -> Result<()> {
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut reader = BufReader::new(&mut socket);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            anyhow::ensure!(reader.read_line(&mut line)? > 0, "missing headers");
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line);
        }
        assert!(headers.starts_with(&expected_request));
        assert!(headers.contains(&format!("Content-Length: {}\r\n", payload.len())));
        let mut body = vec![0; payload.len()];
        reader.read_exact(&mut body)?;
        assert_eq!(body, payload);
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            decision.len()
        )?;
        socket.write_all(decision)?;
        Ok(())
    });
    let root = tempfile::tempdir()?;
    let mut child = Command::new(runtime_binary())
        .args(["--hook", "agy", "pre-tool-use"])
        .env("PATH", root.path())
        .env(
            "MATOU_PROVIDER_HOOK_URL",
            format!("http://{endpoint}/hooks/{token}"),
        )
        .env("MATOU_PROVIDER_HOOK_TOKEN", token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(payload)?;
    let result = child.wait_with_output()?;
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(result.stdout, decision);
    worker.join().unwrap()?;
    Ok(())
}

#[cfg(windows)]
#[test]
fn antigravity_context_command_runs_through_windows_shell_with_spaced_paths() -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::time::Instant;

    use matou_native_core::{
        context_optimizer_config::{AgentOptimizerSettings, ContextOptimizerConfig},
        pty::LaunchProfile,
    };
    use matou_native_runtime::context_guard::interceptor::HookInterceptor;

    let directory = tempfile::tempdir()?;
    let data_root = directory.path().join("验收 数据");
    let bin = directory.path().join("工具 路径");
    fs::create_dir_all(&data_root)?;
    fs::create_dir_all(&bin)?;
    fs::write(
        bin.join("npm.cmd"),
        b"@echo off\r\necho %*\r\nexit /b 17\r\n",
    )?;
    ContextOptimizerConfig::for_data_root(&data_root).set_agent_settings(
        "antigravity",
        AgentOptimizerSettings {
            noise_filter: true,
            ..Default::default()
        },
    )?;
    let evaluation = HookInterceptor::new().evaluate_hook(
        &data_root,
        LaunchProfile::Antigravity,
        "会话 标识",
        &json!({"hookEventName":"PreToolUse", "toolCall":{"name":"run_command", "args":{
            "CommandLine":"npm test -- --runInBand", "WaitMsBeforeAsync":5000
        }}}),
        Instant::now() + Duration::from_secs(2),
    );
    let reply: Value = serde_json::from_str(&evaluation.response_json)?;
    let test_binary = std::env::current_exe()?;
    let prefix = format!(r#""{}" --mt "#, test_binary.display());
    let wrapped = reply["overwrite"]["CommandLine"]
        .as_str()
        .and_then(|line| line.strip_prefix(&prefix))
        .expect("hook must wrap the command using the native runtime entry");
    // The integration test owns the process calling evaluate_hook. Replace its
    // executable prefix with the actual Runtime while keeping the emitted args.
    let runtime = runtime_binary();
    let output = Command::new("cmd.exe")
        .args(["/d", "/s", "/c"])
        .raw_arg(format!(r#"""{}" --mt {wrapped}""#, runtime.display()))
        .current_dir(directory.path())
        .env("PATH", bin)
        .output()?;
    assert_eq!(output.status.code(), Some(17), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout)?.trim(),
        "test -- --runInBand"
    );
    Ok(())
}
