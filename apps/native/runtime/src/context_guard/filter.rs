//! Deterministic terminal noise trimming, large JSON crushing, secret
//! redaction, and sensitive-path protection.
//!
//! All trimmed outputs obey a hard 8.8 KB ceiling so host agents never spill
//! tool results into temporary files that force an extra read turn.

use std::{path::Path, sync::LazyLock};

use regex::Regex;
use serde_json::Value;

use crate::context_guard::redact_secrets;

pub const HARD_CAP_BYTES: usize = 8_800;

static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)").unwrap()
});

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrimResult {
    pub text: String,
    pub original_bytes: usize,
    pub saved_bytes: usize,
    pub trimmed: bool,
}

/// Returns `true` if `path` looks like a secret-bearing file (`.env`, private
/// keys, credentials) while allowing safe templates like `.env.example`.
pub fn is_sensitive_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower == ".env" || lower.starts_with(".env.") {
        let is_template = lower.ends_with(".example")
            || lower.ends_with(".sample")
            || lower.ends_with(".template")
            || lower.ends_with(".dist");
        return !is_template;
    }
    matches!(
        lower.as_str(),
        "id_rsa"
            | "id_ed25519"
            | "id_ecdsa"
            | ".npmrc"
            | ".netrc"
            | "credentials.json"
            | "secrets.json"
            | "secrets.yaml"
            | "secrets.yml"
    ) || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
}

/// Strips ANSI escapes, collapses carriage-return progress overwrites, redacts
/// secrets, and compresses noisy build/test/JSON outputs down to <= 8.8 KB.
pub fn trim_terminal_output(
    raw: &str,
    command: &str,
    threshold_bytes: usize,
    log_source_id: Option<&str>,
) -> TrimResult {
    let original_bytes = raw.len();
    let cleaned = redact_secrets(&strip_ansi_and_progress(raw));
    let threshold = threshold_bytes.max(HARD_CAP_BYTES);

    if cleaned.len() <= threshold && cleaned.len() <= HARD_CAP_BYTES {
        let saved = original_bytes.saturating_sub(cleaned.len());
        return TrimResult {
            text: cleaned,
            original_bytes,
            saved_bytes: saved,
            trimmed: false,
        };
    }

    // Attempt large JSON summary first if the output looks like JSON.
    let trimmed_body = if looks_like_json(&cleaned) {
        crush_json(&cleaned).unwrap_or_else(|| compress_log_lines(&cleaned, command))
    } else {
        compress_log_lines(&cleaned, command)
    };

    let footer = match log_source_id {
        Some(id) => format!(
            "\n\n[matou context-guard] Trimmed {} -> ~{} bytes. Full log saved as `{id}` (query with `mt ctx search <query> --source {id}`).",
            original_bytes,
            trimmed_body.len().min(HARD_CAP_BYTES)
        ),
        None => format!(
            "\n\n[matou context-guard] Trimmed {} bytes of repetitive output.",
            original_bytes
        ),
    };

    let budget = HARD_CAP_BYTES.saturating_sub(footer.len());
    let mut final_text = truncate_utf8(&trimmed_body, budget).to_owned();
    final_text.push_str(&footer);

    let saved_bytes = original_bytes.saturating_sub(final_text.len());
    TrimResult {
        text: final_text,
        original_bytes,
        saved_bytes,
        trimmed: true,
    }
}

fn strip_ansi_and_progress(input: &str) -> String {
    let no_ansi = ANSI_RE.replace_all(input, "");
    let mut out = String::with_capacity(no_ansi.len().min(65_536));
    for (idx, raw_line) in no_ansi.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        // Keep only the last carriage-return segment on progress-bar lines.
        let segment = raw_line
            .rsplit('\r')
            .find(|s| !s.is_empty())
            .unwrap_or("");
        out.push_str(segment);
    }
    out
}

fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

fn crush_json(text: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(text.trim()).ok()?;
    let mut lines = Vec::new();
    match &parsed {
        Value::Array(items) => {
            lines.push(format!("JSON Array ({} items)", items.len()));
            let mut errors = Vec::new();
            for (idx, item) in items.iter().enumerate() {
                if item_looks_like_error(item) && errors.len() < 15 {
                    errors.push(format!("[{idx}] {}", compact_json_preview(item, 240)));
                }
            }
            if !errors.is_empty() {
                lines.push(format!("Error/failing rows ({} shown):", errors.len()));
                lines.extend(errors);
            }
            lines.push("Sample items:".to_owned());
            for (idx, item) in items.iter().take(5).enumerate() {
                lines.push(format!("[{idx}] {}", compact_json_preview(item, 240)));
            }
        }
        Value::Object(map) => {
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            lines.push(format!("JSON Object ({} top-level keys): {}", keys.len(), keys.join(", ")));
            for (k, v) in map.iter().take(25) {
                lines.push(format!("- {k}: {}", compact_json_preview(v, 220)));
            }
        }
        _ => return None,
    }
    Some(lines.join("\n"))
}

fn item_looks_like_error(val: &Value) -> bool {
    match val {
        Value::Object(map) => map.iter().any(|(k, v)| {
            let lk = k.to_ascii_lowercase();
            (lk.contains("err") || lk.contains("fail") || lk == "status" || lk == "level")
                && match v {
                    Value::String(s) => {
                        let ls = s.to_ascii_lowercase();
                        ls.contains("err") || ls.contains("fail") || ls == "fatal" || ls == "panic"
                    }
                    Value::Bool(b) => *b && (lk.contains("err") || lk.contains("fail")),
                    Value::Number(n) => n.as_i64().is_some_and(|code| code >= 400),
                    _ => false,
                }
        }),
        _ => false,
    }
}

fn compact_json_preview(val: &Value, max_bytes: usize) -> String {
    let encoded = serde_json::to_string(val).unwrap_or_default();
    if encoded.len() <= max_bytes {
        encoded
    } else {
        format!("{}...", truncate_utf8(&encoded, max_bytes))
    }
}

fn compress_log_lines(text: &str, command: &str) -> String {
    let all_lines: Vec<&str> = text.lines().collect();
    let total = all_lines.len();
    let mut keep = vec![false; total];

    // Keep head (first 12 lines) and tail (last 25 lines) for orientation & summary.
    for flag in keep.iter_mut().take(12.min(total)) {
        *flag = true;
    }
    for flag in keep.iter_mut().skip(total.saturating_sub(25)) {
        *flag = true;
    }

    // Keep every error/failure/panic/warning line plus +-2 context lines.
    for (idx, line) in all_lines.iter().enumerate() {
        if is_important_line(line) {
            let start = idx.saturating_sub(2);
            let end = (idx + 3).min(total);
            for flag in &mut keep[start..end] {
                *flag = true;
            }
        }
    }

    let mut out = Vec::new();
    let mut omitted_run = 0usize;
    let is_diff = command.to_ascii_lowercase().contains("git diff");
    for (idx, line) in all_lines.iter().enumerate() {
        let should_keep = keep[idx]
            || (is_diff && (line.starts_with("diff --git") || line.starts_with("@@ ")));
        if should_keep && !is_pure_noise_line(line) {
            if omitted_run > 0 {
                out.push(format!("... ({omitted_run} lines omitted) ..."));
                omitted_run = 0;
            }
            out.push((*line).to_owned());
        } else {
            omitted_run += 1;
        }
    }
    if omitted_run > 0 {
        out.push(format!("... ({omitted_run} lines omitted) ..."));
    }
    out.join("\n")
}

fn is_important_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("error")
        || l.contains("failed")
        || l.contains("failure")
        || l.contains("panic")
        || l.contains("exception")
        || l.contains("traceback")
        || l.contains("assertion")
        || l.contains("fatal:")
        || l.contains("test result:")
        || l.starts_with("e   ")
        || l.contains("--> ")
}

fn is_pure_noise_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("Compiling ")
        || trimmed.starts_with("Downloading ")
        || trimmed.starts_with("Downloaded ")
        || trimmed.starts_with("Fresh ")
        || trimmed.starts_with("Fetching ")
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
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
    fn sensitive_path_blocks_env_and_keys_but_allows_examples() {
        assert!(is_sensitive_path(Path::new("/repo/.env")));
        assert!(is_sensitive_path(Path::new("/repo/.env.local")));
        assert!(is_sensitive_path(Path::new("/home/user/.ssh/id_ed25519")));
        assert!(is_sensitive_path(Path::new("/repo/server.pem")));

        assert!(!is_sensitive_path(Path::new("/repo/.env.example")));
        assert!(!is_sensitive_path(Path::new("/repo/.env.sample")));
        assert!(!is_sensitive_path(Path::new("/repo/src/main.rs")));
    }

    #[test]
    fn trims_large_test_output_under_cap_and_preserves_errors() {
        let mut raw = String::new();
        for i in 0..800 {
            raw.push_str(&format!("   Compiling crate_{i} v0.1.0\n"));
            raw.push_str(&format!("test suite::pass_{i} ... ok\n"));
        }
        raw.push_str("error[E0425]: cannot find value `foo` in this scope\n");
        raw.push_str(" --> src/lib.rs:42:9\n");
        raw.push_str("test result: FAILED. 800 passed; 1 failed\n");

        let res = trim_terminal_output(&raw, "cargo test", 16_384, Some("log_0007"));
        assert!(res.trimmed);
        assert!(res.text.len() <= HARD_CAP_BYTES);
        assert!(res.saved_bytes > 20_000);
        assert!(res.text.contains("cannot find value `foo`"));
        assert!(res.text.contains("log_0007"));
    }

    #[test]
    fn crushes_large_json_array_and_surfaces_error_rows() {
        let mut items = Vec::new();
        for i in 0..300 {
            if i == 150 {
                items.push(serde_json::json!({ "id": i, "status": "failed", "error": "timeout on /api/pay" }));
            } else {
                items.push(serde_json::json!({ "id": i, "status": "ok", "payload": "x".repeat(80) }));
            }
        }
        let raw = serde_json::to_string(&items).unwrap();
        let res = trim_terminal_output(&raw, "curl /api/events", 16_384, Some("log_0009"));
        assert!(res.trimmed);
        assert!(res.text.len() <= HARD_CAP_BYTES);
        assert!(res.text.contains("JSON Array (300 items)"));
        assert!(res.text.contains("timeout on /api/pay"));
    }
}
