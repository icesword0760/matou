//! Build a compact, readable scrollback backfill from a Claude Code JSONL transcript.
//!
//! When a Claude Code session previously ran in fullscreen / alternate-screen TUI mode,
//! its primary terminal scrollback buffer is nearly empty on restart. Backfilling from
//! the session's durable JSONL transcript restores pre-compaction and recent turns into
//! primary scrollback once, before the new process draws its startup screen.

use std::{
    collections::VecDeque,
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
};

use serde_json::Value;

/// Only scan the tail of very large transcripts so session startup stays fast.
const MAX_READ_BYTES: u64 = 32 * 1024 * 1024;
/// Keep backfilled history well below the 10,000-line terminal scrollback limit,
/// leaving plenty of headroom for live conversation output.
const MAX_BACKFILL_LINES: usize = 4_000;
const MAX_BACKFILL_BYTES: usize = 256 * 1024;
/// Trigger backfill only when the recovered primary scrollback has almost no history.
pub(crate) const MIN_EXISTING_HISTORY_LINES: usize = 20;

const HEADER: &str = "\x1b[2m─── 已从会话记录恢复历史对话（含压缩前内容） ───\x1b[0m";
const FOOTER: &str = "\x1b[2m─── 以上为恢复的历史对话 ───\x1b[0m";
const COMPACT_DIVIDER: &str = "\x1b[2m─── 对话在此处进行过压缩（compact） ───\x1b[0m";

pub(crate) fn format_claude_transcript(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    if file_len == 0 {
        return None;
    }

    let skipped_prefix = file_len > MAX_READ_BYTES;
    if skipped_prefix {
        file.seek(SeekFrom::End(-(MAX_READ_BYTES as i64))).ok()?;
    }

    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut raw = Vec::with_capacity(4096);
    if skipped_prefix {
        // Discard the partial first line after seeking into the middle of the file.
        let _ = reader.read_until(b'\n', &mut raw);
        raw.clear();
    }

    let mut lines: VecDeque<String> = VecDeque::new();
    let mut total_bytes: usize = 0;

    let push_line = |lines: &mut VecDeque<String>, total_bytes: &mut usize, line: String| {
        let len = line.len() + 2; // account for \r\n
        *total_bytes += len;
        lines.push_back(line);
        while lines.len() > MAX_BACKFILL_LINES || *total_bytes > MAX_BACKFILL_BYTES {
            if let Some(dropped) = lines.pop_front() {
                *total_bytes = total_bytes.saturating_sub(dropped.len() + 2);
            } else {
                break;
            }
        }
    };

    loop {
        raw.clear();
        let read = match reader.read_until(b'\n', &mut raw) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        if read == 0 {
            break;
        }
        // Fast byte-level prefilter to skip bulky tool/progress/file-history records.
        if !contains_any_marker(&raw) {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&raw) else {
            continue;
        };

        if value.get("subtype").and_then(Value::as_str) == Some("compact_boundary") {
            push_line(&mut lines, &mut total_bytes, String::new());
            push_line(&mut lines, &mut total_bytes, COMPACT_DIVIDER.to_owned());
            push_line(&mut lines, &mut total_bytes, String::new());
            continue;
        }

        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            continue;
        };
        if kind != "user" && kind != "assistant" {
            continue;
        }
        if value.get("isMeta").and_then(Value::as_bool) == Some(true)
            || value
                .get("isCompactSummary")
                .and_then(Value::as_bool)
                == Some(true)
        {
            continue;
        }

        let Some(content) = value.get("message").and_then(|msg| msg.get("content")) else {
            continue;
        };
        let texts = extract_text_blocks(content);
        if texts.is_empty() {
            continue;
        }

        let prefix = if kind == "user" {
            "\x1b[1;34m❯\x1b[0m "
        } else {
            "\x1b[1;32m●\x1b[0m "
        };

        let mut first = true;
        for block in texts {
            for raw_line in block.lines() {
                let formatted = if first {
                    first = false;
                    format!("{prefix}{raw_line}")
                } else {
                    format!("  {raw_line}")
                };
                push_line(&mut lines, &mut total_bytes, formatted);
            }
        }
        push_line(&mut lines, &mut total_bytes, String::new());
    }

    if lines.is_empty() {
        return None;
    }

    let mut output = Vec::with_capacity(total_bytes + HEADER.len() + FOOTER.len() + 16);
    output.extend_from_slice(b"\r\n");
    output.extend_from_slice(HEADER.as_bytes());
    output.extend_from_slice(b"\r\n\r\n");
    for line in lines {
        output.extend_from_slice(line.as_bytes());
        output.extend_from_slice(b"\r\n");
    }
    output.extend_from_slice(FOOTER.as_bytes());
    output.extend_from_slice(b"\r\n");
    Some(output)
}

fn contains_any_marker(haystack: &[u8]) -> bool {
    contains_subslice(haystack, b"\"type\":\"user\"")
        || contains_subslice(haystack, b"\"type\": \"user\"")
        || contains_subslice(haystack, b"\"type\":\"assistant\"")
        || contains_subslice(haystack, b"\"type\": \"assistant\"")
        || contains_subslice(haystack, b"\"compact_boundary\"")
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

fn extract_text_blocks(content: &Value) -> Vec<&str> {
    let mut blocks = Vec::new();
    match content {
        Value::String(text) => {
            let trimmed = text.trim();
            if !trimmed.is_empty() && !is_internal_tag(trimmed) {
                blocks.push(trimmed);
            }
        }
        Value::Array(items) => {
            for item in items {
                if item.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && !is_internal_tag(trimmed) {
                        blocks.push(trimmed);
                    }
                }
            }
        }
        _ => {}
    }
    blocks
}

fn is_internal_tag(text: &str) -> bool {
    text.starts_with("<system-reminder>")
        || text.starts_with("<command-name>")
        || text.starts_with("<command-message>")
        || text.starts_with("<local-command-")
        || text.starts_with("<task-notification>")
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn formats_user_assistant_and_compact_boundary_while_skipping_meta_and_tools() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type":"user","message":{{"role":"user","content":"第一轮提问"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"thinking","thinking":"secret"}},{{"type":"tool_use","name":"Read"}},{{"type":"text","text":"第一轮回答\n第二行"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"system","subtype":"compact_boundary"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"user","isCompactSummary":true,"message":{{"role":"user","content":"hidden summary"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"user","isMeta":true,"message":{{"role":"user","content":"hidden meta"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"user","message":{{"role":"user","content":"<system-reminder>internal</system-reminder>"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"压缩后的回复"}}]}}}}"#
        )
        .unwrap();

        let rendered = String::from_utf8(format_claude_transcript(file.path()).unwrap()).unwrap();
        assert!(rendered.contains("第一轮提问"));
        assert!(rendered.contains("第一轮回答"));
        assert!(rendered.contains("  第二行"));
        assert!(rendered.contains("对话在此处进行过压缩"));
        assert!(rendered.contains("压缩后的回复"));
        assert!(!rendered.contains("hidden summary"));
        assert!(!rendered.contains("hidden meta"));
        assert!(!rendered.contains("system-reminder"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn returns_none_for_empty_or_non_dialogue_transcript() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, r#"{{"type":"progress","data":"ignored"}}"#).unwrap();
        assert!(format_claude_transcript(file.path()).is_none());
    }
}
