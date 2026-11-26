use std::{
    path::{Path, PathBuf},
    sync::{LazyLock, mpsc},
};

use matou_native_core::git_repo::{git_command, git_status_porcelain};
use regex::Regex;

use serde_json::Value;

use super::{redact_secrets, store::ContextGuardStore};

const LOG_LIMIT: usize = 10;

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
    let Ok(status) = git_status_porcelain(cwd) else {
        return Vec::new();
    };
    let repo_root = git_command(cwd)
        .ok()
        .and_then(|mut command| command.args(["rev-parse", "--show-toplevel"]).output().ok())
        .filter(|output| output.status.success())
        .map(|output| {
            PathBuf::from(
                String::from_utf8_lossy(&output.stdout)
                    .trim_end_matches('\n')
                    .trim_end_matches('\r'),
            )
        });
    let repo_root = repo_root.as_deref().unwrap_or(cwd);
    let mut entries = status.split(|byte| *byte == 0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use serde_json::json;

    #[test]
    fn recent_log_ids_are_session_scoped() -> Result<()> {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let store = ContextGuardStore::open(root.path())?;
        let source = store.save_log("owner", "Bash", "cargo test", "output")?;
        store.save_log("other", "Bash", "echo other", "other card")?;
        assert_eq!(recent_log_ids(root.path(), "owner"), [source.id]);
        assert!(recent_log_ids(root.path(), "other").len() == 1);
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
