use std::{borrow::Cow, collections::VecDeque};

use anyhow::Result;
use base64::{
    Engine,
    alphabet::STANDARD,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

const BLOCK_LIMIT: i64 = 100;
const OUTPUT_LINE_LIMIT: usize = 5_000;
const OUTPUT_UNIT_LIMIT: usize = 1024 * 1024;
const PENDING_UNIT_LIMIT: usize = 64 * 1024;
const JS_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const OSC_START: &str = "\x1b]";
const COMMAND_BASE64: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectedShellBlock {
    pub command: String,
    pub output: String,
    pub exit_code: i64,
    pub started_at: i64,
    pub completed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellHistoryBlock {
    pub id: String,
    pub session_id: String,
    pub command: String,
    pub cwd: String,
    pub output: String,
    pub exit_code: i64,
    pub started_at: i64,
    pub completed_at: i64,
}

/// Save a completed command and prune this session's older blocks in one transaction.
pub fn complete(
    connection: &mut Connection,
    id: &str,
    session_id: &str,
    cwd: &str,
    block: CollectedShellBlock,
) -> Result<ShellHistoryBlock> {
    let block = ShellHistoryBlock {
        id: id.to_owned(),
        session_id: session_id.to_owned(),
        command: block.command,
        cwd: cwd.to_owned(),
        output: retain_newest_output(&block.output),
        exit_code: block.exit_code,
        started_at: block.started_at,
        completed_at: block.completed_at,
    };
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO shell_history_blocks (
           id, session_id, command_text, cwd, output, exit_code, started_at, completed_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            block.id,
            block.session_id,
            block.command,
            block.cwd,
            block.output,
            block.exit_code,
            block.started_at,
            block.completed_at,
        ],
    )?;
    tx.execute(
        "DELETE FROM shell_history_blocks
         WHERE session_id = ? AND id NOT IN (
           SELECT id FROM shell_history_blocks
           WHERE session_id = ?
           ORDER BY completed_at DESC, id DESC
           LIMIT ?
         )",
        params![session_id, session_id, BLOCK_LIMIT],
    )?;
    tx.commit()?;
    Ok(block)
}

pub fn list(connection: &Connection, session_id: &str) -> Result<Vec<ShellHistoryBlock>> {
    let mut query = connection.prepare(
        "SELECT id, session_id, command_text, cwd, output, exit_code, started_at, completed_at
         FROM shell_history_blocks WHERE session_id = ?
         ORDER BY completed_at ASC, id ASC",
    )?;
    let rows = query.query_map([session_id], |row| {
        Ok(ShellHistoryBlock {
            id: row.get(0)?,
            session_id: row.get(1)?,
            command: row.get(2)?,
            cwd: row.get(3)?,
            output: row.get(4)?,
            exit_code: row.get(5)?,
            started_at: row.get(6)?,
            completed_at: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn list_for_launch(
    connection: &Connection,
    session_id: &str,
    enabled: bool,
) -> Result<Vec<ShellHistoryBlock>> {
    if enabled {
        list(connection, session_id)
    } else {
        Ok(Vec::new())
    }
}

pub fn purge(connection: &Connection, session_id: &str) -> Result<()> {
    connection.execute(
        "DELETE FROM shell_history_blocks WHERE session_id = ?",
        [session_id],
    )?;
    Ok(())
}

#[derive(Default)]
pub struct Collector {
    pending: Option<(String, i64)>,
    active: Option<ActiveBlock>,
}

struct ActiveBlock {
    command: String,
    output: OutputLineTail,
    started_at: i64,
}

impl Collector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume decoded terminal output without altering the input to the live terminal.
    /// Only a valid completion marker closes the active block.
    pub fn ingest(&mut self, data: &str, now: i64) -> Vec<CollectedShellBlock> {
        let (buffer, mut source_started_at) = match self.pending.take() {
            Some((mut pending, started_at)) => {
                pending.push_str(data);
                (Cow::Owned(pending), started_at)
            }
            None => (Cow::Borrowed(data), now),
        };
        let mut source = buffer.as_ref();
        let mut completed = Vec::new();
        while !source.is_empty() {
            let Some(marker_start) = source.find(OSC_START) else {
                self.append_output(source);
                break;
            };
            self.append_output(&source[..marker_start]);
            let marker = &source[marker_start..];
            let body = &marker[OSC_START.len()..];
            let Some((end, terminator_len)) = osc_end(body) else {
                let tail = utf16_suffix(body, PENDING_UNIT_LIMIT - OSC_START.len());
                self.pending = Some((
                    format!("{OSC_START}{tail}"),
                    if marker_start == 0 {
                        source_started_at
                    } else {
                        now
                    },
                ));
                break;
            };
            let body = &body[..end];
            let marker_len = OSC_START.len() + end + terminator_len;
            if let Some(command) = decode_command(body) {
                self.active = Some(ActiveBlock {
                    command,
                    output: OutputLineTail::default(),
                    started_at: source_started_at,
                });
            } else if let Some(exit_code) = decode_exit_code(body) {
                if let Some(active) = self.active.take() {
                    completed.push(CollectedShellBlock {
                        command: active.command,
                        output: active.output.finish(),
                        exit_code,
                        started_at: active.started_at,
                        completed_at: now,
                    });
                }
            } else if !is_boundary(body) {
                self.append_output(&marker[..marker_len]);
            }
            source = &marker[marker_len..];
            source_started_at = now;
        }
        completed
    }

    fn append_output(&mut self, data: &str) {
        if let Some(active) = self.active.as_mut() {
            active.output.append(data);
        }
    }
}

fn osc_end(body: &str) -> Option<(usize, usize)> {
    let bytes = body.as_bytes();
    bytes
        .iter()
        .enumerate()
        .find_map(|(index, byte)| match byte {
            b'\x07' => Some((index, 1)),
            b'\x1b' if bytes.get(index + 1) == Some(&b'\\') => Some((index, 2)),
            _ => None,
        })
}

fn decode_command(body: &str) -> Option<String> {
    let payload = body.strip_prefix("633;E;")?;
    if payload.contains('\x1b') {
        return None;
    }
    // Buffer.from(payload, 'base64') accepts both alphabets and ignores nonalphabet characters.
    let mut encoded: Vec<u8> = payload
        .bytes()
        .take_while(|&byte| byte != b'=')
        .filter_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' => Some(byte),
            b'-' => Some(b'+'),
            b'_' => Some(b'/'),
            _ => None,
        })
        .collect();
    if encoded.len() % 4 == 1 {
        encoded.pop();
    }
    let bytes = COMMAND_BASE64.decode(encoded).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn decode_exit_code(body: &str) -> Option<i64> {
    let code = body.strip_prefix("133;D;")?;
    let digits = code.strip_prefix('-').unwrap_or(code);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value: i64 = code.parse().ok()?;
    (-JS_MAX_SAFE_INTEGER..=JS_MAX_SAFE_INTEGER)
        .contains(&value)
        .then_some(value)
}

fn is_boundary(body: &str) -> bool {
    let Some(rest) = body.strip_prefix("133;") else {
        return false;
    };
    matches!(
        rest.as_bytes(),
        [b'A' | b'B' | b'C'] | [b'A' | b'B' | b'C', b';', ..]
    ) && !rest.contains('\x1b')
}

/// Retain a bounded number of UTF-16 units as output arrives.
#[derive(Default)]
struct OutputLineTail {
    lines: VecDeque<VecDeque<u16>>,
    partial: VecDeque<u16>,
    complete_line_units: usize,
}

impl OutputLineTail {
    fn append(&mut self, data: &str) {
        for segment in data.split_inclusive('\n') {
            let line = segment.strip_suffix('\n');
            let max = OUTPUT_UNIT_LIMIT - usize::from(line.is_some());
            while self.partial.len() > max {
                self.partial.pop_front();
            }
            for unit in line.unwrap_or(segment).encode_utf16() {
                if self.partial.len() == max {
                    self.partial.pop_front();
                }
                self.partial.push_back(unit);
            }
            if line.is_some() {
                self.complete_line_units += self.partial.len() + 1;
                self.lines.push_back(std::mem::take(&mut self.partial));
            }
            while (self.lines.len() > OUTPUT_LINE_LIMIT
                || self.complete_line_units + self.partial.len() > OUTPUT_UNIT_LIMIT)
                && let Some(removed) = self.lines.pop_front()
            {
                self.complete_line_units -= removed.len() + 1;
            }
        }
    }

    fn finish(mut self) -> String {
        // The current partial line counts even if the final character was a newline.
        if self.lines.len() == OUTPUT_LINE_LIMIT {
            self.lines.pop_front();
        }
        let units = self
            .lines
            .into_iter()
            .flat_map(|line| line.into_iter().chain([u16::from(b'\n')]))
            .chain(self.partial);
        char::decode_utf16(units)
            .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    }
}

/// Equivalent to a JS UTF-16 suffix at the UTF-8 boundary. A split surrogate is
/// converted to U+FFFD, as when the reference sends such text to terminal or SQLite.
fn utf16_suffix(value: &str, maximum: usize) -> Cow<'_, str> {
    let mut remaining = maximum;
    for (offset, character) in value.char_indices().rev() {
        let units = character.len_utf16();
        if units > remaining {
            let suffix = &value[offset + character.len_utf8()..];
            return if remaining == 0 {
                Cow::Borrowed(suffix)
            } else {
                Cow::Owned(format!("{}{suffix}", char::REPLACEMENT_CHARACTER))
            };
        }
        remaining -= units;
    }
    Cow::Borrowed(value)
}

fn retain_newest_output(value: &str) -> String {
    let suffix = utf16_suffix(value, OUTPUT_UNIT_LIMIT);
    match suffix.rmatch_indices('\n').nth(OUTPUT_LINE_LIMIT - 1) {
        Some((offset, _)) => suffix[offset + 1..].to_owned(),
        None => suffix.into_owned(),
    }
}

pub fn format_shell_history_for_terminal(blocks: &[ShellHistoryBlock], divider: &str) -> String {
    if blocks.is_empty() {
        return String::new();
    }
    let mut rendered = String::new();
    for (index, block) in blocks.iter().enumerate() {
        if index != 0 {
            rendered.push_str("\r\n");
        }
        rendered.push_str("❯ ");
        rendered.push_str(&block.command);
        rendered.push_str("\r\n");
        for segment in block.output.split_inclusive('\n') {
            let line = segment.strip_suffix('\n').unwrap_or(segment);
            let line = if segment.ends_with('\n') {
                line.strip_suffix('\r').unwrap_or(line)
            } else {
                line
            };
            rendered.push_str(line);
            rendered.push_str("\r\n");
        }
    }
    rendered.push_str("\r\n\x1b[2m");
    rendered.push_str(divider);
    rendered.push_str("\x1b[0m\r\n");
    rendered
}
