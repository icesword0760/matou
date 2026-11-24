//! Zero-dependency native Rust file outline extractor and multi-pattern
//! workspace batch search. All outputs obey the 8.8 KB hard ceiling.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use regex::Regex;

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

/// Searches multiple literal or regex queries across `root` in a single pass
/// and returns a compact, deduplicated report capped at 8.8 KB.
pub fn batch_search(root: &Path, queries: &[String]) -> Result<String> {
    let clean_queries: Vec<&str> = queries
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .take(12)
        .collect();
    if clean_queries.is_empty() {
        anyhow::bail!("At least one non-empty query is required");
    }

    let compiled: Vec<(&str, Option<Regex>)> = clean_queries
        .iter()
        .map(|&q| (q, Regex::new(&format!("(?i){}", regex::escape(q))).ok()))
        .collect();

    let mut results: Vec<Vec<String>> = vec![Vec::new(); compiled.len()];
    let mut files_scanned = 0usize;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        if files_scanned >= MAX_BATCH_FILES_SCANNED {
            break;
        }
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
            if entry.is_dir() {
                if !is_ignored_dir(name) {
                    stack.push(entry);
                }
                continue;
            }
            let Ok(meta) = fs::metadata(&entry) else {
                continue;
            };
            if !meta.is_file() || meta.len() as usize > MAX_BATCH_FILE_BYTES {
                continue;
            }
            files_scanned += 1;
            let Ok(text) = fs::read_to_string(&entry) else {
                continue;
            };
            let rel = entry.strip_prefix(root).unwrap_or(&entry);
            for (idx, line) in text.lines().enumerate() {
                for (q_idx, (_, re_opt)) in compiled.iter().enumerate() {
                    if results[q_idx].len() >= MAX_MATCHES_PER_QUERY {
                        continue;
                    }
                    if re_opt.as_ref().is_some_and(|re| re.is_match(line)) {
                        results[q_idx].push(format!(
                            "  {}:{}: {}",
                            rel.display(),
                            idx + 1,
                            truncate_line(line.trim(), 100)
                        ));
                    }
                }
            }
        }
    }

    let mut out = format!(
        "Batch search in {} ({files_scanned} files scanned, {} queries):\n",
        root.display(),
        compiled.len()
    );
    for (q_idx, (query, _)) in compiled.iter().enumerate() {
        let section = format!("\n## Query `{query}` ({} hits)\n", results[q_idx].len());
        if out.len() + section.len() + 64 > HARD_CAP_BYTES {
            out.push_str("\n... (output capped at 8.8 KB)\n");
            break;
        }
        out.push_str(&section);
        if results[q_idx].is_empty() {
            out.push_str("  (no matches)\n");
            continue;
        }
        for hit in &results[q_idx] {
            if out.len() + hit.len() + 48 > HARD_CAP_BYTES {
                out.push_str("  ... (truncated)\n");
                break;
            }
            out.push_str(hit);
            out.push('\n');
        }
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
            "fn alpha_token() {}\nfn beta_token() {}\n",
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
        assert!(out.contains("src/a.rs:1"));
        assert!(out.contains("src/a.rs:2"));
        assert!(!out.contains("node_modules"));
        assert!(out.len() <= HARD_CAP_BYTES);
    }
}
