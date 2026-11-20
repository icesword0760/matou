use std::{borrow::Cow, sync::LazyLock};

use percent_encoding::percent_decode_str;
use regex::{Regex, RegexSet};
use url::Url;

const WORK_TAIL_UNITS: usize = 512;
const CWD_TAIL_UNITS: usize = 4096;
const JS_SPACE: &str = r"[\x09-\x0D\x20\u{00A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]";

static OSC_133: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\x1b\]133;([CD])(?:;(-?[0-9]+))?(?:\x07|\x1b\\)"));
static OSC_7: LazyLock<Regex> = LazyLock::new(|| regex(r"\x1b\]7;([^\x07\x1b]*)(?:\x07|\x1b\\)"));
static OSC_DISPLAY: LazyLock<Regex> = LazyLock::new(|| regex(r"\x1b\][^\x07]*(?:\x07|\x1b\\)"));
static CSI_DISPLAY: LazyLock<Regex> = LazyLock::new(|| regex(r"\x1b\[[0-?]*[ -/]*[@-~]"));
static CHARSET_DISPLAY: LazyLock<Regex> = LazyLock::new(|| regex(r"\x1b[()][0-2A-Z]"));
static HIDDEN_DISPLAY: LazyLock<Regex> =
    LazyLock::new(|| regex(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]"));

static BLOCKING_PROMPT: LazyLock<RegexSet> = LazyLock::new(|| {
    regex_set([
        r"(?:^|\s)(?i-u:enter|input|type|provide)\s+[^:：?？\r\n]{1,64}[:：?？]$",
        r"(?:^|\s)(?i-u:password|passphrase|pin|token)\s*[:：?？]$",
        r"(?:^|\s)(?:请输入|输入|密码|口令|请选择)[^:：?？\r\n]{0,64}[:：?？]$",
        r"^\s*(?i-u:enter|input|select|choose)(?-u:\b)[^:：?？\r\n]{0,64}[:：]$",
        r"(?:^|\s)(?i-u:password|passphrase)(?-u:\b)[^:：?？\r\n]{0,64}[:：?？]$",
        r"(?:\[[yYnN](?:/[yYnN])?\]|\([yYnN](?:/[yYnN])?\))$",
    ])
});
static LABELED_CHEVRON: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?:^|\s)[\p{L}\p{N}_][\p{L}\p{N} _.:/@+\-]{1,63}>$"));
static SUMMARY_CHROME: LazyLock<RegexSet> = LazyLock::new(|| {
    regex_set([
        r"^[%$#>]$",
        r"^(?:\([^)]*\)\s*)?➜\s+\S+(?:\s+git:\([^)]+\)\s*\S*)?$",
        r"^[^\s@]+@[^\s]+\s+[^\r\n\u{2028}\u{2029}]+\s+[%$#>]$",
        r"^[❯›»]$",
        r"^[─━═╌╍┄┅┈┉\s]+$",
        r"^[⏸⏵▶▷]\s*[^\r\n\u{2028}\u{2029}]*(?-u:\b)(?i-u:manual mode on|for agents)(?-u:\b)",
    ])
});

fn js_space_patterns(pattern: &str) -> String {
    pattern
        .replace(r"\s", JS_SPACE)
        .replace(r"\S", &format!("[^{JS_SPACE}]"))
}

fn regex(pattern: &str) -> Regex {
    Regex::new(&js_space_patterns(pattern)).expect("terminal matcher")
}

fn regex_set<const N: usize>(patterns: [&str; N]) -> RegexSet {
    RegexSet::new(patterns.map(js_space_patterns)).expect("terminal matchers")
}

fn js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'..='\u{000d}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

#[derive(Default)]
pub struct WorkStatusTracker {
    buffer: String,
}

impl WorkStatusTracker {
    pub fn ingest(&mut self, chunk: &str) -> Vec<String> {
        self.buffer.push_str(chunk);
        let mut consumed = 0;
        let mut statuses = Vec::new();
        for frame in OSC_133.captures_iter(&self.buffer) {
            consumed = frame.get(0).unwrap().end();
            let status = if &frame[1] == "C" {
                "running"
            } else {
                let code = frame.get(2).map_or("0", |value| value.as_str());
                match code.parse::<f64>() {
                    Ok(0.0) => "idle",
                    Ok(130.0) => "interrupted",
                    _ => "error",
                }
            };
            statuses.push(status.to_owned());
        }
        self.buffer = utf16_tail(&self.buffer[consumed..], WORK_TAIL_UNITS);
        let completed = matches!(
            statuses.last().map(String::as_str),
            Some("idle" | "interrupted" | "error")
        );
        if !completed && is_blocking_prompt(&self.buffer) {
            statuses.push("needs-input".to_owned());
        }
        statuses
    }
}

fn is_blocking_prompt(raw: &str) -> bool {
    let without_osc = OSC_DISPLAY.replace_all(raw, "");
    let without_csi = CSI_DISPLAY.replace_all(&without_osc, "");
    let visible = HIDDEN_DISPLAY.replace_all(&without_csi, "");
    let line = visible
        .rsplit(['\r', '\n'])
        .next()
        .unwrap_or_default()
        .trim_end_matches(js_whitespace);
    LABELED_CHEVRON.is_match(line) || BLOCKING_PROMPT.is_match(&utf16_prompt_text(line))
}

// Non-Unicode reference prompt patterns count UTF-16 code units. Astral
// characters can only match their free-form text, so two non-ASCII stand-ins
// preserve their length and boundary behavior for those patterns.
fn utf16_prompt_text(line: &str) -> Cow<'_, str> {
    if !line.chars().any(|character| character.len_utf16() == 2) {
        return Cow::Borrowed(line);
    }
    let mut text = String::with_capacity(line.len());
    for character in line.chars() {
        if character.len_utf16() == 2 {
            text.push_str("\u{e000}\u{e000}");
        } else {
            text.push(character);
        }
    }
    Cow::Owned(text)
}

#[derive(Default)]
pub struct CwdTracker {
    buffer: String,
}

impl CwdTracker {
    pub fn ingest(&mut self, chunk: &str) -> Option<String> {
        self.buffer.push_str(chunk);
        self.buffer = utf16_tail(&self.buffer, CWD_TAIL_UNITS);

        let cwd = OSC_7
            .captures_iter(&self.buffer)
            .filter_map(|frame| file_url_path(&frame[1]))
            .last();
        let last_terminator = self.buffer.rfind('\x07').max(self.buffer.rfind("\x1b\\"));
        if let Some(terminator) = last_terminator {
            // The reference drops the first byte of the terminator, leaving
            // the backslash of ESC-backslash at the start of the remainder.
            self.buffer.drain(..terminator + 1);
        }
        cwd
    }
}

fn file_url_path(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let path = url.path();
    for (offset, _) in path.match_indices('%') {
        let digits = path.as_bytes().get(offset + 1..offset + 3)?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
    }
    let mut decoded = percent_decode_str(path).decode_utf8().ok()?.into_owned();
    if matches!(path.as_bytes(), [b'/', drive, b'|', rest @ ..]
        if drive.is_ascii_alphabetic() && (rest.is_empty() || rest[0] == b'/'))
    {
        decoded.replace_range(2..3, ":");
    }
    Some(decoded)
}

pub fn terminal_summary_lines(raw: &str) -> Vec<String> {
    let without_osc = OSC_DISPLAY.replace_all(raw, "");
    let without_charset = CHARSET_DISPLAY.replace_all(&without_osc, "");
    let without_csi = CSI_DISPLAY.replace_all(&without_charset, "");
    let line_breaks = without_csi.replace('\r', "\n");
    let visible = HIDDEN_DISPLAY.replace_all(&line_breaks, "");
    let mut lines = visible
        .rsplit('\n')
        .map(|line| line.trim_end_matches(js_whitespace))
        .filter(|line| {
            let trimmed = line.trim_start_matches(js_whitespace);
            !trimmed.is_empty() && !SUMMARY_CHROME.is_match(trimmed)
        })
        .take(4)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines.reverse();
    lines
}

pub fn utf16_tail(value: &str, mut remaining: usize) -> String {
    for (offset, character) in value.char_indices().rev() {
        let units = character.len_utf16();
        if units > remaining {
            let suffix = &value[offset + character.len_utf8()..];
            return if remaining == 0 {
                suffix.to_owned()
            } else {
                format!("{}{suffix}", char::REPLACEMENT_CHARACTER)
            };
        }
        remaining -= units;
    }
    value.to_owned()
}
