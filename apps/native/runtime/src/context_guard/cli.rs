//! Zero-MCP `mt ctx` CLI subcommand handler.
//!
//! Invoked directly by `mt_cli::run_with` when the first argument is `ctx`.
//! Runs in-process with zero network hops and enforces the 8.8 KB response cap.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::context_guard::{
    filter::HARD_CAP_BYTES,
    outline::{batch_search, extract_file_outline},
    redact_secrets,
    store::ContextGuardStore,
};

const CTX_HELP: &str = "\
mt ctx — Matou built-in zero-MCP context helpers (all outputs <= 8.8 KB)

Usage:
  mt ctx outline <file>
  mt ctx batch-search <query1> [query2 ...] [--dir <path>]
  mt ctx index <file> [--session <id>] [--data-root <path>]
  mt ctx search <query> [--source <id>] [--limit <n>] [--data-root <path>]
  mt ctx resume [--session <id>] [--data-root <path>]
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
        "search" => cmd_search(rest, default_data_root)?,
        "resume" => cmd_resume(rest, default_data_root)?,
        other => bail!("Unknown `mt ctx` subcommand: {other}\n\n{CTX_HELP}"),
    };
    Ok(cap_output(&redact_secrets(&raw)))
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
    let mut session_id = std::env::var("MATOU_SESSION_ID").unwrap_or_else(|_| "default".to_owned());
    let mut explicit_root: Option<PathBuf> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--session" => {
                session_id = args
                    .get(idx + 1)
                    .context("--session requires an id")?
                    .clone();
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
    let saved = store.save_log(&session_id, "mt-ctx-index", &path.display().to_string(), &redacted)?;
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
    let mut limit: usize = 5;
    let mut explicit_root: Option<PathBuf> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
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
    let hits = store.search(&query, source_id.as_deref(), limit)?;
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

fn cmd_resume(args: &[String], default_data_root: Option<&Path>) -> Result<String> {
    let mut session_id = std::env::var("MATOU_SESSION_ID").unwrap_or_else(|_| "default".to_owned());
    let mut explicit_root: Option<PathBuf> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--session" => {
                session_id = args
                    .get(idx + 1)
                    .context("--session requires an id")?
                    .clone();
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
    let data_root = resolve_data_root(explicit_root.as_deref(), default_data_root)?;
    let store = ContextGuardStore::open(&data_root)?;
    let logs = store.list_session_logs(&session_id, 10)?;
    if logs.is_empty() {
        return Ok(format!(
            "No saved context sources found for session `{session_id}`."
        ));
    }
    let mut out = format!(
        "Post-compact resume clues for session `{session_id}` ({} saved sources):\n",
        logs.len()
    );
    for rec in logs {
        out.push_str(&format!(
            "- `{}` ({}, {} KB): {}\n",
            rec.id,
            rec.tool_name,
            rec.raw_bytes / 1024,
            rec.command
        ));
    }
    out.push_str("\nQuery any source with: mt ctx search <query> --source <id>\n");
    Ok(out)
}

fn resolve_data_root(explicit: Option<&Path>, default_root: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Some(p) = default_root {
        return Ok(p.to_path_buf());
    }
    if let Some(env_root) = std::env::var_os("MATOU_DATA_ROOT").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(env_root));
    }
    bail!("MATOU_DATA_ROOT is not set; pass --data-root <path>")
}

fn cap_output(text: &str) -> String {
    if text.len() <= HARD_CAP_BYTES {
        return text.to_owned();
    }
    let suffix = "\n... [capped at 8.8 KB]";
    let budget = HARD_CAP_BYTES.saturating_sub(suffix.len());
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
