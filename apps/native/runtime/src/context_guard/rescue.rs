use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{LazyLock, mpsc},
};

use regex::Regex;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use super::{redact_secrets, store::ContextGuardStore};

const LOG_LIMIT: usize = 10;
const LOG_PREVIEW_BYTES: usize = 8 * 1024;
const LOG_COMMAND_BYTES: usize = 512;
const LOG_LIST_PREVIEW_BYTES: usize = 240;

pub(crate) fn redact(text: &str, secrets: &[String]) -> String {
    let mut text = text.to_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        text = text.replace(secret, "[REDACTED]");
    }
    static CREDENTIALS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r#"(?ix)
        (\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|password|passwd)
             ["']?\s*[:=]\s*)
             (?:"[^"\r\n]+"|'[^'\r\n]+'|[^\s,;'"<>`]+)
        | (\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)
        | (-----BEGIN\x20(?:[A-Z]+\x20)?PRIVATE\x20KEY-----[\s\S]*?(?:-----END\x20(?:[A-Z]+\x20)?PRIVATE\x20KEY-----|\z))
        "#
    ).expect("credential redaction pattern")
    });
    CREDENTIALS
        .replace_all(&redact_secrets(&text), "${1}[REDACTED]")
        .into_owned()
}

/// Plain complete lines are progress. Terminal repaints, cursor movement and
/// in-place spinners are already covered by provider activity when meaningful.
pub(crate) fn has_completed_output_line(data: &str) -> bool {
    data.split_inclusive('\n').any(|line| {
        let Some(line) = line.strip_suffix('\n') else {
            return false;
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        !line.chars().any(|ch| ch.is_control() && ch != '\t')
            && line.chars().filter(|ch| ch.is_alphabetic()).take(3).count() == 3
    })
}

pub(crate) fn start_git_snapshot(cwd: PathBuf) -> std::io::Result<mpsc::Receiver<Vec<String>>> {
    let (send, receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("context-handoff-git".into())
        .spawn(move || {
            let _ = send.send(git_modified_files(&cwd));
        })?;
    Ok(receive)
}

fn git_modified_files(cwd: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let repo_root = Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            PathBuf::from(
                String::from_utf8_lossy(&output.stdout)
                    .trim_end_matches('\n')
                    .trim_end_matches('\r'),
            )
        });
    let repo_root = repo_root.as_deref().unwrap_or(cwd);
    let mut entries = output.stdout.split(|byte| *byte == 0);
    let mut files = Vec::new();
    while let Some(entry) = entries.next().filter(|entry| entry.len() > 3) {
        for path in std::iter::once(&entry[3..]).chain(
            entry[..2]
                .iter()
                .any(|flag| matches!(flag, b'R' | b'C'))
                .then(|| entries.next())
                .flatten(),
        ) {
            let path = String::from_utf8_lossy(path);
            let absolute = repo_root.join(path.as_ref());
            files.push(display_path(cwd, &absolute));
        }
    }
    files
}

pub(crate) fn display_path(cwd: &Path, path: &Path) -> String {
    let path = if path.is_absolute() {
        path.strip_prefix(cwd).unwrap_or(path)
    } else {
        path.strip_prefix(".").unwrap_or(path)
    };
    path.to_string_lossy().into_owned()
}

/// Only paths in input fields that a known write tool treats as targets.
pub(crate) fn write_targets(name: &str, input: &Value) -> Vec<String> {
    if !matches!(
        name.to_ascii_lowercase().as_str(),
        "edit"
            | "multiedit"
            | "write"
            | "notebookedit"
            | "write_to_file"
            | "replace_file_content"
            | "multi_replace_file_content"
            | "apply_patch"
            | "write_file"
            | "edit_file"
    ) {
        return Vec::new();
    }

    let mut targets: Vec<_> = [
        "file_path",
        "filePath",
        "notebook_path",
        "notebookPath",
        "path",
        "TargetFile",
        "targetFile",
        "target_file",
        "AbsolutePath",
        "Path",
    ]
    .into_iter()
    .filter_map(|key| input[key].as_str())
    .map(str::to_owned)
    .collect();
    if name.eq_ignore_ascii_case("apply_patch") {
        if let Some(patch) = input
            .as_str()
            .or_else(|| input["patch"].as_str())
            .or_else(|| input["input"].as_str())
        {
            targets.extend(patch.lines().filter_map(|line| {
                [
                    "*** Add File: ",
                    "*** Update File: ",
                    "*** Delete File: ",
                    "*** Move to: ",
                ]
                .into_iter()
                .find_map(|prefix| line.strip_prefix(prefix).map(str::to_owned))
            }));
        }
    }
    targets
}

pub(crate) fn recent_log_ids(root: &Path, session: &str) -> Vec<String> {
    ContextGuardStore::open(root)
        .and_then(|store| store.list_session_logs(session, 50))
        .unwrap_or_default()
        .into_iter()
        .filter(|log| log.tool_name != "mt-ctx-index")
        .take(LOG_LIMIT)
        .map(|log| log.id)
        .collect()
}

pub(crate) fn recent_logs(root: &Path, session: &str, secrets: &[String]) -> Result<Vec<Value>> {
    let Some(db) = open_log_reader(root)? else {
        return Ok(Vec::new());
    };
    let mut list = db.prepare(
        "SELECT id, command, raw_bytes FROM log_sources
         WHERE session_id = ? AND tool_name <> 'mt-ctx-index' ORDER BY seq DESC LIMIT ?",
    )?;
    let rows = list.query_map(params![session, LOG_LIMIT as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (id, command, raw_bytes) = row?;
        let preview = read_log_preview(&db, session, &id, secrets)?.unwrap_or_default();
        Ok(json!({
            "id": id,
            "command": clip(&redact(&command, secrets), LOG_COMMAND_BYTES),
            "rawBytes": raw_bytes,
            "preview": clip(&preview, LOG_LIST_PREVIEW_BYTES),
        }))
    })
    .collect()
}

pub(crate) fn log_preview(
    root: &Path,
    session: &str,
    id: &str,
    secrets: &[String],
) -> Result<Option<String>> {
    match open_log_reader(root)? {
        Some(db) => read_log_preview(&db, session, id, secrets),
        None => Ok(None),
    }
}

fn open_log_reader(root: &Path) -> Result<Option<Connection>> {
    let path = root.join("context_guard.sqlite");
    if !path.is_file() {
        return Ok(None);
    }
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(std::time::Duration::from_millis(80))?;
    Ok(Some(db))
}

fn read_log_preview(
    db: &Connection,
    session: &str,
    id: &str,
    secrets: &[String],
) -> Result<Option<String>> {
    // Match the reconstruction rule used by the log store. Take a bounded
    // prefix plus look-ahead so redaction sees credentials across chunk boundaries.
    let Some((size, stored)) = db
        .query_row(
            "SELECT raw_bytes, COALESCE((SELECT SUM(length(CAST(body AS BLOB)))
           FROM log_chunks WHERE source_id = log_sources.id), 0)
         FROM log_sources WHERE session_id = ? AND id = ? AND tool_name <> 'mt-ctx-index' ",
            params![session, id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut query =
        db.prepare("SELECT body FROM log_chunks WHERE source_id = ? ORDER BY chunk_index")?;
    let mut chunks = query.query([id])?;
    let mut text = String::new();
    while let Some(row) = chunks.next()? {
        if size != stored && !text.is_empty() {
            text.push('\n');
        }
        text.push_str(row.get_ref(0)?.as_str()?);
        if text.len() > LOG_PREVIEW_BYTES * 2 {
            break;
        }
    }
    Ok(Some(
        clip(&redact(&text, secrets), LOG_PREVIEW_BYTES).to_owned(),
    ))
}

fn clip(text: &str, max: usize) -> &str {
    &text[..text.floor_char_boundary(text.len().min(max))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_are_session_scoped_bounded_and_redacted() -> Result<()> {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let store = ContextGuardStore::open(root.path())?;
        let raw = format!(
            "{}\nAPI_KEY=private-sample-value\n{}",
            "测试".repeat(580),
            "内容".repeat(1_700),
        );
        let source = store.save_log(
            "owner",
            "Bash",
            "cargo test --password=private-command",
            &raw,
        )?;
        store.save_log("other", "Bash", "echo other", "other card")?;
        let details = recent_logs(root.path(), "owner", &[])?;
        assert_eq!(details.len(), 1);
        assert_eq!(details[0]["id"], source.id);
        assert_eq!(details[0]["rawBytes"], raw.len());
        assert!(
            !details[0]["command"]
                .as_str()
                .unwrap()
                .contains("private-command")
        );
        assert!(log_preview(root.path(), "other", &source.id, &[])?.is_none());
        let preview = log_preview(root.path(), "owner", &source.id, &[])?.unwrap();
        assert!(preview.len() <= LOG_PREVIEW_BYTES);
        assert!(preview.contains("API_KEY=[REDACTED]"));
        assert!(!preview.contains("private-sample-value"));
        assert_eq!(recent_log_ids(root.path(), "owner"), [source.id]);
        Ok(())
    }

    #[test]
    fn patch_targets_match_written_files_without_taking_context_lines() {
        let patch =
            "*** Update File: src/lib.rs\n+text\n*** Add File: new file.rs\n*** Move to: moved.rs";
        assert_eq!(
            write_targets("apply_patch", &json!({"patch": patch})),
            ["src/lib.rs", "new file.rs", "moved.rs"]
        );
        assert!(write_targets("Read", &json!({"file_path":"other.rs"})).is_empty());
        assert_eq!(
            write_targets("NotebookEdit", &json!({"notebook_path":"math.ipynb"})),
            ["math.ipynb"]
        );
    }
}
