//! Zero-dependency native Rust file outline extractor and multi-pattern
//! workspace batch search. All outputs obey the 8.8 KB hard ceiling.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use regex::RegexSetBuilder;

use crate::context_guard::filter::HARD_CAP_BYTES;

const MAX_OUTLINE_FILE_BYTES: usize = 2 * 1024 * 1024; // 2 MiB
const MAX_BATCH_FILE_BYTES: usize = 512 * 1024; // 512 KiB
const MAX_BATCH_FILES_SCANNED: usize = 4_000;
const MAX_MATCHES_PER_QUERY: usize = 15;

/// Produces a compact structural outline (line numbers + top-level signatures,
/// headings, or diff file headers) capped at 8.8 KB.
pub fn extract_file_outline(path: &Path) -> Result<String> {
    let meta = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if !meta.is_file() {
        anyhow::bail!("Not a regular file: {}", path.display());
    }
    if meta.len() as usize > MAX_OUTLINE_FILE_BYTES {
        anyhow::bail!(
            "File too large for outline ({} KB > {} KB)",
            meta.len() / 1024,
            MAX_OUTLINE_FILE_BYTES / 1024
        );
    }
    let content = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let total_lines = content.lines().count();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let mut entries = Vec::new();
    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_outline_line(&ext, raw_line, trimmed) {
            let preview = truncate_line(trimmed, 120);
            entries.push(format!("L{line_no}: {preview}"));
        }
    }

    let header = format!(
        "Outline: {} ({total_lines} lines, {} bytes, {} items)\n",
        path.display(),
        content.len(),
        entries.len()
    );
    let mut out = header;
    let mut omitted = 0usize;
    for entry in &entries {
        if out.len() + entry.len() + 48 > HARD_CAP_BYTES {
            omitted += 1;
            continue;
        }
        out.push_str(entry);
        out.push('\n');
    }
    if entries.is_empty() {
        out.push_str("(no top-level symbols or headings detected; read targeted line ranges)\n");
    } else if omitted > 0 {
        out.push_str(&format!("... ({omitted} more outline entries omitted)\n"));
    }
    Ok(out)
}

/// Searches multiple case-insensitive literal queries across `root` in a single pass
/// and returns a compact, deduplicated report capped at 8.8 KB.
pub fn batch_search(root: &Path, queries: &[String]) -> Result<String> {
    let mut seen = HashSet::new();
    let clean_queries: Vec<&str> = queries
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .filter(|q| seen.insert(q.to_lowercase()))
        .take(12)
        .collect();
    if clean_queries.is_empty() {
        anyhow::bail!("At least one non-empty query is required");
    }

    let patterns = RegexSetBuilder::new(clean_queries.iter().map(|query| regex::escape(query)))
        .case_insensitive(true)
        .build()
        .context("compile batch search queries")?;

    let mut hits = vec![0; clean_queries.len()];
    let mut rows = String::new();
    let mut files_scanned = 0usize;
    let mut stack = vec![root.to_path_buf()];
    let mut capped = false;
    let root_label = root.display().to_string();
    let root_label = truncate_line(&root_label, 512);

    'scan: while let Some(dir) = stack.pop() {
        let Ok(read_dir) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = read_dir.flatten().map(|e| e.path()).collect();
        entries.sort();
        for entry in entries {
            let name = entry
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let Ok(meta) = fs::symlink_metadata(&entry) else {
                continue;
            };
            if meta.is_dir() {
                if !is_ignored_dir(name) {
                    stack.push(entry);
                }
                continue;
            }
            if !meta.is_file() || meta.len() as usize > MAX_BATCH_FILE_BYTES {
                continue;
            }
            if files_scanned >= MAX_BATCH_FILES_SCANNED {
                break 'scan;
            }
            files_scanned += 1;
            let Ok(text) = fs::read_to_string(&entry) else {
                continue;
            };
            let rel = entry.strip_prefix(root).unwrap_or(&entry);
            for (idx, line) in text.lines().enumerate() {
                let matched = patterns.matches(line);
                if !matched
                    .iter()
                    .any(|index| hits[index] < MAX_MATCHES_PER_QUERY)
                {
                    continue;
                }
                let labels = matched
                    .iter()
                    .map(|index| {
                        hits[index] += 1;
                        truncate_line(clean_queries[index], 96)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let path = rel.to_string_lossy().replace('\\', "/");
                let row = format!(
                    "  {}:{}: {} [matches: {labels}]\n",
                    truncate_line(&path, 512),
                    idx + 1,
                    truncate_line(line.trim(), 100),
                );
                // Reserve room for the final header and cap notice.
                if rows.len() + row.len() + root_label.len() + 160 > HARD_CAP_BYTES {
                    capped = true;
                    break 'scan;
                }
                rows.push_str(&row);
                if hits.iter().all(|&count| count >= MAX_MATCHES_PER_QUERY) {
                    break 'scan;
                }
            }
        }
    }

    let mut out = format!(
        "Batch search in {root_label} ({files_scanned} files scanned, {} queries):\n",
        clean_queries.len()
    );
    if rows.is_empty() {
        out.push_str("  (no matches)\n");
    }
    out.push_str(&rows);
    if capped {
        out.push_str("... (output capped at 8.8 KB)\n");
    }
    Ok(out)
}

fn is_outline_line(ext: &str, raw_line: &str, trimmed: &str) -> bool {
    let indent = raw_line.len() - raw_line.trim_start().len();
    match ext {
        "md" | "markdown" => trimmed.starts_with('#'),
        "patch" | "diff" => {
            trimmed.starts_with("diff --git")
                || trimmed.starts_with("+++ ")
                || trimmed.starts_with("@@ ")
        }
        "rs" => {
            indent <= 4
                && (trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("pub(crate) fn ")
                    || trimmed.starts_with("pub async fn ")
                    || trimmed.starts_with("fn ")
                    || trimmed.starts_with("async fn ")
                    || trimmed.starts_with("pub struct ")
                    || trimmed.starts_with("struct ")
                    || trimmed.starts_with("pub enum ")
                    || trimmed.starts_with("enum ")
                    || trimmed.starts_with("pub trait ")
                    || trimmed.starts_with("trait ")
                    || trimmed.starts_with("impl ")
                    || trimmed.starts_with("impl<")
                    || trimmed.starts_with("pub mod ")
                    || trimmed.starts_with("mod "))
        }
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => {
            indent <= 2
                && (trimmed.starts_with("export ")
                    || trimmed.starts_with("function ")
                    || trimmed.starts_with("async function ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("interface ")
                    || trimmed.starts_with("type ")
                    || trimmed.starts_with("const "))
        }
        "py" => {
            indent <= 4
                && (trimmed.starts_with("def ")
                    || trimmed.starts_with("async def ")
                    || trimmed.starts_with("class "))
        }
        "go" => {
            indent == 0
                && (trimmed.starts_with("func ") || trimmed.starts_with("type "))
        }
        _ => {
            indent == 0
                && (trimmed.starts_with('#')
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("def ")
                    || trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("function "))
        }
    }
}

fn is_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".turbo"
            | ".cache"
            | "vendor"
            | "__pycache__"
            | ".venv"
    )
}

fn truncate_line(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_and_markdown_outlines_with_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let rs_file = dir.path().join("demo.rs");
        fs::write(
            &rs_file,
            "pub struct Foo {\n    x: i32,\n}\n\nimpl Foo {\n    pub fn bar(&self) -> i32 {\n        self.x\n    }\n}\n",
        )
        .unwrap();

        let outline = extract_file_outline(&rs_file).unwrap();
        assert!(outline.contains("L1: pub struct Foo"));
        assert!(outline.contains("L5: impl Foo"));
        assert!(outline.contains("L6: pub fn bar(&self) -> i32"));
        assert!(outline.len() <= HARD_CAP_BYTES);
    }

    #[test]
    fn batch_search_finds_multiple_queries_and_skips_ignored_dirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        fs::write(
            dir.path().join("src/a.rs"),
            "fn alpha_token() {} // beta_token\nfn beta_token() {}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("node_modules/pkg/index.js"),
            "const alpha_token = 1;\n",
        )
        .unwrap();

        let out = batch_search(
            dir.path(),
            &["alpha_token".to_owned(), "beta_token".to_owned()],
        )
        .unwrap();
        assert_eq!(out.matches("src/a.rs:1").count(), 1);
        assert!(out.contains("[matches: alpha_token, beta_token]"));
        assert!(out.contains("src/a.rs:2"));
        assert!(!out.contains("node_modules"));
        assert!(out.len() <= HARD_CAP_BYTES);
    }

    #[test]
    fn batch_search_caps_many_matches_without_splitting_a_utf8_row() {
        let dir = tempfile::tempdir().unwrap();
        let queries: Vec<_> = (0..12)
            .map(|index| format!("{index}{}", "检".repeat(30)))
            .collect();
        fs::write(
            dir.path().join("many.txt"),
            format!("{}\n", queries.join(" ")).repeat(20),
        )
        .unwrap();
        let out = batch_search(dir.path(), &queries).unwrap();
        assert!(out.len() <= HARD_CAP_BYTES);
        assert!(out.contains("output capped"));
        assert!(
            out.lines()
                .filter(|line| line.contains("many.txt"))
                .all(|line| line.ends_with(']'))
        );
    }
}
