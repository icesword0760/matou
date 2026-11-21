use super::*;

use crate::{
    history::{decode_output, read_first_valid, segment_groups},
    owner::ShellOwner,
};
use encoding_rs::UTF_8;
use matou_native_core::journal::{Frame, MAX_SAFE_INTEGER, Record};
use matou_native_terminal::{Screen, Terminal, TerminalOptions};

pub(super) enum ReadKind {
    Current(TextLimitFields),
    History(TextLimitFields, Option<Value>, Option<Value>),
    Commands(Value),
}

pub(super) struct TextLimitFields {
    lines: Value,
    bytes: Value,
}

#[derive(Clone, Copy)]
pub(super) struct TextLimits {
    lines: usize,
    bytes: usize,
}

pub(super) enum TerminalRead {
    Current(TextLimits),
    History(TextLimits, u64, Option<u64>),
    Commands(usize),
}

impl ReadKind {
    pub(super) fn parse(method: &str, params: &Map<String, Value>) -> WireResult<Self> {
        let supported = match method {
            "terminal.read-current" => &["target", "maxLines", "maxBytes"][..],
            "terminal.read-history" => &[
                "target",
                "maxLines",
                "maxBytes",
                "fromSequence",
                "throughSequence",
            ],
            "terminal.read-commands" => &["target", "limit"],
            _ => unreachable!("terminal read method"),
        };
        if params.keys().any(|key| !supported.contains(&key.as_str())) {
            return Err(Fault::new(INVALID, "params contains an unsupported field"));
        }
        let field = |name| params.get(name).cloned();
        if method == "terminal.read-commands" {
            return Ok(Self::Commands(field("limit").unwrap_or_default()));
        }
        let limits = TextLimitFields {
            lines: field("maxLines").unwrap_or_default(),
            bytes: field("maxBytes").unwrap_or_default(),
        };
        Ok(if method == "terminal.read-current" {
            Self::Current(limits)
        } else {
            Self::History(limits, field("fromSequence"), field("throughSequence"))
        })
    }

    pub(super) fn validate(&self) -> WireResult<TerminalRead> {
        match self {
            Self::Current(limits) => Ok(TerminalRead::Current(limits.validate()?)),
            Self::History(limits, from, through) => {
                let limits = limits.validate()?;
                let from = from
                    .as_ref()
                    .map(|value| sequence(value, "fromSequence"))
                    .transpose()?
                    .unwrap_or(0);
                let through = through
                    .as_ref()
                    .map(|value| sequence(value, "throughSequence"))
                    .transpose()?;
                if through.is_some_and(|through| through < from) {
                    return Err(Fault::new(
                        INVALID,
                        "throughSequence must be at least fromSequence",
                    ));
                }
                Ok(TerminalRead::History(limits, from, through))
            }
            Self::Commands(limit) => Ok(TerminalRead::Commands(limit_field(limit, "limit", 1000)?)),
        }
    }
}

impl TextLimitFields {
    fn validate(&self) -> WireResult<TextLimits> {
        Ok(TextLimits {
            lines: limit_field(&self.lines, "maxLines", 5000)?,
            bytes: limit_field(&self.bytes, "maxBytes", 1024 * 1024)?,
        })
    }
}

fn limit_field(value: &Value, field: &str, max: u64) -> WireResult<usize> {
    integer(value, max)
        .map(|n| n as usize)
        .ok_or_else(|| Fault::new(INVALID, format!("{field} must be between 1 and {max}")))
}

fn sequence(value: &Value, field: &str) -> WireResult<u64> {
    if value.as_f64() == Some(0.0) {
        return Ok(0);
    }
    integer(value, MAX_SAFE_INTEGER)
        .ok_or_else(|| Fault::new(INVALID, format!("{field} must be a non-negative integer")))
}

pub(super) enum Reply {
    Ready(Vec<u8>),
    Screen(Receiver<Result<Screen>>, PathBuf, String, TextLimits),
    Replay(PathBuf, String, TextLimits),
    History(PathBuf, String, TextLimits, u64, Option<u64>),
}

pub(super) fn prepare(
    id: &str,
    root: &Path,
    reader: &Connection,
    owner: &ShellOwner,
    session_id: &str,
    read: TerminalRead,
) -> WireResult<Reply> {
    match read {
        TerminalRead::Current(limits) => match owner.read_current_screen(session_id) {
            Ok(Some(result)) => Ok(Reply::Screen(
                result,
                root.into(),
                session_id.into(),
                limits,
            )),
            // A worker can exit between topology resolution and the screen request.
            Ok(None) | Err(_) => Ok(Reply::Replay(root.into(), session_id.into(), limits)),
        },
        TerminalRead::History(limits, from, through) => Ok(Reply::History(
            root.into(),
            session_id.into(),
            limits,
            from,
            through,
        )),
        TerminalRead::Commands(limit) => {
            let boundaries = commands(reader, session_id, limit).map_err(|_| {
                Fault::new(
                    "INTERNAL_ERROR",
                    "could not read terminal command boundaries",
                )
            })?;
            serialize_ok(id, boundaries).map(Reply::Ready)
        }
    }
}

impl Reply {
    pub(super) fn complete(
        self,
        request: &Envelope<'_>,
        state: &ServerState,
    ) -> WireResult<Vec<u8>> {
        let check = || {
            if state.stopping() {
                Err(Fault::new(TIMEOUT, "host control is stopping"))
            } else if (state.clock)() > request.deadline {
                Err(deadline_elapsed())
            } else {
                Ok(())
            }
        };
        check()?;
        let result = match self {
            Self::Ready(body) => return Ok(body),
            Self::Screen(result, root, session, limits) => {
                let screen = loop {
                    match result.recv_timeout(IO_POLL) {
                        Ok(result) => break result.map_err(|_| screen_fault())?,
                        Err(mpsc::RecvTimeoutError::Timeout) => check()?,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break replay(&root, &session, check)?;
                        }
                    }
                };
                current(request.id, screen, limits)
            }
            Self::Replay(root, session, limits) => {
                current(request.id, replay(&root, &session, check)?, limits)
            }
            Self::History(root, session, limits, from, through) => {
                history(request.id, &root, &session, limits, from, through, check)
            }
        };
        check()?;
        result
    }
}

fn screen_fault() -> Fault {
    Fault::new("INTERNAL_ERROR", "could not read terminal screen")
}

// Scan existing segment files only. The indexed replay API also refreshes its
// sidecar, which would make a control read change the journal directory.
fn records(
    root: &Path,
    session: &str,
    check: impl Fn() -> WireResult<()>,
    mut visit: impl FnMut(Record) -> WireResult<bool>,
) -> WireResult<()> {
    use std::path::Component;

    let mut parts = Path::new(session).components();
    let fault = || Fault::new("INTERNAL_ERROR", "could not read terminal journal");
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
        return Err(fault());
    }
    let directory = root.join("journal").join(session);
    let groups = match segment_groups(&directory) {
        Ok(groups) => groups,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(fault()),
    };
    let mut previous = 0;
    for (index, paths) in groups {
        check()?;
        let batch = read_first_valid(&paths)
            .or_else(|original| {
                // Compression can remove the raw file after the directory listing.
                let Some(paths) = segment_groups(&directory)
                    .ok()
                    .and_then(|mut groups| groups.remove(&index))
                else {
                    return Err(original);
                };
                read_first_valid(&paths)
            })
            .map_err(|_| fault())?;
        for record in batch {
            check()?;
            let sequence = record.frame.sequence();
            if sequence <= previous {
                return Err(Fault::new(
                    "INTERNAL_ERROR",
                    "non-monotonic sequence across journal segments",
                ));
            }
            previous = sequence;
            if !visit(record)? {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn replay(root: &Path, session: &str, check: impl Fn() -> WireResult<()>) -> WireResult<Screen> {
    let mut options = TerminalOptions::new(80, 24, 7, 14);
    let mut terminal = Terminal::new(options).map_err(|_| screen_fault())?;
    let mut decoder = UTF_8.new_decoder_with_bom_removal();
    let mut text = String::new();
    records(root, session, check, |record| {
        match record.frame {
            Frame::Output { data, .. } => {
                text.clear();
                decode_output(&mut decoder, &data, &mut text, false);
                terminal.feed(text.as_bytes());
                terminal.drain_responses();
                terminal.drain_clipboard();
            }
            Frame::Resize { cols, rows, .. } => {
                options.cols = cols.try_into().map_err(|_| screen_fault())?;
                options.rows = rows.try_into().map_err(|_| screen_fault())?;
                terminal
                    .resize(
                        options.cols,
                        options.rows,
                        options.cell_width_px,
                        options.cell_height_px,
                    )
                    .map_err(|_| screen_fault())?;
            }
            Frame::Reset { .. } => terminal = Terminal::new(options).map_err(|_| screen_fault())?,
            _ => {}
        }
        Ok(true)
    })?;
    text.clear();
    decode_output(&mut decoder, &[], &mut text, true);
    terminal.feed(text.as_bytes());
    terminal.screen().map_err(|_| screen_fault())
}

fn tail_start(text: &str, limits: TextLimits) -> usize {
    let mut start = text.len().saturating_sub(limits.bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.as_bytes()[start..]
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(limits.lines - 1)
        .map_or(start, |(position, _)| start + position + 1)
}

fn current(id: &str, screen: Screen, limits: TextLimits) -> WireResult<Vec<u8>> {
    #[derive(Serialize)]
    struct Current<'a> {
        text: &'a str,
        cols: usize,
        rows: usize,
        truncated: bool,
        source: &'static str,
    }
    let mut text = String::new();
    for row in screen.cells.chunks(screen.cols) {
        let Some(last) = row.iter().rposition(|cell| cell.has_content) else {
            text.push('\n');
            continue;
        };
        for cell in &row[..=last] {
            if cell.width != 0 {
                text.push_str(if cell.has_content { &cell.text } else { " " });
            }
        }
        text.push('\n');
    }
    let text = text.trim_end_matches('\n');
    let start = tail_start(text, limits);
    serialize_ok(
        id,
        Current {
            text: &text[start..],
            cols: screen.cols,
            rows: screen.rows,
            truncated: start != 0,
            source: "screen",
        },
    )
}

fn history(
    id: &str,
    root: &Path,
    session: &str,
    limits: TextLimits,
    from: u64,
    through: Option<u64>,
    check: impl Fn() -> WireResult<()>,
) -> WireResult<Vec<u8>> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct History<'a> {
        text: &'a str,
        truncated: bool,
        first_sequence: u64,
        last_sequence: u64,
        source: &'static str,
    }
    let mut first = 0;
    let mut last = 0;
    let mut decoder = UTF_8.new_decoder_with_bom_removal();
    let mut text = String::new();
    let mut truncated = false;
    records(root, session, check, |record| {
        let sequence = record.frame.sequence();
        if through.is_some_and(|through| sequence > through) {
            return Ok(false);
        }
        if sequence < from {
            return Ok(true);
        }
        if first == 0 {
            first = sequence;
        }
        last = sequence;
        if let Frame::Output { data, .. } = record.frame {
            decode_output(&mut decoder, &data, &mut text, false);
            if text.len() > limits.bytes * 2 {
                text.drain(..tail_start(&text, limits));
                truncated = true;
            }
        }
        Ok(true)
    })?;
    decode_output(&mut decoder, &[], &mut text, true);
    let start = tail_start(&text, limits);
    serialize_ok(
        id,
        History {
            text: &text[start..],
            truncated: truncated || start != 0,
            first_sequence: first,
            last_sequence: last,
            source: "journal",
        },
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Boundary {
    command_id: String,
    session_id: String,
    started_sequence: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    executed_sequence: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_sequence: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

fn commands(
    reader: &Connection,
    session_id: &str,
    limit: usize,
) -> rusqlite::Result<Vec<Boundary>> {
    let mut statement = reader.prepare(
        "SELECT command_id, session_id, started_sequence, executed_sequence, ended_sequence,
                command_text, cwd, exit_code, created_at, updated_at
         FROM terminal_commands WHERE session_id = ?1
         ORDER BY started_sequence DESC, rowid DESC LIMIT ?2",
    )?;
    let rows = statement.query_map((session_id, limit as i64), |row| {
        Ok(Boundary {
            command_id: row.get(0)?,
            session_id: row.get(1)?,
            started_sequence: row.get(2)?,
            executed_sequence: row.get(3)?,
            ended_sequence: row.get(4)?,
            command_text: row.get(5)?,
            cwd: row.get(6)?,
            exit_code: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    })?;
    let mut boundaries = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    boundaries.reverse();
    Ok(boundaries)
}
