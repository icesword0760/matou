//! The native client for local control discovery, terminal reads, and navigation.

use serde::Serialize;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
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
                std::env::var("MATOU_CONTROL_TOKEN")
                    .ok()
                    .filter(|token| !token.is_empty())
                    .or_else(|| std::env::var("DSH_MATOU_CONTROL_TOKEN").ok()),
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
    let json_output = json_output_requested(command, &args[1..]);
    let (endpoint, token) = identity();
    let result = request(command, &args[1..], endpoint, token);
    match result {
        Ok(result) => {
            let partial = result.as_ref().is_some_and(|value| {
                value["kind"] == "fork-batch"
                    && value["failed"].as_u64().is_some_and(|count| count > 0)
            });
            let output = if json_output {
                result
                    .as_ref()
                    .map(pretty_json)
                    .unwrap_or_else(|| "undefined".into())
            } else {
                let value = result.as_ref().unwrap_or(&Value::Null);
                match command.as_str() {
                    "identify" => format_identity(value),
                    "list" => format_list(value),
                    "commands" => format_commands(value),
                    "send" if args.iter().any(|arg| arg == "--enter") => "已发送文本并回车".into(),
                    "send" => "已发送文本".into(),
                    "key" => format!(
                        "已发送按键 {}",
                        args.get(2)
                            .and_then(|key| normalize_key(key).ok())
                            .unwrap_or_default()
                    ),
                    "create" | "fork" | "remove" | "close" => format_action(value),
                    _ => value["text"].as_str().unwrap_or_default().into(),
                }
            };
            if writeln!(stdout, "{output}").is_ok() {
                if partial { 6 } else { 0 }
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

fn request(
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
    if matches!(command, "read" | "history" | "commands") {
        let (target, method, mut params) = terminal_request(command, args)?;
        let client = Client::new(Path::new(&endpoint), &token)?;
        params["target"] = client.terminal_selector(target)?;
        return client.request(method, params);
    }
    if matches!(command, "send" | "key") {
        let target = positional(args.first(), &format!("{command} 需要目标"))?;
        let client = Client::new(Path::new(&endpoint), &token)?;
        let target = client.terminal_selector(target)?;
        if command == "key" {
            let raw = args.get(1).ok_or_else(|| usage("key 需要按键名称"))?;
            return client.request(
                "terminal.send-key",
                json!({"target": target, "key": normalize_key(raw)?}),
            );
        }
        let text = args[1..]
            .iter()
            .filter(|arg| !matches!(arg.as_str(), "--enter" | "--json"))
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        let text = text.trim();
        if text.is_empty() {
            return Err(usage("send 需要发送文本"));
        }
        return client.request(
            "terminal.send-text",
            json!({
                "target": target, "text": text, "submit": args.iter().any(|arg| arg == "--enter"),
            }),
        );
    }
    if matches!(command, "create" | "fork" | "remove" | "close") {
        let client = Client::new(Path::new(&endpoint), &token)?;
        let (method, params) = structure_request(&client, command, args)?;
        return client.request(method, params);
    }
    if matches!(command, "focus" | "switch") {
        let (method, entity, target) = navigation_request(command, args)?;
        let client = Client::new(Path::new(&endpoint), &token)?;
        let selector = if target == "current" {
            json!({"kind": "current", "entity": entity})
        } else {
            client.selector(target, true)?
        };
        let result = client.request(method, json!({"target": selector}))?;
        if wants_json(args) {
            return Ok(result);
        }
        return Ok(Some(
            json!({"text": client.format_navigation(result.as_ref())}),
        ));
    }
    let (method, params) = discovery_request(command, args)?;
    Client::new(Path::new(&endpoint), &token)?.request(method, params)
}

fn wants_json(args: &[String]) -> bool {
    args.iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--json")
}

fn usage(message: impl Into<String>) -> CliError {
    CliError::new("INVALID_REQUEST", message)
}

fn json_output_requested(command: &str, args: &[String]) -> bool {
    let fields: &[&str] = match command {
        "create" => &[
            "--path",
            "--title",
            "--submission-key",
            "--workspace",
            "--task",
            "--canvas",
            "--profile",
            "--cwd",
            "--permission-mode",
        ],
        "fork" => &[
            "--items-json",
            "--batch-key",
            "--retry-item-key",
            "--retry-item-keys-json",
            "--retry-items-json",
            "--title",
            "--environment-json",
            "--prompt",
            "--submission-key",
        ],
        "remove" => &["--scope"],
        "read" | "history" => &["--lines", "--bytes"],
        "commands" => &["--limit"],
        _ => &[],
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg == "--json" {
            return true;
        }
        if fields.contains(&arg.as_str()) {
            args.next();
        }
    }
    false
}

fn positional<'a>(arg: Option<&'a String>, message: &str) -> Result<&'a str, CliError> {
    arg.filter(|arg| !arg.is_empty() && !arg.starts_with("--"))
        .map(String::as_str)
        .ok_or_else(|| usage(message))
}

fn normalize_key(raw: &str) -> Result<&'static str, CliError> {
    Ok(match raw.trim().to_ascii_lowercase().as_str() {
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "escape" | "esc" => "Escape",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "up" | "arrowup" => "ArrowUp",
        "down" | "arrowdown" => "ArrowDown",
        "left" | "arrowleft" => "ArrowLeft",
        "right" | "arrowright" => "ArrowRight",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "ctrl+c" => "CtrlC",
        "ctrl+d" => "CtrlD",
        "ctrl+l" => "CtrlL",
        "ctrl+u" => "CtrlU",
        "ctrl+z" => "CtrlZ",
        _ => return Err(usage(format!("不支持的按键：{raw}"))),
    })
}

struct Options<'a> {
    values: std::collections::HashMap<&'a str, &'a str>,
    flags: std::collections::HashSet<&'a str>,
    retry_keys: Vec<&'a str>,
}

impl<'a> Options<'a> {
    fn parse(
        args: &'a [String],
        values: &[&str],
        booleans: &[&str],
        repeat_retry: bool,
    ) -> Result<Self, CliError> {
        let mut options = Self {
            values: Default::default(),
            flags: Default::default(),
            retry_keys: Vec::new(),
        };
        let mut args = args.iter().map(String::as_str);
        while let Some(flag) = args.next() {
            if flag == "--" {
                if let Some(extra) = args.next() {
                    return Err(usage(format!("-- 之后存在多余参数：{extra}")));
                }
                break;
            }
            if flag == "--json" || booleans.contains(&flag) {
                if !options.flags.insert(flag) {
                    return Err(usage(format!("{flag} 不得重复")));
                }
                continue;
            }
            if values.contains(&flag) || (repeat_retry && flag == "--retry-item-key") {
                let value = args.next().ok_or_else(|| usage(format!("{flag} 需要值")))?;
                if flag == "--retry-item-key" {
                    options.retry_keys.push(value);
                } else {
                    let key = if flag == "--retry-items-json" {
                        "--retry-item-keys-json"
                    } else {
                        flag
                    };
                    if options.values.insert(key, value).is_some() {
                        return Err(usage(format!("{flag} 不得重复")));
                    }
                }
                continue;
            }
            return Err(usage(if flag.starts_with("--") {
                format!("未知选项：{flag}")
            } else {
                format!("多余参数：{flag}")
            }));
        }
        Ok(options)
    }

    fn require(&self, flag: &str) -> Result<&'a str, CliError> {
        self.values
            .get(flag)
            .copied()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| usage(format!("缺少 {flag}")))
    }

    fn optional(&self, params: &mut Value, fields: &[(&str, &str)], flag: &str, field: &str) {
        for (option, key) in fields {
            if let Some(value) = self.values.get(option) {
                params[key] = json!(value);
            }
        }
        if self.flags.contains(flag) {
            params[field] = json!(true);
        }
    }

    fn key(&self, flag: &str) -> Value {
        self.values
            .get(flag)
            .map_or_else(|| json!(uuid::Uuid::new_v4().to_string()), |key| json!(key))
    }
}

fn structure_request(
    client: &Client<'_>,
    command: &str,
    args: &[String],
) -> Result<(&'static str, Value), CliError> {
    let operation = args.first().map(String::as_str).unwrap_or_default();
    if command == "create" {
        let (method, target_flag, field, entity) = match operation {
            "workspace" => ("structure.create.workspace", "--path", "path", None),
            "task" => (
                "structure.create.task",
                "--workspace",
                "workspace",
                Some("workspace"),
            ),
            "canvas" => ("structure.create.canvas", "--task", "task", Some("task")),
            "session" => (
                "structure.create.session",
                "--canvas",
                "canvas",
                Some("canvas"),
            ),
            _ => return Err(usage("create 需要 workspace、task、canvas 或 session")),
        };
        let mut flags = vec![target_flag, "--title", "--submission-key"];
        if operation == "session" {
            flags.extend(["--profile", "--cwd", "--permission-mode"]);
        }
        let options = Options::parse(&args[1..], &flags, &["--enter"], false)?;
        let target = options.require(target_flag)?;
        let mut params = json!({"submissionKey": options.key("--submission-key")});
        if operation == "session" {
            let profile = options.values.get("--profile").copied().unwrap_or("shell");
            if !["shell", "claude-code", "codex", "antigravity", "dsh"].contains(&profile) {
                return Err(usage(
                    "--profile 需要 shell、claude-code、codex、antigravity、dsh",
                ));
            }
            if options.values.get("--permission-mode").is_some_and(|mode| {
                !["default", "acceptEdits", "plan", "bypassPermissions"].contains(mode)
            }) {
                return Err(usage(
                    "--permission-mode 需要 default、acceptEdits、plan 或 bypassPermissions",
                ));
            }
            params["profile"] = json!(profile);
        }
        params[field] = match entity {
            Some(entity) => client.action_selector(target, Some(entity))?,
            None => json!(target),
        };
        options.optional(
            &mut params,
            &[
                ("--title", "title"),
                ("--cwd", "cwd"),
                ("--permission-mode", "permissionMode"),
            ],
            "--enter",
            "enter",
        );
        return Ok((method, params));
    }
    if command == "fork" {
        let method = match operation {
            "child" => "structure.fork.child",
            "sibling" => "structure.fork.sibling",
            "children" => "structure.fork.children",
            _ => return Err(usage("fork 需要 child、sibling 或 children")),
        };
        let source = positional(args.get(1), &format!("fork {operation} 需要源会话"))?;
        if operation == "children" {
            let options = Options::parse(
                &args[2..],
                &[
                    "--items-json",
                    "--batch-key",
                    "--retry-item-keys-json",
                    "--retry-items-json",
                ],
                &[],
                true,
            )?;
            let input = options.require("--items-json")?;
            let buffer;
            let input = if input == "-" {
                let mut bytes = Vec::new();
                io::stdin()
                    .lock()
                    .take(1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| usage("无法读取标准输入"))?;
                if bytes.len() > 1024 * 1024 {
                    return Err(usage("--items-json 标准输入不能超过 1 MiB"));
                }
                buffer = String::from_utf8(bytes).map_err(|_| usage("--items-json 需要 UTF-8"))?;
                &buffer
            } else {
                input
            };
            let items = parse_json(input, "--items-json", Value::is_array)?;
            let mut retry_keys = options
                .retry_keys
                .iter()
                .map(|key| json!(key))
                .collect::<Vec<_>>();
            let retry_json = options.values.get("--retry-item-keys-json");
            if let Some(array) = retry_json {
                let array = parse_json(array, "--retry-item-keys-json", |value| {
                    value
                        .as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string))
                })?;
                retry_keys.extend(array.as_array().unwrap().iter().cloned());
            }
            let retry = !retry_keys.is_empty() || retry_json.is_some();
            if retry && !options.values.contains_key("--batch-key") {
                return Err(usage("重试失败项时必须显式提供原 --batch-key"));
            }
            let mut params = json!({"source": client.action_selector(source, None)?, "batchKey": options.key("--batch-key"), "items": items});
            if retry {
                params["retryItemKeys"] = json!(retry_keys);
            }
            return Ok((method, params));
        }
        let options = Options::parse(
            &args[2..],
            &[
                "--title",
                "--environment-json",
                "--prompt",
                "--submission-key",
            ],
            &["--start"],
            false,
        )?;
        let environment = parse_json(
            options.require("--environment-json")?,
            "--environment-json",
            Value::is_object,
        )?;
        let title = options.require("--title")?;
        let mut params = json!({
            "source": client.action_selector(source, None)?, "title": title, "environment": environment,
            "submissionKey": options.key("--submission-key"),
        });
        options.optional(&mut params, &[("--prompt", "prompt")], "--start", "start");
        return Ok((method, params));
    }
    let (method, preview, canvas) = match (command, operation) {
        ("remove", "preview") => ("structure.remove.preview", true, false),
        ("remove", "commit") => ("structure.remove.commit", false, false),
        ("close", "canvas-preview") => ("structure.canvas-close.preview", true, true),
        ("close", "canvas-commit") => ("structure.canvas-close.commit", false, true),
        ("remove", _) => return Err(usage("remove 需要 preview 或 commit")),
        _ => return Err(usage("close 需要 canvas-preview 或 canvas-commit")),
    };
    let missing = if !preview {
        "确认引用"
    } else if canvas {
        "画布目标"
    } else {
        "目标"
    };
    let argument = positional(args.get(1), &format!("{command} {operation} 需要{missing}"))?;
    let options = Options::parse(
        &args[2..],
        if preview && !canvas {
            &["--scope"]
        } else {
            &[]
        },
        &[],
        false,
    )?;
    if !preview {
        return Ok((method, json!({"confirmationRef": argument})));
    }
    let scope = if canvas {
        None
    } else {
        let scope = options.require("--scope")?;
        if !matches!(scope, "node" | "subtree") {
            return Err(usage("--scope 需要 node 或 subtree"));
        }
        Some(scope)
    };
    let mut params =
        json!({"target": client.action_selector(argument, canvas.then_some("canvas"))?});
    if let Some(scope) = scope {
        params["scope"] = json!(scope);
    }
    Ok((method, params))
}

fn parse_json(
    input: &str,
    flag: &str,
    shape: impl FnOnce(&Value) -> bool,
) -> Result<Value, CliError> {
    let value =
        serde_json::from_str(input).map_err(|_| usage(format!("{flag} 需要有效的 JSON")))?;
    if !shape(&value) {
        return Err(usage(format!("{flag} 的 JSON 格式无效")));
    }
    Ok(value)
}

fn navigation_request<'a>(
    command: &str,
    args: &'a [String],
) -> Result<(&'static str, &'static str, &'a str), CliError> {
    let (method, entity, args) = if command == "focus" {
        ("navigation.focus.session", "session", args)
    } else {
        match args.first().map(String::as_str) {
            Some("workspace") => ("navigation.switch.workspace", "workspace", &args[1..]),
            Some("task") => ("navigation.switch.task", "task", &args[1..]),
            Some("canvas") => ("navigation.switch.canvas", "canvas", &args[1..]),
            _ => {
                return Err(CliError::new(
                    "INVALID_REQUEST",
                    "switch 需要 workspace、task 或 canvas",
                ));
            }
        }
    };
    let target = args
        .first()
        .filter(|arg| !arg.is_empty() && !arg.starts_with("--"))
        .ok_or_else(|| {
            CliError::new(
                "INVALID_REQUEST",
                if command == "focus" {
                    "focus 需要会话目标".into()
                } else {
                    format!("switch {entity} 需要目标")
                },
            )
        })?;
    let mut flags = args[1..].iter();
    let mut json = false;
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--json" if json => {
                return Err(CliError::new("INVALID_REQUEST", "--json 不得重复"));
            }
            "--json" => json = true,
            "--" => {
                if let Some(extra) = flags.next() {
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
    Ok((method, entity, target))
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

fn terminal_request<'a>(
    command: &str,
    args: &'a [String],
) -> Result<(&'a str, &'static str, Value), CliError> {
    let Some(target) = args
        .first()
        .filter(|arg| !arg.is_empty() && !arg.starts_with("--"))
    else {
        return Err(CliError::new(
            "INVALID_REQUEST",
            format!("{command} 需要目标"),
        ));
    };
    let (method, mut params) = match command {
        "read" => (
            "terminal.read-current",
            json!({"maxLines": 200, "maxBytes": 65_536}),
        ),
        "history" => (
            "terminal.read-history",
            json!({"maxLines": 1000, "maxBytes": 65_536}),
        ),
        "commands" => ("terminal.read-commands", json!({"limit": 100})),
        _ => unreachable!("terminal read command"),
    };
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        let (field, max) = match arg.as_str() {
            "--json" => continue,
            "--lines" if command != "commands" => ("maxLines", 5000),
            "--bytes" if command != "commands" => ("maxBytes", 1024 * 1024),
            "--limit" if command == "commands" => ("limit", 1000),
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
        };
        let value = args
            .next()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| (1.0..=9_007_199_254_740_991.0).contains(value) && value.fract() == 0.0)
            .ok_or_else(|| CliError::new("INVALID_REQUEST", format!("{arg} 需要正整数")))?;
        if value > f64::from(max) {
            return Err(CliError::new(
                "INVALID_REQUEST",
                format!("{arg} 需要 1-{max} 的整数"),
            ));
        }
        params[field] = json!(value as u64);
    }
    Ok((target, method, params))
}

fn format_action(value: &Value) -> String {
    let path = |path: &Value| {
        let mut parts = Vec::new();
        for (key, name) in [
            ("window", "窗口"),
            ("workspace", "工作空间"),
            ("task", "事项"),
            ("canvas", "画布"),
            ("session", "会话"),
        ] {
            if let Some(title) = path[key]["title"].as_str() {
                let directory = path[key]["path"]
                    .as_str()
                    .map(|path| format!("（{path}）"))
                    .unwrap_or_default();
                parts.push(format!("{name} {title}{directory}"));
            }
        }
        parts.join(" / ")
    };
    let location = || {
        let path = path(&value["activePath"]);
        if path.is_empty() {
            "已更新".into()
        } else {
            path
        }
    };
    let fork = |item: &Value| {
        let state = match item["state"].as_str() {
            Some("created") => "已创建",
            Some("ready") => "已就绪",
            Some("started") => "已启动",
            Some("failed") => "失败",
            _ => "未知",
        };
        let branch = item["environment"]["branch"]
            .as_str()
            .unwrap_or("未命名分支");
        let environment = match item["environment"]["mode"].as_str() {
            Some("current") => "当前执行环境".into(),
            Some("new-worktree") => format!("新 Worktree（{branch}）"),
            Some("existing-worktree") => format!("现有 Worktree（{branch}）"),
            _ => "未知环境".into(),
        };
        format!("环境：{environment} | 状态：{state}")
    };
    let number = |value: &Value, key: &str| value[key].as_u64().unwrap_or(0);
    let preserves = "项目文件、Git 分支和 Worktree 保持不变。";
    match value["kind"].as_str() {
        Some("created") => {
            let entity = match value["entity"].as_str() {
                Some("workspace") => "工作空间",
                Some("task") => "事项",
                Some("canvas") => "画布",
                Some("session") => "会话",
                _ => "对象",
            };
            let created = path(&value["path"]);
            let focus = path(&value["focusedPath"]);
            let mut output = format!("已创建{entity}");
            if !created.is_empty() {
                output.push_str(&format!("：{created}"));
            }
            if !focus.is_empty() {
                output.push_str(&format!("\n当前焦点：{focus}"));
            }
            output
        }
        Some("forked") => {
            let target = path(&value["path"]);
            format!(
                "{} | {}",
                if target.is_empty() {
                    "新会话"
                } else {
                    &target
                },
                fork(value)
            )
        }
        Some("fork-batch") => {
            let mut lines = value["items"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, item)| {
                    format!(
                        "{}. {} | {}",
                        index + 1,
                        display(&item["title"]),
                        fork(item)
                    )
                })
                .collect::<Vec<_>>();
            lines.push(format!(
                "汇总：成功 {} 项，失败 {} 项。",
                number(value, "succeeded"),
                number(value, "failed")
            ));
            lines.join("\n")
        }
        Some(kind @ ("removal-preview" | "canvas-close-preview")) => {
            let operation = if kind == "canvas-close-preview" {
                "关闭画布"
            } else {
                "移除"
            };
            let impact = &value["impact"];
            let target = path(&impact["target"]);
            let target = if target.is_empty() {
                "已选定对象"
            } else {
                &target
            };
            let scope = if impact["scope"] == "subtree" {
                "当前节点及全部子节点"
            } else {
                "当前节点"
            };
            format!(
                "{operation}预览目标：{target}\n操作范围：{scope}\n影响：事项 {}，画布 {}，会话 {}，子节点 {}\n将结束：运行或等待中会话 {}，终端进程 {}\n{preserves}",
                number(impact, "tasks"),
                number(impact, "canvases"),
                number(impact, "sessions"),
                number(impact, "descendants"),
                number(impact, "liveRuns"),
                number(impact, "terminalProcesses")
            )
        }
        Some("removed") => format!(
            "已移除：事项 {}，画布 {}，会话 {}\n当前位置：{}\n{preserves}",
            number(value, "removedTasks"),
            number(value, "removedCanvases"),
            number(value, "removedSessions"),
            location()
        ),
        Some("canvas-closed") => format!(
            "已关闭画布，结束会话 {} 个。\n当前位置：{}\n{preserves}",
            number(value, "removedSessions"),
            location()
        ),
        _ => "操作已完成".into(),
    }
}

fn format_commands(value: &Value) -> String {
    let Some(items) = value.as_array() else {
        return String::new();
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let text = item.as_str().or_else(|| {
                item.get("command")
                    .filter(|value| !value.is_null())
                    .or_else(|| item.get("text").filter(|value| !value.is_null()))
                    .or_else(|| item.get("input"))
                    .and_then(Value::as_str)
            });
            let display = text.map_or_else(|| item.to_string(), str::to_owned);
            format!("{}. {display}", index + 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    format_target_path(target)
}

fn format_target_path(target: &Value) -> String {
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

fn response_budget(method: &str, params: &Value) -> Duration {
    match method {
        "structure.fork.child" | "structure.fork.sibling" if params["start"] == true => {
            Duration::from_secs(75)
        }
        "structure.fork.children" => {
            let Some(items) = params["items"].as_array().filter(|items| !items.is_empty()) else {
                return BUDGET;
            };
            let total = items.len().min(50);
            let starts = items
                .iter()
                .take(total)
                .filter(|item| item["start"] == true)
                .count();
            Duration::from_secs(15 + 60 * starts as u64 + if starts == total { 0 } else { 70 })
        }
        _ => BUDGET,
    }
}

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

        let response_budget = response_budget(method, &params);
        let request_id = uuid::Uuid::new_v4().to_string();
        let body = serde_json::to_vec(&json!({
            "version": VERSION,
            "requestId": request_id,
            "token": self.token,
            "method": method,
            "params": params,
            "deadlineAt": now_ms() + response_budget.as_millis() as i64,
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
        let response = timeout(response_budget, async {
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
        self.selector(target, false)
    }

    fn action_selector(&self, target: &str, current: Option<&str>) -> Result<Value, CliError> {
        if target == "current" {
            let entity = current.ok_or_else(|| usage("current 需要明确的层级"))?;
            return Ok(json!({"kind": "current", "entity": entity}));
        }
        self.selector(target, true)
    }

    fn selector(&self, target: &str, navigation: bool) -> Result<Value, CliError> {
        match target {
            "self" => return Ok(json!({ "kind": "self" })),
            "left" | "right" => {
                return self.positional_selector(
                    json!({"kind": "relative", "direction": target}),
                    navigation,
                );
            }
            "parent" => {
                return self.positional_selector(
                    json!({"kind": "relation", "relation": target}),
                    navigation,
                );
            }
            "child" if !navigation => return Ok(json!({ "kind": "relation", "relation": target })),
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
                    return self.positional_selector(
                        json!({ "kind": "relation", "relation": "child", "ordinal": ordinal }),
                        navigation,
                    );
                }
                return self
                    .positional_selector(json!({"kind": "sibling", "ordinal": ordinal}), true);
            }
        }
        if !navigation && uuid::Uuid::parse_str(target).is_ok() {
            return Ok(json!({ "sessionId": target }));
        }
        let (targets, revision) = listing(self.request("host.list", json!({ "scope": "all" }))?)?;
        if !navigation && !targets.iter().any(|entry| entry["ref"] == target) {
            return Err(CliError::new(
                "TARGET_NOT_FOUND",
                format!("目标 {target} 不在当前 Matou 拓扑中"),
            ));
        }
        Ok(json!({ "kind": "ref", "ref": target, "projectionRevision": revision }))
    }

    fn positional_selector(
        &self,
        mut selector: Value,
        needs_revision: bool,
    ) -> Result<Value, CliError> {
        if needs_revision {
            let (_, revision) =
                listing(self.request("host.list", json!({ "scope": "current-level" }))?)?;
            selector["projectionRevision"] = json!(revision);
        }
        Ok(selector)
    }

    fn format_navigation(&self, result: Option<&Value>) -> String {
        let Some(result) = result.filter(|value| value["kind"] == "navigated") else {
            return "操作已完成".into();
        };
        let path = &result["finalPath"];
        if let Ok((targets, _)) = self
            .request("host.list", json!({"scope": "all"}))
            .and_then(listing)
            && let Some(target) = targets.iter().find(|target| {
                target["window"]["id"] == path["targetWindowId"]
                    && target["workspaceId"] == path["workspaceId"]
                    && target["taskId"] == path["taskId"]
                    && target["canvas"]["id"] == path["sceneId"]
                    && target["sessionId"] == path["sessionId"]
            })
        {
            return format!("已切换到：{}", format_target_path(target));
        }
        "已切换到目标位置并完成聚焦".into()
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
