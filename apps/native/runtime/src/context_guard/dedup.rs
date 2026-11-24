//! Per-session duplicate-read guard, small-diff shortcut, and tiered file-read
//! size limits.
//!
//! Safety invariants:
//! - Never blocks small files or narrow slices (< 4 KB).
//! - Any blocked read can be retried within 120 seconds and will automatically
//!   pass through so legitimate patch reviews or subagents never stall.
//! - Calling `reset_session` (e.g. after `/compact`) immediately clears the
//!   session's read cache so files can be re-read freely.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

pub const MIN_DEDUP_BYTES: usize = 4_096;
pub const STANDARD_READ_CAP_BYTES: usize = 46_080; // 45 KiB
pub const NOISE_READ_CAP_BYTES: usize = 16_384; // 16 KiB
pub const REVIEW_READ_CAP_BYTES: usize = 131_072; // 128 KiB
pub const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;
pub const MAX_DIFF_BYTES: usize = 6_500;
pub const MAX_DIFF_RATIO: f64 = 0.40;
pub const RETRY_PASSTHROUGH_TTL: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileTier {
    Noise,
    Standard,
    Review,
}

impl FileTier {
    pub fn max_bytes(self) -> usize {
        match self {
            Self::Noise => NOISE_READ_CAP_BYTES,
            Self::Standard => STANDARD_READ_CAP_BYTES,
            Self::Review => REVIEW_READ_CAP_BYTES,
        }
    }
}

pub fn classify_file_tier(path: &Path) -> FileTier {
    let lower = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if lower.ends_with(".patch") || lower.ends_with(".diff") {
        return FileTier::Review;
    }
    if lower.ends_with(".log")
        || lower.ends_with(".lock")
        || lower.ends_with(".min.js")
        || lower.ends_with(".min.css")
        || lower.ends_with(".map")
        || lower == "package-lock.json"
        || lower == "pnpm-lock.yaml"
        || lower == "yarn.lock"
        || lower == "cargo.lock"
    {
        return FileTier::Noise;
    }
    FileTier::Standard
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadDecision {
    Allow,
    BlockDuplicate {
        reason: String,
        saved_bytes: usize,
    },
    ReturnDiff {
        diff: String,
        saved_bytes: usize,
    },
    BlockOversized {
        tier: FileTier,
        file_bytes: usize,
        cap_bytes: usize,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ReadKey {
    session_id: String,
    path: PathBuf,
    start_line: Option<usize>,
    end_line: Option<usize>,
}

#[derive(Clone, Debug)]
struct ReadRecord {
    sha256: [u8; 32],
    snapshot: Option<String>,
}

#[derive(Default)]
pub struct ReadDedupGuard {
    reads: HashMap<ReadKey, ReadRecord>,
    retry_confirm: HashMap<ReadKey, Instant>,
}

impl ReadDedupGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears all cached read records for `session_id` (e.g., right after `/compact`).
    pub fn reset_session(&mut self, session_id: &str) {
        self.reads.retain(|k, _| k.session_id != session_id);
        self.retry_confirm.retain(|k, _| k.session_id != session_id);
    }

    /// Evaluates whether a read of `path` (optional 1-based line range) should
    /// pass through, be deduplicated, return a compact diff, or be capped.
    pub fn evaluate_read(
        &mut self,
        session_id: &str,
        path: &Path,
        start_line: Option<usize>,
        end_line: Option<usize>,
        now: Instant,
    ) -> ReadDecision {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let Ok(meta) = fs::metadata(&canonical) else {
            return ReadDecision::Allow;
        };
        if !meta.is_file() {
            return ReadDecision::Allow;
        }
        let file_bytes = meta.len() as usize;
        let key = ReadKey {
            session_id: session_id.to_owned(),
            path: canonical.clone(),
            start_line,
            end_line,
        };

        // Second-try passthrough within 120s prevents ever trapping an agent.
        if let Some(expiry) = self.retry_confirm.get(&key).copied() {
            if now <= expiry {
                self.retry_confirm.remove(&key);
                self.record_read(&key, &canonical, start_line, end_line, file_bytes);
                return ReadDecision::Allow;
            }
            self.retry_confirm.remove(&key);
        }

        // Unbounded full-file size check against file tier.
        let is_unbounded = start_line.is_none() && end_line.is_none();
        let tier = classify_file_tier(&canonical);
        let cap = tier.max_bytes();
        if is_unbounded && file_bytes > cap {
            self.retry_confirm.insert(key, now + RETRY_PASSTHROUGH_TTL);
            let reason = format!(
                "[matou context-guard] `{}` is {} KB (tier limit {} KB). Use `mt ctx outline` or read a targeted line range; retry the same read within 120s to bypass.",
                canonical.display(),
                file_bytes / 1024,
                cap / 1024
            );
            return ReadDecision::BlockOversized {
                tier,
                file_bytes,
                cap_bytes: cap,
                reason,
            };
        }

        // Skip dedup on small files / narrow slices (< 4 KB).
        let Some(content) = read_slice_utf8(&canonical, start_line, end_line, MAX_SNAPSHOT_BYTES) else {
            return ReadDecision::Allow;
        };
        if content.len() < MIN_DEDUP_BYTES {
            return ReadDecision::Allow;
        }

        let digest = sha256_bytes(content.as_bytes());
        if let Some(prev) = self.reads.get(&key) {
            if prev.sha256 == digest {
                self.retry_confirm.insert(key, now + RETRY_PASSTHROUGH_TTL);
                let saved_bytes = content.len();
                let reason = format!(
                    "[matou context-guard] `{}` ({} KB) is unchanged since your earlier read in this session. Reuse the existing context, or retry within 120s to force a re-read.",
                    canonical.display(),
                    saved_bytes / 1024
                );
                return ReadDecision::BlockDuplicate {
                    reason,
                    saved_bytes,
                };
            }
            if let Some(old_text) = prev.snapshot.as_deref()
                && let Some(diff) = build_compact_diff(old_text, &content)
            {
                let saved_bytes = content.len().saturating_sub(diff.len());
                self.reads.insert(
                    key,
                    ReadRecord {
                        sha256: digest,
                        snapshot: Some(content),
                    },
                );
                return ReadDecision::ReturnDiff { diff, saved_bytes };
            }
        }

        self.reads.insert(
            key,
            ReadRecord {
                sha256: digest,
                snapshot: Some(content),
            },
        );
        ReadDecision::Allow
    }

    fn record_read(
        &mut self,
        key: &ReadKey,
        canonical: &Path,
        start_line: Option<usize>,
        end_line: Option<usize>,
        file_bytes: usize,
    ) {
        if file_bytes > MAX_SNAPSHOT_BYTES {
            return;
        }
        if let Some(content) = read_slice_utf8(canonical, start_line, end_line, MAX_SNAPSHOT_BYTES)
            && content.len() >= MIN_DEDUP_BYTES
        {
            self.reads.insert(
                key.clone(),
                ReadRecord {
                    sha256: sha256_bytes(content.as_bytes()),
                    snapshot: Some(content),
                },
            );
        }
    }
}

fn read_slice_utf8(
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
    max_bytes: usize,
) -> Option<String> {
    let raw = fs::read(path).ok()?;
    if raw.len() > max_bytes && start_line.is_none() && end_line.is_none() {
        return None;
    }
    let text = String::from_utf8(raw).ok()?;
    if start_line.is_none() && end_line.is_none() {
        return Some(text);
    }
    let start = start_line.unwrap_or(1).saturating_sub(1);
    let end = end_line.unwrap_or(usize::MAX);
    let sliced: Vec<&str> = text
        .lines()
        .enumerate()
        .filter(|(idx, _)| *idx >= start && *idx < end)
        .map(|(_, line)| line)
        .collect();
    Some(sliced.join("\n"))
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Builds a compact line-level diff when changes are small (< 6.5 KB and < 40%
/// of the new file). Returns `None` if a full re-read is cleaner.
fn build_compact_diff(old: &str, new: &str) -> Option<String> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let max_len = old_lines.len().max(new_lines.len());
    let mut diff_lines = Vec::new();
    for idx in 0..max_len {
        let o = old_lines.get(idx).copied();
        let n = new_lines.get(idx).copied();
        if o != n {
            let line_no = idx + 1;
            if let Some(ol) = o {
                diff_lines.push(format!("-L{line_no}: {ol}"));
            }
            if let Some(nl) = n {
                diff_lines.push(format!("+L{line_no}: {nl}"));
            }
        }
    }
    if diff_lines.is_empty() {
        return None;
    }
    let body = format!(
        "[matou context-guard] File changed slightly since last read ({} diff lines):\n{}",
        diff_lines.len(),
        diff_lines.join("\n")
    );
    if body.len() <= MAX_DIFF_BYTES && (body.len() as f64) <= (new.len() as f64 * MAX_DIFF_RATIO) {
        Some(body)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_read_blocks_once_then_passes_on_retry_and_resets_after_compact() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.rs");
        let content = "fn sample_line() { println!(\"hello world\"); }\n".repeat(150);
        fs::write(&file, &content).unwrap();

        let mut guard = ReadDedupGuard::new();
        let t0 = Instant::now();

        // First read passes.
        assert_eq!(
            guard.evaluate_read("s1", &file, None, None, t0),
            ReadDecision::Allow
        );
        // Immediate duplicate read is blocked.
        assert!(matches!(
            guard.evaluate_read("s1", &file, None, None, t0 + Duration::from_secs(5)),
            ReadDecision::BlockDuplicate { .. }
        ));
        // Second try within 120s passes through.
        assert_eq!(
            guard.evaluate_read("s1", &file, None, None, t0 + Duration::from_secs(10)),
            ReadDecision::Allow
        );
        // After /compact reset, reading again passes immediately.
        guard.reset_session("s1");
        assert_eq!(
            guard.evaluate_read("s1", &file, None, None, t0 + Duration::from_secs(20)),
            ReadDecision::Allow
        );
    }

    #[test]
    fn small_edit_returns_compact_diff_and_oversized_log_blocks_first_try() {
        let dir = tempfile::tempdir().unwrap();
        let code = dir.path().join("app.ts");
        let mut lines: Vec<String> = (1..=200)
            .map(|i| format!("export const item_{i} = 'value_{i}_padding_text';"))
            .collect();
        fs::write(&code, lines.join("\n")).unwrap();

        let mut guard = ReadDedupGuard::new();
        let t0 = Instant::now();
        assert_eq!(guard.evaluate_read("s1", &code, None, None, t0), ReadDecision::Allow);

        // Modify a single line.
        lines[41] = "export const item_42 = 'UPDATED_VALUE';".to_owned();
        fs::write(&code, lines.join("\n")).unwrap();
        match guard.evaluate_read("s1", &code, None, None, t0 + Duration::from_secs(2)) {
            ReadDecision::ReturnDiff { diff, saved_bytes } => {
                assert!(diff.contains("UPDATED_VALUE"));
                assert!(saved_bytes > 4_000);
            }
            other => panic!("expected ReturnDiff, got {other:?}"),
        }

        // Oversized .log (> 16 KB) blocks on first try, passes on second.
        let log = dir.path().join("server.log");
        fs::write(&log, "INFO line\n".repeat(3_000)).unwrap();
        assert!(matches!(
            guard.evaluate_read("s1", &log, None, None, t0),
            ReadDecision::BlockOversized { tier: FileTier::Noise, .. }
        ));
        assert_eq!(
            guard.evaluate_read("s1", &log, None, None, t0 + Duration::from_secs(5)),
            ReadDecision::Allow
        );
    }
}
