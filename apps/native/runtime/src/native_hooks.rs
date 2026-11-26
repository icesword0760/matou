//! Private, process-level entry points used by managed provider hooks.

use std::{
    ffi::OsString,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

use matou_native_core::{
    dispatch_roles::{DispatchRolesStore, format_roles_markdown},
    pty::LaunchProfile,
};
use serde_json::{Value, json};

pub(crate) const MODE: &str = "--hook";
pub(crate) const PROFILE_ENV: &str = "MATOU_PROVIDER_HOOK_PROFILE";
const MAX_BODY: u64 = 1024 * 1024;
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) fn antigravity_command(runtime: &Path, event: &str) -> anyhow::Result<String> {
    let path = runtime
        .to_str()
        .filter(|path| !path.chars().any(char::is_control))
        .ok_or_else(|| anyhow::anyhow!("Invalid native hook path"))?;
    #[cfg(windows)]
    let executable = format!(r#""{path}""#);
    #[cfg(not(windows))]
    let executable = format!("'{}'", path.replace('\'', "'\\''"));
    Ok(format!("{executable} {MODE} agy {event}"))
}

pub(crate) fn run(args: impl IntoIterator<Item = OsString>) -> i32 {
    let mut args = args.into_iter();
    let kind = args.next();
    let extra = args.next();
    if args.next().is_some() {
        return 2;
    }
    let output = match (kind.as_deref().and_then(|value| value.to_str()), extra) {
        (Some("claude-session-start"), None) => {
            let payload = read_payload();
            if let Some(payload) = payload.as_deref()
                && matches_hook_profile(LaunchProfile::ClaudeCode)
            {
                let _ = provider_callback(payload, None, false);
            }
            let data_root = std::env::var_os("MATOU_DATA_ROOT")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from);
            let is_worker =
                std::env::var_os("MATOU_FOREMAN_DEPTH").is_some_and(|depth| !depth.is_empty());
            claude_session_start(data_root.as_deref(), is_worker)
        }
        (Some("agy"), event) => {
            let event = event
                .as_deref()
                .and_then(|value| value.to_str())
                .unwrap_or("observe");
            if !matches!(
                event,
                "observe" | "session-start" | "pre-tool-use" | "post-tool-use" | "statusline"
            ) {
                return 2;
            }
            antigravity_hook(event)
        }
        _ => return 2,
    };
    if let Some(output) = output {
        let _ = io::stdout().lock().write_all(&output);
    }
    0
}

fn claude_session_start(data_root: Option<&Path>, is_worker: bool) -> Option<Vec<u8>> {
    let mut context = "Matou host control is active because the matou-host-control plugin is loaded.\n\n\
For any request about another, adjacent, left, or right Matou card, session, or terminal, invoke `matou-host-control:mt-terminal` immediately and follow its `mt identify -> mt list -> mt read/history/commands` flow.\n\n\
Treat Matou's `mt` control plane as authoritative for these requests. Skip alternate terminal host environment discovery and other terminal-host probes in this Matou-managed session."
        .to_owned();
    if !is_worker {
        let roles = data_root
            .filter(|root| {
                small_config(
                    &root.join(matou_native_core::dispatch_roles::ROLES_FILENAME),
                    Value::is_array,
                )
            })
            .and_then(|root| DispatchRolesStore::for_data_root(root).snapshot().ok())
            .map(|snapshot| snapshot.config)
            .unwrap_or_default();
        context.push_str("\n\n在当前码头托管会话中派活请直接运行 `mt-foreman`。配置变更后可运行 `mt-foreman roles` 查看最新配置。\n\n");
        context.push_str(&format_roles_markdown(&roles));
    }
    if let Some(instructions) = data_root
        .filter(|root| {
            small_config(&root.join("context-optimizer.json"), |agents| {
                agents.is_object()
                    && agents.get("claude-code").is_none_or(|claude| {
                        claude.is_object()
                            && ["enabled", "cliTools"]
                                .iter()
                                .all(|key| claude.get(key).is_none_or(Value::is_boolean))
                    })
            })
        })
        .and_then(|root| crate::context_guard::managed_cli_instructions(root, "claude-code"))
    {
        context.push_str("\n\n");
        context.push_str(instructions);
    }
    serde_json::to_vec(&json!({"hookSpecificOutput": {
        "hookEventName": "SessionStart",
        "additionalContext": context
    }}))
    .ok()
}

fn small_config(path: &Path, valid_agents: impl FnOnce(&Value) -> bool) -> bool {
    const MAX_CONFIG: u64 = 256 * 1024;
    match std::fs::metadata(path) {
        Err(error) => error.kind() == io::ErrorKind::NotFound,
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_CONFIG => {
            std::fs::read(path).ok().is_some_and(|bytes| {
                bytes.len() as u64 <= MAX_CONFIG
                    && serde_json::from_slice::<Value>(&bytes).is_ok_and(|value| {
                        value.is_object()
                            && value.get("version").is_none_or(|version| version == 1)
                            && value.get("agents").is_none_or(valid_agents)
                    })
            })
        }
        _ => false,
    }
}

fn read_payload() -> Option<Vec<u8>> {
    let mut payload = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_BODY + 1)
        .read_to_end(&mut payload)
        .ok()?;
    (payload.len() as u64 <= MAX_BODY && serde_json::from_slice::<Value>(&payload).is_ok())
        .then_some(payload)
}

fn antigravity_hook(event: &str) -> Option<Vec<u8>> {
    let payload = read_payload()?;
    if !matches_hook_profile(LaunchProfile::Antigravity) {
        return None;
    }
    let pre_tool = event == "pre-tool-use"
        || (event == "observe"
            && serde_json::from_slice::<Value>(&payload).ok()?["hookEventName"] == "PreToolUse");
    let kind = match event {
        "post-tool-use" | "statusline" => Some(event),
        _ if pre_tool => Some("pre-tool-use"),
        _ => None,
    };
    let decision = provider_callback(&payload, kind, pre_tool);
    if event == "session-start" {
        let path = std::env::var_os("MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE")?;
        let mut output = Vec::new();
        let file = std::fs::File::open(path).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        file.take(MAX_BODY + 1).read_to_end(&mut output).ok()?;
        return (output.len() as u64 <= MAX_BODY
            && serde_json::from_slice::<Value>(&output).is_ok())
        .then_some(output);
    }
    decision
}

fn matches_hook_profile(profile: LaunchProfile) -> bool {
    std::env::var_os(PROFILE_ENV).is_none_or(|owner| owner == profile.kind())
}

fn provider_callback(payload: &[u8], kind: Option<&str>, wait_for_result: bool) -> Option<Vec<u8>> {
    let url = std::env::var("MATOU_PROVIDER_HOOK_URL").ok()?;
    let token = std::env::var("MATOU_PROVIDER_HOOK_TOKEN").ok()?;
    let (port, path) = url.strip_prefix("http://127.0.0.1:")?.split_once('/')?;
    let port = port.parse::<u16>().ok()?;
    if token.len() != 43
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        || path.strip_prefix("hooks/")? != token
    {
        return None;
    }
    let query = kind.map_or_else(String::new, |kind| format!("?kind={kind}"));
    let endpoint = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut socket = TcpStream::connect_timeout(&endpoint, CALLBACK_TIMEOUT).ok()?;
    socket.set_read_timeout(Some(CALLBACK_TIMEOUT)).ok()?;
    socket.set_write_timeout(Some(CALLBACK_TIMEOUT)).ok()?;
    write!(socket, "POST /{path}{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len()).ok()?;
    socket.write_all(payload).ok()?;
    if !wait_for_result {
        let _ = socket.shutdown(std::net::Shutdown::Write);
        return None;
    }
    let mut response = Vec::new();
    socket.take(MAX_BODY + 1).read_to_end(&mut response).ok()?;
    if response.len() as u64 > MAX_BODY || !response.starts_with(b"HTTP/1.1 200 ") {
        return None;
    }
    let start = response.windows(4).position(|bytes| bytes == b"\r\n\r\n")? + 4;
    Some(response.split_off(start))
}
