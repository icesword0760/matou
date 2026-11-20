//! The native discovery client. It only talks to the local control endpoint.

use serde::Serialize;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Write},
};

use std::{path::Path, time::Duration};

pub(crate) const MODE: &str = "--internal-mt";

pub(crate) fn install_shim(data_root: &Path) -> anyhow::Result<std::path::PathBuf> {
    use std::{
        fs::{self, DirBuilder, Permissions},
        os::unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        },
    };

    // The control server has already created and validated the private parent.
    let dir = data_root.join("control/bin");
    match DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(&dir)?;
    // SAFETY: geteuid only reads the current process credential.
    anyhow::ensure!(
        metadata.is_dir() && metadata.uid() == unsafe { libc::geteuid() },
        "mt bin directory is not an owned directory"
    );
    fs::set_permissions(&dir, Permissions::from_mode(0o700))?;
    let executable = std::env::current_exe()?;
    let mut shim = tempfile::NamedTempFile::new_in(&dir)?;
    shim.write_all(b"#!/bin/sh\nexec '")?;
    for part in executable
        .as_os_str()
        .as_bytes()
        .split_inclusive(|byte| *byte == b'\'')
    {
        if let Some(path) = part.strip_suffix(b"'") {
            shim.write_all(path)?;
            shim.write_all(b"'\\''")?;
        } else {
            shim.write_all(part)?;
        }
    }
    writeln!(shim, "' {MODE} \"$@\"")?;
    shim.as_file()
        .set_permissions(Permissions::from_mode(0o700))?;
    shim.persist(dir.join("mt"))?;
    Ok(dir)
}

const MISSING_IDENTITY: &str = "`mt` 仅在 Matou 托管终端中可用；当前进程缺少会话控制身份";
const NOT_CONNECTED: &str = "未连接到 Matou Host Control；请在 Matou 托管终端中重试";

/// Invoke the CLI from the runtime child before the stdio handshake.
pub fn run(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args: Vec<_> = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    run_with(
        &args,
        || {
            (
                std::env::var_os("MATOU_CONTROL_ENDPOINT"),
                std::env::var("MATOU_CONTROL_TOKEN").ok(),
            )
        },
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
}

pub(crate) fn run_with(
    args: &[String],
    identity: impl FnOnce() -> (Option<OsString>, Option<String>),
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> i32 {
    let Some(command) = args
        .first()
        .filter(|arg| !matches!(arg.as_str(), "--help" | "-h"))
    else {
        return if writeln!(stdout, "{HELP}").is_ok() {
            0
        } else {
            1
        };
    };
    let json_output = args[1..]
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--json");
    let (endpoint, token) = identity();
    let result = discover(command, &args[1..], endpoint, token);
    match result {
        Ok(result) => {
            let output = if json_output {
                result
                    .as_ref()
                    .map(pretty_json)
                    .unwrap_or_else(|| "undefined".into())
            } else if command == "identify" {
                format_identity(result.as_ref().unwrap_or(&Value::Null))
            } else {
                format_list(result.as_ref().unwrap_or(&Value::Null))
            };
            if writeln!(stdout, "{output}").is_ok() {
                0
            } else {
                1
            }
        }
        Err(error) => {
            let _ = writeln!(stderr, "{}", error.format(json_output));
            error.exit_code()
        }
    }
}

fn discover(
    command: &str,
    args: &[String],
    endpoint: Option<OsString>,
    token: Option<String>,
) -> Result<Option<Value>, CliError> {
    let (Some(endpoint), Some(token)) = (
        endpoint.filter(|value| !value.is_empty()),
        token.filter(|value| !value.is_empty()),
    ) else {
        return Err(CliError::new("CAPABILITY_DENIED", MISSING_IDENTITY));
    };
    let (method, params) = discovery_request(command, args)?;
    Client::new(Path::new(&endpoint), &token)?.request(method, params)
}

fn discovery_request(command: &str, args: &[String]) -> Result<(&'static str, Value), CliError> {
    if !matches!(command, "identify" | "list") {
        return Err(CliError::new(
            "INVALID_REQUEST",
            format!("未知命令：{command}"),
        ));
    }
    let mut all = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => {}
            "--all" if command == "list" => all = true,
            "--" => {
                if let Some(extra) = args.next() {
                    return Err(CliError::new(
                        "INVALID_REQUEST",
                        format!("-- 之后存在多余参数：{extra}"),
                    ));
                }
                break;
            }
            flag if flag.starts_with("--") => {
                return Err(CliError::new(
                    "INVALID_REQUEST",
                    format!("未知选项：{flag}"),
                ));
            }
            extra => {
                return Err(CliError::new(
                    "INVALID_REQUEST",
                    format!("多余参数：{extra}"),
                ));
            }
        }
    }
    if command == "identify" {
        Ok(("host.identify", json!({})))
    } else {
        Ok((
            "host.list",
            json!({ "scope": if all { "all" } else { "current-level" } }),
        ))
    }
}

#[derive(Debug, Serialize)]
pub struct CliError {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Candidates>,
}

#[derive(Debug, Serialize)]
struct Candidates {
    candidates: Vec<Candidate>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    human_path: String,
}

impl CliError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    fn from_response(response: &Value) -> Result<Self, Self> {
        let mut error = Self::new(
            response["code"].as_str().ok_or_else(invalid_response)?,
            response["message"].as_str().ok_or_else(invalid_response)?,
        );
        if error.code == "AMBIGUOUS_TARGET" {
            error.details = response["details"]["candidates"]
                .as_array()
                .filter(|items| !items.is_empty())
                .and_then(|items| {
                    items
                        .iter()
                        .map(|item| {
                            let path = item["humanPath"].as_str()?;
                            if path.trim().is_empty() || path.len() > 4096 {
                                return None;
                            }
                            Some(Candidate {
                                human_path: path.into(),
                            })
                        })
                        .collect::<Option<Vec<_>>>()
                })
                .map(|candidates| Candidates { candidates });
        }
        Ok(error)
    }

    pub(crate) fn exit_code(&self) -> i32 {
        match self.code.as_str() {
            "INVALID_REQUEST" => 2,
            "TARGET_NOT_FOUND" | "AMBIGUOUS_TARGET" | "STALE_PROJECTION" | "CONFLICT" => 3,
            "CAPABILITY_DENIED"
            | "TARGET_NOT_READY"
            | "RUNTIME_NOT_READY"
            | "UNSUPPORTED"
            | "CONFIRMATION_REQUIRED"
            | "CONFIRMATION_EXPIRED"
            | "CONFIRMATION_STALE"
            | "PATH_CONFLICT"
            | "BRANCH_CONFLICT"
            | "WORKTREE_CONFLICT"
            | "STORAGE_READ_ONLY" => 4,
            "TIMEOUT" | "CONNECTION_ERROR" | "NAVIGATION_TIMEOUT" => 5,
            "PARTIAL_SUCCESS" => 6,
            _ => 1,
        }
    }

    pub(crate) fn format(&self, json_output: bool) -> String {
        if json_output {
            return pretty_json(self);
        }
        let Some(details) = &self.details else {
            return self.message.clone();
        };
        match details.candidates.len() {
            0 | 1 => self.message.clone(),
            2..=5 => {
                let mut text = format!("{}\n候选项：", self.message);
                for (index, candidate) in details.candidates.iter().enumerate() {
                    use std::fmt::Write;
                    let _ = write!(text, "\n{}. {}", index + 1, candidate.human_path);
                }
                text
            }
            _ => format!(
                "{}\n候选项超过 5 个，请补充筛选条件，例如工作空间、事项、画布、会话标题或路径。",
                self.message
            ),
        }
    }
}

fn pretty_json(value: &impl Serialize) -> String {
    serde_json::to_string_pretty(value).expect("control JSON contains only serializable values")
}

fn display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "undefined".into(),
        _ => value.to_string(),
    }
}

fn format_identity(result: &Value) -> String {
    let target = &result["target"];
    if target.is_null() {
        return "已识别当前 Matou 会话".into();
    }
    format!(
        "窗口 {} / 工作空间 {} / 事项 {} / 画布 {} / 会话 {}（{}）",
        display(&target["window"]["ordinal"]),
        display(&target["workspace"]["name"]),
        display(&target["task"]["name"]),
        display(&target["canvas"]["name"]),
        display(&target["session"]["ordinal"]),
        display(&target["title"]),
    )
}

fn format_list(result: &Value) -> String {
    let targets = result["targets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if targets.is_empty() {
        return "当前范围内没有会话".into();
    }
    targets
        .iter()
        .map(|target| {
            let entry = format!(
                "{}. {} [{}]",
                display(&target["session"]["ordinal"]),
                display(&target["title"]),
                display(&target["profile"])
            );
            let cwd = target["cwd"].as_str().unwrap_or_default();
            let detached = target["session"]["detached"] == true;
            [
                entry.as_str(),
                cwd,
                if detached { "（独立窗口）" } else { "" },
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const BUDGET: Duration = Duration::from_secs(5);

pub struct Client<'a> {
    endpoint: &'a Path,
    token: &'a str,
    runtime: tokio::runtime::Runtime,
}

impl<'a> Client<'a> {
    pub fn new(endpoint: &'a Path, token: &'a str) -> Result<Self, CliError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| CliError::new("CONNECTION_ERROR", NOT_CONNECTED))?;
        Ok(Self {
            endpoint,
            token,
            runtime,
        })
    }

    pub fn request(&self, method: &str, params: Value) -> Result<Option<Value>, CliError> {
        self.runtime.block_on(self.exchange(method, params))
    }

    async fn exchange(&self, method: &str, params: Value) -> Result<Option<Value>, CliError> {
        use crate::host_control::{MAX_FRAME_BYTES, VERSION, now_ms};
        use tokio::{net::UnixStream, time::timeout};

        let request_id = uuid::Uuid::new_v4().to_string();
        let body = serde_json::to_vec(&json!({
            "version": VERSION,
            "requestId": request_id,
            "token": self.token,
            "method": method,
            "params": params,
            "deadlineAt": now_ms() + BUDGET.as_millis() as i64,
        }))
        .expect("control request contains only JSON values");
        if body.len() > MAX_FRAME_BYTES {
            return Err(CliError::new(
                "INVALID_REQUEST",
                "Host Control 请求超过大小限制",
            ));
        }
        let socket = timeout(BUDGET, UnixStream::connect(self.endpoint))
            .await
            .map_err(|_| {
                CliError::new(
                    "TIMEOUT",
                    "连接 Matou Host Control 超时；请确认 Matou 仍在运行后重试",
                )
            })?
            .map_err(|_| CliError::new("CONNECTION_ERROR", NOT_CONNECTED))?;
        let response = timeout(BUDGET, async {
            write_all(&socket, &(body.len() as u32).to_be_bytes()).await?;
            write_all(&socket, &body).await?;
            let mut length = [0; 4];
            read_exact(&socket, &mut length).await?;
            let length = u32::from_be_bytes(length) as usize;
            if length > MAX_FRAME_BYTES {
                return Err(CliError::new(
                    "INVALID_RESPONSE",
                    "Host Control 响应超过大小限制",
                ));
            }
            let mut bytes = vec![0; length];
            read_exact(&socket, &mut bytes).await?;
            let response = serde_json::from_str(&String::from_utf8_lossy(&bytes))
                .map_err(|_| invalid_response())?;
            parse_response(response, &request_id)
        })
        .await
        .map_err(|_| {
            CliError::new(
                "TIMEOUT",
                "等待 Matou Host Control 响应超时；请确认 Matou 仍在运行后重试",
            )
        })?;
        response
    }

    /// Encode a terminal target for a later action, obtaining its projection
    /// revision when the wire selector requires one.
    pub fn terminal_selector(&self, target: &str) -> Result<Value, CliError> {
        match target {
            "self" => return Ok(json!({ "kind": "self" })),
            "left" | "right" => return Ok(json!({ "kind": "relative", "direction": target })),
            "parent" | "child" => return Ok(json!({ "kind": "relation", "relation": target })),
            _ => {}
        }
        for kind in ["child", "sibling"] {
            if let Some(suffix) = target
                .strip_prefix(kind)
                .and_then(|rest| rest.strip_prefix(':'))
                && !suffix.is_empty()
                && suffix.bytes().all(|byte| byte.is_ascii_digit())
            {
                let ordinal = suffix
                    .parse::<u64>()
                    .ok()
                    .filter(|value| (1..=9_007_199_254_740_991).contains(value))
                    .ok_or_else(|| {
                        CliError::new("INVALID_REQUEST", format!("{kind} ordinal 需要正整数"))
                    })?;
                if kind == "child" {
                    return Ok(
                        json!({ "kind": "relation", "relation": "child", "ordinal": ordinal }),
                    );
                }
                let (_, revision) =
                    listing(self.request("host.list", json!({ "scope": "current-level" }))?)?;
                return Ok(
                    json!({ "kind": "sibling", "ordinal": ordinal, "projectionRevision": revision }),
                );
            }
        }
        if uuid::Uuid::parse_str(target).is_ok() {
            return Ok(json!({ "sessionId": target }));
        }
        let (targets, revision) = listing(self.request("host.list", json!({ "scope": "all" }))?)?;
        if !targets.iter().any(|entry| entry["ref"] == target) {
            return Err(CliError::new(
                "TARGET_NOT_FOUND",
                format!("目标 {target} 不在当前 Matou 拓扑中"),
            ));
        }
        Ok(json!({ "kind": "ref", "ref": target, "projectionRevision": revision }))
    }
}

fn listing(value: Option<Value>) -> Result<(Vec<Value>, String), CliError> {
    let Some(Value::Object(mut value)) = value else {
        return Err(CliError::new(
            "INVALID_RESPONSE",
            "Host Control 列表响应格式错误",
        ));
    };
    let (Some(Value::Array(targets)), Some(Value::String(revision))) =
        (value.remove("targets"), value.remove("projectionRevision"))
    else {
        return Err(CliError::new(
            "INVALID_RESPONSE",
            "Host Control 列表响应缺少投影版本",
        ));
    };
    Ok((targets, revision))
}

fn parse_response(mut value: Value, request_id: &str) -> Result<Option<Value>, CliError> {
    let ok = value["ok"].as_bool().ok_or_else(invalid_response)?;
    let actual = value["requestId"].as_str().ok_or_else(invalid_response)?;
    if value["version"].as_f64() != Some(f64::from(crate::host_control::VERSION)) {
        return Err(invalid_response());
    }
    let error = value
        .get("error")
        .map(CliError::from_response)
        .transpose()?;
    if actual != request_id {
        return Err(CliError::new(
            "INVALID_RESPONSE",
            "Host Control 返回了不匹配的请求标识",
        ));
    }
    if !ok {
        return Err(
            error.unwrap_or_else(|| CliError::new("INTERNAL_ERROR", "Host Control 请求失败"))
        );
    }
    Ok(value
        .as_object_mut()
        .and_then(|value| value.remove("result")))
}

fn invalid_response() -> CliError {
    CliError::new(
        "INVALID_RESPONSE",
        "Matou Host Control 返回了无效响应；请重试",
    )
}

fn connection_lost(_: io::Error) -> CliError {
    CliError::new(
        "CONNECTION_ERROR",
        "与 Matou Host Control 的连接已中断；请重试",
    )
}

async fn read_exact(
    socket: &tokio::net::UnixStream,
    mut remaining: &mut [u8],
) -> Result<(), CliError> {
    while !remaining.is_empty() {
        socket.readable().await.map_err(connection_lost)?;
        match socket.try_read(remaining) {
            Ok(0) => return Err(invalid_response()),
            Ok(n) => remaining = &mut remaining[n..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(connection_lost(error)),
        }
    }
    Ok(())
}

async fn write_all(socket: &tokio::net::UnixStream, mut remaining: &[u8]) -> Result<(), CliError> {
    while !remaining.is_empty() {
        socket.writable().await.map_err(connection_lost)?;
        match socket.try_write(remaining) {
            Ok(0) => return Err(connection_lost(io::ErrorKind::WriteZero.into())),
            Ok(n) => remaining = &remaining[n..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(connection_lost(error)),
        }
    }
    Ok(())
}

const HELP: &str = r#"Matou Host Control

Usage:
  mt identify [--json]
  mt list [--all] [--json]
  mt read TARGET [--lines N] [--bytes N] [--json]
  mt history TARGET [--lines N] [--bytes N] [--json]
  mt commands TARGET [--limit N] [--json]
  mt send TARGET TEXT [--enter] [--json]
  mt key TARGET KEY [--json]

  mt create workspace --path PATH [--title TITLE] [--submission-key KEY] [--enter] [--json]
  mt create task --workspace TARGET [--title TITLE] [--submission-key KEY] [--enter] [--json]
  mt create canvas --task TARGET [--title TITLE] [--submission-key KEY] [--enter] [--json]
  mt create session --canvas TARGET [--profile shell|claude-code|codex|antigravity|dsh] [--title TITLE] [--submission-key KEY] [--enter] [--json]

  mt fork child SOURCE --title TITLE --environment-json JSON [--prompt TEXT] [--start] [--submission-key KEY] [--json]
  mt fork sibling SOURCE --title TITLE --environment-json JSON [--prompt TEXT] [--start] [--submission-key KEY] [--json]
  mt fork children SOURCE --items-json JSON|- [--batch-key KEY] [--retry-item-key KEY] [--json]

  mt remove preview TARGET --scope node|subtree [--json]
  mt remove commit CONFIRMATION_REF [--json]
  mt close canvas-preview TARGET [--json]
  mt close canvas-commit CONFIRMATION_REF [--json]

  mt focus TARGET [--json]
  mt switch workspace TARGET [--json]
  mt switch task TARGET [--json]
  mt switch canvas TARGET [--json]

Targets: current (where supported), self, left, right, parent, child:N, sibling:N, or a ref from mt list --json

Batch JSON can be read from standard input with --items-json - (maximum 1 MiB).
Retry flags require the original explicit --batch-key.
Option values are consumed verbatim and may begin with '-'; standalone -- ends options with no trailing arguments."#;
