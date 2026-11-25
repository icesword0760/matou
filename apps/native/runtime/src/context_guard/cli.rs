//! Zero-MCP `mt ctx` CLI subcommand handler.
//!
//! Invoked directly by `mt_cli::run_with` when the first argument is `ctx`.
//! Runs in-process with zero network hops and enforces the 8.8 KB response cap.

use std::{
    env,
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{ChildStderr, Command, ExitStatus, Output, Stdio},
    sync::mpsc,
    thread,
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use matou_native_core::context_optimizer_config::{
    ContextOptimizerConfig, DEFAULT_MAX_LOG_BYTES, KNOWN_AGENT_IDS,
};

use crate::context_guard::{
    filter::{
        HARD_CAP_BYTES, strip_ansi_and_progress, summarize_tool_output, trim_terminal_output,
    },
    outline::{batch_search, extract_file_outline},
    redact_secrets,
    store::{ContextGuardStore, record_saved_stats},
};

const EXEC_CAP_BYTES: usize = 7_900;
const DEFAULT_EXEC_AGENT_ID: &str = "antigravity";
const CLI_TEXT_CAP_BYTES: usize = HARD_CAP_BYTES - 1; // The dispatcher adds a newline.
const EXEC_USAGE: &str = "Usage: mt ctx exec [--session <id>] [--data-root <path>] [--max-bytes <n>] [--agent <id>] -- <command...>";

const CTX_HELP: &str = "\
mt ctx — Matou built-in zero-MCP context helpers (all outputs <= 8.8 KB)

Usage:
  mt ctx outline <file>
  mt ctx batch-search <query1> [query2 ...] [--dir <path>]
  mt ctx index <file> [--session <id>] [--data-root <path>]
  mt ctx log <id> [--offset <bytes>] [--data-root <path>]
  mt ctx search <query> [--source <id>] [--limit <n>] [--session <id>] [--data-root <path>]
  mt ctx resume [--session <id>] [--data-root <path>]
  mt ctx exec [--session <id>] [--data-root <path>] [--max-bytes <n>] [--agent <id>] -- <command...>
";

pub fn run_ctx_command(args: &[String], default_data_root: Option<&Path>) -> Result<String> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Ok(CTX_HELP.to_owned());
    };
    if matches!(sub, "--help" | "-h" | "help") {
        return Ok(CTX_HELP.to_owned());
    }
    let rest = &args[1..];
    let raw = match sub {
        "outline" => cmd_outline(rest)?,
        "batch-search" => cmd_batch_search(rest)?,
        "index" => cmd_index(rest, default_data_root)?,
        "log" => cmd_log(rest, default_data_root)?,
        "search" => cmd_search(rest, default_data_root)?,
        "resume" => cmd_resume(rest, default_data_root)?,
        other => bail!("Unknown `mt ctx` subcommand: {other}\n\n{CTX_HELP}"),
    };
    Ok(cap_output(&redact_secrets(&raw)))
}

pub(crate) fn run_ctx_exec(
    args: &[OsString],
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<i32> {
    let separator = args
        .iter()
        .position(|arg| arg == "--")
        .context(EXEC_USAGE)?;
    let mut options = args[..separator].chunks_exact(2);
    anyhow::ensure!(options.remainder().is_empty(), "{EXEC_USAGE}");
    let command_args = &args[separator + 1..];
    anyhow::ensure!(
        command_args.first().is_some_and(|arg| !arg.is_empty()),
        "{EXEC_USAGE}"
    );

    let (mut session, mut root, mut max_bytes) = (None, None, None);
    let mut agent_id = DEFAULT_EXEC_AGENT_ID;
    for pair in &mut options {
        anyhow::ensure!(
            !pair[1].is_empty() && !pair[1].as_encoded_bytes().starts_with(b"--"),
            "{EXEC_USAGE}"
        );
        match pair[0].to_str() {
            Some("--session") => session = Some(pair[1].to_str().context("invalid --session id")?),
            Some("--data-root") => root = Some(Path::new(&pair[1])),
            Some("--agent") => {
                agent_id = pair[1]
                    .to_str()
                    .filter(|id| KNOWN_AGENT_IDS.contains(id))
                    .context("unknown --agent id")?;
            }
            Some("--max-bytes") => {
                max_bytes = Some(
                    pair[1]
                        .to_str()
                        .context("invalid --max-bytes")?
                        .parse::<usize>()
                        .ok()
                        .filter(|bytes| *bytes > 0)
                        .context("invalid --max-bytes")?,
                );
            }
            _ => bail!("{EXEC_USAGE}"),
        }
    }

    let mut command = if let [script] = command_args {
        shell_command(script)
    } else {
        let mut direct = Command::new(&command_args[0]);
        direct.args(&command_args[1..]);
        direct
    };
    let output = capture_command(&mut command)?;
    let code = exit_code(output.status);
    let raw_bytes = output.stdout.len() + output.stderr.len();
    let data_root = resolve_data_root(root, None).ok();
    let threshold = max_bytes.unwrap_or_else(|| {
        data_root
            .as_ref()
            .and_then(|root| {
                let config = ContextOptimizerConfig::for_data_root(root);
                if !config
                    .path()
                    .metadata()
                    .is_ok_and(|metadata| metadata.is_file())
                {
                    return None;
                }
                config
                    .effective_for(agent_id)
                    .ok()
                    .map(|settings| settings.max_log_bytes.max(HARD_CAP_BYTES))
            })
            .unwrap_or(DEFAULT_MAX_LOG_BYTES)
    });
    if raw_bytes <= threshold {
        let _ = stdout.write_all(&output.stdout);
        let _ = stderr.write_all(&output.stderr);
        return Ok(code);
    }

    let mut full = String::from_utf8_lossy(&output.stdout).into_owned();
    if !full.is_empty() && !full.ends_with('\n') && !output.stderr.is_empty() {
        full.push('\n');
    }
    full.push_str(&String::from_utf8_lossy(&output.stderr));
    let full = redact_secrets(&full);
    let command_text = redact_secrets(
        &command_args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" "),
    );
    let session = resolve_session_id(session);
    let stored = data_root.as_ref().and_then(|root| {
        // A locked database immediately falls back to local output.
        ContextGuardStore::open_for_hook(root)
            .ok()?
            .save_log(
                &session,
                &format!("mt ctx exec (exit {code})"),
                &command_text,
                &full,
            )
            .ok()
    });
    let budget = EXEC_CAP_BYTES;
    let summary = stored.as_ref().and_then(|log| {
        let cleaned = redact_secrets(&strip_ansi_and_progress(&full));
        summarize_tool_output(&cleaned, &command_text, budget, &log.id, false)
    });
    let text = summary.unwrap_or_else(|| {
        let fallback = trim_terminal_output(&full, &command_text, HARD_CAP_BYTES, None).text;
        let note = stored.as_ref().map_or_else(
            || "\n\n[matou context-guard] Full log could not be saved.".to_owned(),
            |log| {
                format!(
                    "\n\n[matou context-guard] Full log saved as `{}`. Read with `mt ctx log {}`.",
                    log.id, log.id
                )
            },
        );
        let end = fallback.floor_char_boundary(budget - note.len());
        format!("{}{note}", &fallback[..end])
    });
    if stdout.write_all(text.as_bytes()).is_ok()
        && let Some(root) = data_root.as_ref().filter(|_| stored.is_some())
        && raw_bytes > text.len()
    {
        let _ = record_saved_stats(
            root,
            agent_id,
            (raw_bytes - text.len()) as u64,
            SystemTime::now(),
        );
    }
    Ok(code)
}

#[cfg(not(windows))]
fn shell_command(script: &OsString) -> Command {
    let mut shell = Command::new("/bin/sh");
    shell.arg("-c").arg(script);
    shell
}

#[cfg(windows)]
fn shell_command(script: &OsString) -> Command {
    use std::os::windows::process::CommandExt;

    let mut shell = Command::new("cmd.exe");
    let mut quoted = OsString::from("\"");
    quoted.push(script);
    quoted.push("\"");
    shell.args(["/d", "/s", "/c"]).raw_arg(quoted);
    shell
}

fn capture_command(command: &mut Command) -> Result<Output> {
    thread::scope(|scope| {
        // Prepare the second pipe reader before launching. Once launched, capture
        // errors only leave partial bytes; the exit status comes from wait().
        let (send, receive) = mpsc::channel::<ChildStderr>();
        let stderr = thread::Builder::new().spawn_scoped(scope, move || {
            let mut bytes = Vec::new();
            if let Ok(mut pipe) = receive.recv() {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })?;
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not start mt ctx exec command")?;
        if let Some(pipe) = child.stderr.take() {
            let _ = send.send(pipe);
        }
        drop(send);
        let mut stdout = Vec::new();
        if let Some(mut pipe) = child.stdout.take() {
            let _ = pipe.read_to_end(&mut stdout);
        }
        let status = child
            .wait()
            .context("could not wait for mt ctx exec command")?;
        Ok(Output {
            status,
            stdout,
            stderr: stderr.join().unwrap_or_default(),
        })
    })
}

fn exit_code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(1)
}

fn cmd_outline(args: &[String]) -> Result<String> {
    let path = args
        .first()
        .filter(|s| !s.starts_with('-'))
        .context("Usage: mt ctx outline <file>")?;
    extract_file_outline(Path::new(path))
}

fn cmd_batch_search(args: &[String]) -> Result<String> {
    let mut dir: Option<PathBuf> = None;
    let mut queries = Vec::new();
    let mut idx = 0;
    while idx < args.len() {
        if args[idx] == "--dir" {
            let val = args.get(idx + 1).context("--dir requires a path")?;
            dir = Some(PathBuf::from(val));
            idx += 2;
        } else {
            queries.push(args[idx].clone());
            idx += 1;
        }
    }
    let root = match dir {
        Some(d) => d,
        None => std::env::current_dir().context("resolve current directory")?,
    };
    batch_search(&root, &queries)
}

fn cmd_index(args: &[String], default_data_root: Option<&Path>) -> Result<String> {
    let mut file_path: Option<PathBuf> = None;
    let mut session = None;
    let mut explicit_root: Option<PathBuf> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--session" => {
                session = Some(
                    args.get(idx + 1)
                        .context("--session requires an id")?
                        .as_str(),
                );
                idx += 2;
            }
            "--data-root" => {
                explicit_root = Some(PathBuf::from(
                    args.get(idx + 1).context("--data-root requires a path")?,
                ));
                idx += 2;
            }
            val if file_path.is_none() => {
                file_path = Some(PathBuf::from(val));
                idx += 1;
            }
            other => bail!("Unexpected argument for `mt ctx index`: {other}"),
        }
    }
    let path = file_path.context("Usage: mt ctx index <file>")?;
    let content = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let redacted = redact_secrets(&content);
    let data_root = resolve_data_root(explicit_root.as_deref(), default_data_root)?;
    let store = ContextGuardStore::open(&data_root)?;
    let saved = store.save_log(
        &resolve_session_id(session),
        "mt-ctx-index",
        &redact_secrets(&path.display().to_string()),
        &redacted,
    )?;
    Ok(format!(
        "Indexed `{}` ({} bytes) as source `{}`.\nQuery matching passages with: mt ctx search <query> --source {}",
        path.display(),
        saved.raw_bytes,
        saved.id,
        saved.id
    ))
}

fn cmd_search(args: &[String], default_data_root: Option<&Path>) -> Result<String> {
    let mut query_parts = Vec::new();
    let mut source_id: Option<String> = None;
    let mut session = None;
    let mut limit: usize = 5;
    let mut explicit_root: Option<PathBuf> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--session" => {
                session = Some(
                    args.get(idx + 1)
                        .context("--session requires an id")?
                        .as_str(),
                );
                idx += 2;
            }
            "--source" => {
                source_id = Some(
                    args.get(idx + 1)
                        .context("--source requires a source id")?
                        .clone(),
                );
                idx += 2;
            }
            "--limit" => {
                let raw = args.get(idx + 1).context("--limit requires a number")?;
                limit = raw.parse::<usize>().context("invalid --limit value")?;
                idx += 2;
            }
            "--data-root" => {
                explicit_root = Some(PathBuf::from(
                    args.get(idx + 1).context("--data-root requires a path")?,
                ));
                idx += 2;
            }
            other => {
                query_parts.push(other.to_owned());
                idx += 1;
            }
        }
    }
    let query = query_parts.join(" ");
    if query.trim().is_empty() {
        bail!("Usage: mt ctx search <query> [--source <id>] [--limit <n>]");
    }
    let data_root = resolve_data_root(explicit_root.as_deref(), default_data_root)?;
    let store = ContextGuardStore::open(&data_root)?;
    let hits = store.search_session(
        &query,
        source_id.as_deref(),
        &resolve_session_id(session),
        limit,
    )?;
    if hits.is_empty() {
        return Ok(format!("No matches for `{query}`."));
    }
    let mut out = format!("Found {} matches for `{query}`:\n", hits.len());
    for hit in hits {
        out.push_str(&format!(
            "\n--- {} (chunk {}, {}) ---\n{}\n",
            hit.source_id, hit.chunk_index, hit.title, hit.snippet
        ));
    }
    Ok(out)
}

fn cmd_log(args: &[String], default_data_root: Option<&Path>) -> Result<String> {
    let usage = "Usage: mt ctx log <id> [--offset <bytes>] [--data-root <path>]";
    let id = args.first().context(usage)?;
    anyhow::ensure!(
        id.strip_prefix("log_").is_some_and(|seq| {
            !seq.is_empty() && seq.bytes().all(|byte| byte.is_ascii_digit())
        }),
        "{usage}"
    );
    let mut options = args[1..].chunks_exact(2);
    anyhow::ensure!(options.remainder().is_empty(), "{usage}");
    let mut offset = 0;
    let mut explicit_root = None;
    for option in &mut options {
        match option[0].as_str() {
            "--offset" => offset = option[1].parse::<usize>().context("invalid --offset")?,
            "--data-root" => explicit_root = Some(Path::new(&option[1])),
            _ => bail!("{usage}"),
        }
    }
    let root = resolve_data_root(explicit_root, default_data_root).or_else(|error| {
        // Managed runs already receive this absolute path under their data root.
        let roles = std::env::var_os("MATOU_FOREMAN_ROLES_FILE").map(PathBuf::from);
        roles
            .filter(|path| {
                path.is_absolute()
                    && path.file_name().is_some_and(|name| {
                        name == matou_native_core::dispatch_roles::ROLES_FILENAME
                    })
            })
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .ok_or(error)
    })?;
    let store = ContextGuardStore::open(&root)?;
    let Some(log) = store.get_full_log(id)? else {
        store.require_log_source(id)?;
        bail!("Log `{id}` has no readable content.");
    };
    let remainder = log.get(offset..).context("invalid log byte offset")?;
    if remainder.len() <= CLI_TEXT_CAP_BYTES {
        return Ok(remainder.to_owned());
    }
    // Reserve space for the continuation instruction, including its byte offset.
    let tail = format!(
        "\n\n[Continue with `mt ctx log {id} --offset {}`.]",
        log.len()
    );
    let end = remainder.floor_char_boundary(CLI_TEXT_CAP_BYTES.saturating_sub(tail.len()));
    Ok(format!(
        "{}\n\n[Continue with `mt ctx log {id} --offset {}`.]",
        &remainder[..end],
        offset + end
    ))
}

fn cmd_resume(args: &[String], default_data_root: Option<&Path>) -> Result<String> {
    let mut session = None;
    let mut explicit_root: Option<PathBuf> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--session" => {
                session = Some(
                    args.get(idx + 1)
                        .context("--session requires an id")?
                        .as_str(),
                );
                idx += 2;
            }
            "--data-root" => {
                explicit_root = Some(PathBuf::from(
                    args.get(idx + 1).context("--data-root requires a path")?,
                ));
                idx += 2;
            }
            other => bail!("Unexpected argument for `mt ctx resume`: {other}"),
        }
    }
    let session_id = resolve_session_id(session);
    let data_root = resolve_data_root(explicit_root.as_deref(), default_data_root)?;
    let store = ContextGuardStore::open(&data_root)?;
    let logs = store.list_session_logs(&session_id, 10)?;
    let session_label = &session_id[..session_id.floor_char_boundary(200)];
    let mut out = format!("Post-compact resume clues for session `{session_label}`:\n");
    if let Some(changes) = git_changed_files() {
        out.push_str(&changes);
    }
    out.push_str("\nRecent saved logs and command status:\n");
    if logs.is_empty() {
        out.push_str("(no saved context sources)\n");
    }
    for rec in logs {
        let command = rec.command.lines().next().unwrap_or_default();
        let preview = &command[..command.floor_char_boundary(320)];
        let tool = &rec.tool_name[..rec.tool_name.floor_char_boundary(100)];
        out.push_str(&format!(
            "- `{}` ({tool}, {} bytes): {preview}{}\n",
            rec.id,
            rec.raw_bytes,
            if preview.len() < rec.command.len() {
                "..."
            } else {
                ""
            },
        ));
    }
    if store.mark_manual_resume(&session_id).is_ok() {
        out.push_str("\nManual resume recorded.");
    } else {
        out.push_str("\nCould not record the manual resume marker.");
    }
    Ok(out)
}

fn git_changed_files() -> Option<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let mut out = "\nGit changed files:\n".to_owned();
    if output.stdout.is_empty() {
        out.push_str("(working tree clean)\n");
        return Some(out);
    }
    let mut items = output.stdout.split(|&byte| byte == 0);
    while let Some(item) = items.next().filter(|item| item.len() >= 3) {
        let status = String::from_utf8_lossy(&item[..2]);
        let path = String::from_utf8_lossy(&item[3..]);
        let source = item[..2]
            .iter()
            .any(|b| matches!(b, b'R' | b'C'))
            .then(|| items.next())
            .flatten();
        let entry = match source {
            Some(source) => format!(
                "- {status} {} -> {}\n",
                String::from_utf8_lossy(source).escape_debug(),
                path.escape_debug()
            ),
            None => format!("- {status} {}\n", path.escape_debug()),
        };
        if out.len() + entry.len() > 3_500 {
            out.push_str("... (more changed files omitted)\n");
            break;
        }
        out.push_str(&entry);
    }
    Some(out)
}

fn resolve_session_id(explicit: Option<&str>) -> String {
    explicit
        .map(str::to_owned)
        .or_else(|| {
            env::var("MATOU_SESSION_ID")
                .ok()
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            env::var("MATOU_CONTROL_CALLER_SESSION")
                .ok()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "default".to_owned())
}

fn resolve_data_root(explicit: Option<&Path>, default_root: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Some(env_root) = env::var_os("MATOU_DATA_ROOT").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(env_root));
    }
    if let Some(root) = default_root {
        return Ok(root.to_path_buf());
    }
    if let Some(root) = platform_data_root().filter(|root| root.is_dir()) {
        return Ok(root);
    }
    bail!("Matou data directory not found; set MATOU_DATA_ROOT or pass --data-root <path>")
}

#[cfg(not(windows))]
fn platform_data_root() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".matou"))
}

#[cfg(windows)]
fn platform_data_root() -> Option<PathBuf> {
    let root =
        PathBuf::from(env::var_os("APPDATA").filter(|value| !value.is_empty())?).join("Matou");
    if !root.join("matou.sqlite").exists()
        && let Some(home) = env::var_os("USERPROFILE").filter(|value| !value.is_empty())
    {
        let legacy = PathBuf::from(home).join(".matou");
        if legacy.join("matou.sqlite").exists() {
            return Some(legacy);
        }
    }
    Some(root)
}

fn cap_output(text: &str) -> String {
    if text.len() <= CLI_TEXT_CAP_BYTES {
        return text.to_owned();
    }
    let suffix = "\n... [capped at 8.8 KB]";
    let budget = CLI_TEXT_CAP_BYTES - suffix.len();
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mt_ctx_index_search_and_resume_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("deploy.log");
        fs::write(
            &doc,
            "step 1: build ok\nstep 2: ERROR database_migration_failed token=sk-secret1234567890abcdef\nstep 3: exit 1\n",
        )
        .unwrap();

        let indexed = run_ctx_command(
            &[
                "index".into(),
                doc.display().to_string(),
                "--session".into(),
                "s-42".into(),
            ],
            Some(dir.path()),
        )
        .unwrap();
        assert!(indexed.contains("log_0001"));

        let found = run_ctx_command(
            &[
                "search".into(),
                "database_migration_failed".into(),
                "--source".into(),
                "log_0001".into(),
            ],
            Some(dir.path()),
        )
        .unwrap();
        assert!(found.contains("database_migration_failed"));
        assert!(!found.contains("sk-secret1234567890abcdef"));
        assert!(found.contains("[REDACTED]"));

        let resumed = run_ctx_command(
            &["resume".into(), "--session".into(), "s-42".into()],
            Some(dir.path()),
        )
        .unwrap();
        assert!(resumed.contains("log_0001"));
    }
}
