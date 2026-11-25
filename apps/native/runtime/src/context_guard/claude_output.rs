//! The plain-text Bash and Read shapes supported by Claude Code's PostToolUse hook.

use std::{
    borrow::Cow,
    fs::{self, OpenOptions},
    io::Read,
    path::Path,
    time::Instant,
};

use serde_json::{Value, json};

use super::filter::{HARD_CAP_BYTES, summarize_tool_output};

// Leave space for Claude's system reminder following the tool text.
const MODEL_TEXT_BUDGET: usize = HARD_CAP_BYTES - 800;
const PREVIEW_UNITS: usize = 2_000;

pub(super) struct ClaudeOutput<'a> {
    pub body: Cow<'a, str>,
    pub command: &'a str,
    pub reported_bytes: usize,
    pub model_bytes: usize,
    pub partial: bool,
    read_file: Option<(&'a str, usize, usize)>,
}

impl<'a> ClaudeOutput<'a> {
    pub fn parse(
        payload: &'a Value,
        tool: &str,
        threshold: usize,
        deadline: Instant,
    ) -> Option<Self> {
        let result = payload.get("tool_response")?.as_object()?;
        let input = payload.get("tool_input")?.as_object()?;
        match tool {
            "Bash" => {
                if result.get("isImage")?.as_bool()?
                    || result.get("interrupted")?.as_bool()?
                    || input
                        .get("run_in_background")
                        .is_some_and(|v| v.as_bool() != Some(false))
                    || result
                        .get("noOutputExpected")
                        .is_some_and(|v| v.as_bool() != Some(false))
                    || result.keys().any(|key| {
                        !matches!(
                            key.as_str(),
                            "stdout"
                                | "stderr"
                                | "isImage"
                                | "interrupted"
                                | "noOutputExpected"
                                | "persistedOutputPath"
                                | "persistedOutputSize"
                        )
                    })
                {
                    return None;
                }
                let stdout = result.get("stdout")?.as_str()?;
                let stderr = result.get("stderr")?.as_str()?;
                if !is_text(stdout) || !is_text(stderr) {
                    return None;
                }
                let command = input.get("command")?.as_str()?;
                let (body, reported_bytes, model_bytes, partial) = match (
                    result.get("persistedOutputPath"),
                    result.get("persistedOutputSize"),
                ) {
                    (Some(path), Some(size)) if stderr.is_empty() => {
                        let path = path.as_str()?;
                        let size = usize::try_from(size.as_u64()?).ok()?;
                        if size <= threshold
                            || size <= stdout.len()
                            || path.contains(['\n', '\r', '\0'])
                        {
                            return None;
                        }
                        let model_bytes = persisted_bash_model_bytes(stdout, path, size);
                        let full =
                            read_persisted_output(payload, Path::new(path), size, stdout, deadline);
                        let partial = full.is_none();
                        (
                            full.map_or(Cow::Borrowed(stdout), Cow::Owned),
                            size,
                            model_bytes,
                            partial,
                        )
                    }
                    (None, None) => {
                        let body = if stdout.is_empty() {
                            Cow::Borrowed(stderr)
                        } else if stderr.is_empty() {
                            Cow::Borrowed(stdout)
                        } else {
                            Cow::Owned(format!("{stdout}\n{stderr}"))
                        };
                        let raw_bytes = body.len();
                        (body, raw_bytes, bash_model_bytes(stdout, stderr), false)
                    }
                    _ => return None,
                };
                Some(Self {
                    body,
                    command,
                    reported_bytes,
                    model_bytes,
                    partial,
                    read_file: None,
                })
            }
            "Read" => {
                if result.len() != 2 || result.get("type")?.as_str()? != "text" {
                    return None;
                }
                let file = result.get("file")?.as_object()?;
                if file.len() != 5 {
                    return None;
                }
                let path = file.get("filePath")?.as_str()?;
                let content = file.get("content")?.as_str()?;
                let start_line = usize::try_from(file.get("startLine")?.as_u64()?).ok()?;
                let num_lines = usize::try_from(file.get("numLines")?.as_u64()?).ok()?;
                let total_lines = usize::try_from(file.get("totalLines")?.as_u64()?).ok()?;
                if !is_text(content)
                    || path.is_empty()
                    || start_line == 0
                    || num_lines == 0
                    || content.split('\n').count() != num_lines
                    || start_line.checked_add(num_lines - 1)? > total_lines
                {
                    return None;
                }
                Some(Self {
                    body: Cow::Borrowed(content),
                    command: path,
                    reported_bytes: content.len(),
                    model_bytes: read_model_bytes(content, start_line)?,
                    partial: false,
                    read_file: Some((path, start_line, total_lines)),
                })
            }
            _ => None,
        }
    }

    pub fn replacement(
        &self,
        cleaned: &str,
        command: &str,
        source_id: &str,
    ) -> Option<(Value, usize)> {
        let model_budget = MODEL_TEXT_BUDGET.min(self.model_bytes.checked_sub(1)?);
        let mut budget = model_budget;
        loop {
            let text = summarize_tool_output(cleaned, command, budget, source_id, self.partial)?;
            if let Some((path, start_line, total_lines)) = self.read_file {
                let rendered = read_model_bytes(&text, start_line)?;
                if rendered > model_budget {
                    budget = budget.checked_sub(rendered - model_budget)?;
                    continue;
                }
                return Some((
                    json!({"type": "text", "file": {
                        "filePath": path, "content": text, "startLine": start_line,
                        "numLines": text.split('\n').count(), "totalLines": total_lines
                    }}),
                    self.model_bytes - rendered,
                ));
            }
            let rendered = bash_model_bytes(&text, "");
            return Some((
                json!({"stdout": text, "stderr": "", "interrupted": false, "isImage": false}),
                self.model_bytes - rendered,
            ));
        }
    }
}

fn is_text(text: &str) -> bool {
    !text
        .bytes()
        .any(|byte| matches!(byte, 0..=7 | 14..=26 | 28..=31))
}

fn js_whitespace(ch: char) -> bool {
    ch == '\u{feff}' || (ch.is_whitespace() && ch != '\u{85}')
}

fn bash_stdout(stdout: &str) -> &str {
    let leading = &stdout[..stdout.len() - stdout.trim_start_matches(js_whitespace).len()];
    let start = leading.rfind('\n').map_or(0, |index| index + 1);
    stdout[start..].trim_end_matches(js_whitespace)
}

fn bash_model_bytes(stdout: &str, stderr: &str) -> usize {
    let stdout = bash_stdout(stdout);
    let stderr = stderr.trim_matches(js_whitespace);
    let separator = usize::from(!stdout.is_empty() && !stderr.is_empty());
    (stdout.len() + stderr.len() + separator)
        .saturating_sub(stdout.len() - stdout.trim_start_matches(js_whitespace).len())
}

fn read_model_bytes(content: &str, start_line: usize) -> Option<usize> {
    let mut total = 0;
    let mut trailing = 0;
    for (index, line) in content.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let number = start_line.checked_add(index)?;
        total += number.ilog10() as usize + 2 + line.len() + usize::from(index > 0);
        trailing = line.len() - line.trim_end_matches(js_whitespace).len()
            + usize::from(line.trim_end_matches(js_whitespace).is_empty());
    }
    Some(total - trailing)
}

fn persisted_bash_model_bytes(stdout: &str, path: &str, size: usize) -> usize {
    // Claude 2.1.332 renders a preview of 2,000 UTF-16 units, ending at a
    // newline when that newline is in the latter half of the preview.
    let units: Vec<u16> = bash_stdout(stdout)
        .encode_utf16()
        .take(PREVIEW_UNITS + 1)
        .collect();
    let has_more = units.len() > PREVIEW_UNITS;
    let mut end = units.len().min(PREVIEW_UNITS);
    if has_more {
        if let Some(newline) = units[..end]
            .iter()
            .rposition(|unit| *unit == u16::from(b'\n'))
        {
            if newline > PREVIEW_UNITS / 2 {
                end = newline;
            }
        }
        if matches!(units[end - 1], 0xd800..=0xdbff) {
            end -= 1;
        }
    }
    let preview = String::from_utf16_lossy(&units[..end]);
    let (value, unit) = if size < 1024 * 1024 {
        (size as f64 / 1024.0, "KB")
    } else {
        (size as f64 / (1024.0 * 1024.0), "MB")
    };
    let mut value = format!("{value:.*}", usize::from(value < 100.0));
    if value.ends_with(".0") {
        value.truncate(value.len() - 2);
    }
    let overflow = if has_more { "\n..." } else { "" };
    format!("<persisted-output>\nOutput too large ({value}{unit}). Full output saved to: {path}\n\nPreview (first 2KB):\n{preview}{overflow}\n</persisted-output>").len()
}

fn read_persisted_output(
    payload: &Value,
    path: &Path,
    size: usize,
    stdout: &str,
    deadline: Instant,
) -> Option<String> {
    let transcript = Path::new(payload.get("transcript_path")?.as_str()?);
    let session = payload.get("session_id")?.as_str()?;
    uuid::Uuid::parse_str(session).ok()?;
    let project = transcript.parent()?;
    let name = path.file_name()?.to_str()?;
    if !transcript.is_absolute()
        || transcript.file_name()?.to_str()? != format!("{session}.jsonl")
        || project.parent()?.file_name()? != "projects"
        || !name
            .strip_suffix(".txt")?
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || path != project.join(session).join("tool-results").join(name)
        || Instant::now() >= deadline
    {
        return None;
    }
    // Resolve the project root first, but refuse links anywhere below it.
    let canonical = fs::canonicalize(path).ok()?;
    if canonical
        != fs::canonicalize(project)
            .ok()?
            .join(session)
            .join("tool-results")
            .join(name)
        || !fs::symlink_metadata(path).ok()?.is_file()
    {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(canonical).ok()?;
    let metadata = file.metadata().ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return None;
        }
    }
    if !metadata.is_file() || metadata.len() != size as u64 {
        return None;
    }
    let mut content = Vec::new();
    let mut buffer = [0; 32 * 1024];
    while Instant::now() < deadline {
        let bytes = file.read(&mut buffer).ok()?;
        if bytes == 0 {
            break;
        }
        if content.len() + bytes > size {
            return None;
        }
        content.extend_from_slice(&buffer[..bytes]);
    }
    let full = String::from_utf8(content).ok()?;
    // The host can cut the final UTF-8 character of the inline prefix.
    let prefix = stdout.trim_end_matches('\u{fffd}');
    (!prefix.is_empty() && full.len() == size && full.starts_with(prefix) && is_text(&full))
        .then_some(full)
}
