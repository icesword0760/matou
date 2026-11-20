//! Run-scoped Codex lifecycle. The owner admits hooks before applying effects.
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs::File,
    hash::{DefaultHasher, Hash, Hasher},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use matou_native_core::pty::ShellLaunch;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    derived::CodexInterruption,
    provider_hook::{nonempty, one_line, transcript_uuid},
};

pub(crate) const EVENTS: [(&str, u8); 7] = [
    ("SessionStart", 5),
    ("UserPromptSubmit", 5),
    ("PreToolUse", 10),
    ("PermissionRequest", 10),
    ("PostToolUse", 10),
    ("Stop", 5),
    ("SessionEnd", 3),
];

const ERROR_POLL_INTERVAL: Duration = Duration::from_millis(100);
const ROLLOUT_TAIL_BYTES: u64 = 64 * 1024;

#[derive(PartialEq, Eq)]
pub(crate) struct Hook {
    transcript: Option<PathBuf>,
    has_transcript_field: bool,
    turn: Option<String>,
    fingerprint: Option<String>,
}

impl Hook {
    pub fn parse(payload: &Map<String, Value>, event: &str) -> Self {
        let text = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| payload.get(*key).and_then(nonempty))
        };
        let paths = ["transcript_path", "transcriptPath", "agent_transcript_path"];
        let turn = text(&["turn_id", "turnId"]).map(str::to_owned);
        let fingerprint = match event {
            "PreToolUse" | "PostToolUse" | "PermissionRequest" => Some(
                text(&["tool_use_id", "toolUseId"])
                    .map(|id| format!("id:{id}"))
                    .unwrap_or_else(|| {
                        // Some approval hooks omit a call ID. Deduplicate the
                        // payload in this turn, independent of JSON key order.
                        let mut hash = DefaultHasher::new();
                        for (field, value) in payload {
                            field.hash(&mut hash);
                            value.to_string().hash(&mut hash);
                        }
                        format!("payload:{}", hash.finish())
                    }),
            ),
            _ => None,
        };
        Self {
            transcript: text(&paths).map(PathBuf::from),
            has_transcript_field: paths.iter().any(|key| payload.contains_key(*key)),
            turn,
            fingerprint,
        }
    }
}

pub(crate) struct Effect {
    pub status: &'static str,
    pub error: Option<Value>,
    pub notify: bool,
}

impl Effect {
    fn status(status: &'static str) -> Self {
        Self {
            status,
            error: None,
            notify: false,
        }
    }
}

struct Turn {
    id: Option<String>,
    seen_tools: HashSet<(&'static str, String)>,
    next_poll: Instant,
    scanned_size: Option<u64>,
}

pub(crate) struct Run {
    home: Option<PathBuf>,
    rollout: Option<(PathBuf, File)>,
    pub title: Option<title::Watch>,
    turns_seen: HashSet<String>,
    turn: Option<Turn>,
    ended: bool,
    interruption: CodexInterruption,
}

impl Run {
    pub fn new(launch: &ShellLaunch) -> Self {
        let env = |key: &str| {
            launch
                .env
                .get(OsStr::new(key))
                .or_else(|| launch.inherited_env.get(OsStr::new(key)))
                .filter(|value| !value.is_empty())
        };
        let home = env("CODEX_HOME").map(PathBuf::from).or_else(|| {
            let home_key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
            env(home_key).map(|home| Path::new(home).join(".codex"))
        });
        Self {
            home: home.map(|home| launch.cwd.join(home)),
            rollout: None,
            title: None,
            turns_seen: HashSet::new(),
            turn: None,
            ended: false,
            interruption: CodexInterruption::default(),
        }
    }

    pub fn start_rollout(&self, hook: &Hook, identity: Uuid) -> Option<(PathBuf, File)> {
        let home = self.home.as_ref()?.canonicalize().ok()?;
        let path = hook.transcript.as_ref()?;
        if !path.is_absolute() {
            return None;
        }
        let path = path.canonicalize().ok()?;
        if !path.starts_with(home)
            || path.extension().and_then(OsStr::to_str) != Some("jsonl")
            || transcript_uuid(path.file_name()?.to_str()?)
                .and_then(|id| Uuid::parse_str(id).ok())
                .is_some_and(|id| id != identity)
            || !path.is_file()
        {
            return None;
        }
        let file = File::open(&path).ok()?;
        file.metadata().ok()?.is_file().then_some((path, file))
    }

    pub fn confirm(&mut self, rollout: (PathBuf, File), identity: String) {
        self.title = self
            .home
            .as_ref()
            .map(|home| title::Watch::new(home.clone(), identity));
        self.rollout = Some(rollout);
    }

    pub fn admits_hook(&self, hook: &Hook) -> bool {
        !self.ended
            && self.rollout.as_ref().is_some_and(|(rollout, _)| {
                !hook.has_transcript_field
                    || hook
                        .transcript
                        .as_ref()
                        .and_then(|path| path.canonicalize().ok())
                        .as_ref()
                        == Some(rollout)
            })
    }

    pub fn apply_admitted(&mut self, event: &'static str, hook: Hook, cwd: &str) -> Option<Effect> {
        if let Some(title) = &mut self.title {
            title.refresh(event == "SessionEnd");
        }
        if event == "UserPromptSubmit" {
            if let Some(id) = &hook.turn {
                if !self.turns_seen.insert(id.clone()) {
                    return None;
                }
            } else if self.turn.is_some() {
                return None;
            }
            self.interruption = CodexInterruption::default();
            self.turn = Some(Turn {
                id: hook.turn,
                seen_tools: HashSet::new(),
                next_poll: Instant::now(),
                scanned_size: None,
            });
            return Some(Effect::status("running"));
        }
        if event == "SessionEnd" {
            self.ended = true;
            let error = self.poll_error(cwd, true).and_then(|effect| effect.error);
            self.turn = None;
            return Some(Effect {
                error,
                ..Effect::status("exited")
            });
        }
        let turn = self.turn.as_mut()?;
        if hook.turn.is_some() && hook.turn != turn.id {
            return None;
        }
        match event {
            "Stop" => {
                if let Some(error) = self.poll_error(cwd, true) {
                    return Some(error);
                }
                self.turn = None;
                Some(Effect {
                    notify: true,
                    ..Effect::status("idle")
                })
            }
            "PreToolUse" | "PostToolUse" | "PermissionRequest" => {
                if !turn.seen_tools.insert((event, hook.fingerprint?)) {
                    return None;
                }
                Some(if event == "PermissionRequest" {
                    Effect {
                        notify: true,
                        ..Effect::status("needs-input")
                    }
                } else {
                    Effect::status("running")
                })
            }
            _ => None,
        }
    }

    pub fn output(&mut self, chunk: &str) -> Option<Effect> {
        if self.turn.is_some() && self.interruption.ingest(chunk) {
            self.turn = None;
            return Some(Effect::status("interrupted"));
        }
        None
    }

    pub fn poll_error(&mut self, cwd: &str, flush: bool) -> Option<Effect> {
        let turn = self.turn.as_mut()?;
        let id = turn.id.as_deref()?;
        let now = Instant::now();
        if !flush && now < turn.next_poll {
            return None;
        }
        turn.next_poll = now + ERROR_POLL_INTERVAL;
        let (_, file) = self.rollout.as_mut()?;
        let size = file.metadata().ok()?.len();
        if !flush && turn.scanned_size == Some(size) {
            return None;
        }
        turn.scanned_size = Some(size);
        let message = read_error(file, size, id)?;
        let project = Path::new(cwd).file_name().and_then(OsStr::to_str);
        let subtitle = project.map_or_else(|| "Error".into(), |name| format!("Error in {name}"));
        let body: Vec<_> = one_line(&message).encode_utf16().take(200).collect();
        let error = json!({
            "eventType": "error",
            "title": "Codex",
            "subtitle": subtitle,
            "body": String::from_utf16_lossy(&body),
            "sound": true,
            "cooldownKey": "Stop",
            "replacementKey": format!("codex-turn-error:{id}"),
        });
        // A terminal turn is consumed. A duplicate prompt, Stop or error
        // cannot reopen it, and the next prompt creates a fresh turn.
        self.turn = None;
        Some(Effect {
            error: Some(error),
            ..Effect::status("error")
        })
    }
}

fn read_error(file: &mut File, size: u64, turn: &str) -> Option<String> {
    let start = size.saturating_sub(ROLLOUT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start.saturating_sub(1))).ok()?;
    let mut reader = BufReader::new(file.take(ROLLOUT_TAIL_BYTES + 1));
    let mut line = Vec::new();
    if start != 0 {
        // Begin at a record boundary, including when the tail starts on one.
        reader.read_until(b'\n', &mut line).ok()?;
        line.clear();
    }
    while reader.read_until(b'\n', &mut line).ok()? != 0 {
        if let Ok(row) = serde_json::from_slice::<Value>(&line) {
            let payload = &row["payload"];
            if row["type"] == "event_msg"
                && payload["type"] == "task_complete"
                && payload["turn_id"] == turn
                && let Some(message) = nonempty(&payload["error"]["message"])
            {
                return Some(message.to_owned());
            }
        }
        line.clear();
    }
    None
}

pub(crate) mod title {
    //! Observe the index of an admitted Codex run without blocking the owner on disk reads.
    use std::{
        fs::{self, Metadata, OpenOptions},
        io::{self, BufRead, BufReader, Read},
        path::PathBuf,
        sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
        thread,
        time::{Duration, Instant, SystemTime},
    };

    use rusqlite::Connection;
    use serde_json::Value;

    const POLL_INTERVAL: Duration = Duration::from_millis(250);
    const WATCH_WINDOW: Duration = Duration::from_secs(30);
    const MAX_LINE: u64 = 8 * 1024 * 1024;

    pub(crate) struct Watch {
        home: PathBuf,
        pub identity: String,
        pub due: Instant,
        deadline: Instant,
        final_read: bool,
        stamp: Option<Stamp>,
    }

    impl Watch {
        pub fn new(home: PathBuf, identity: String) -> Self {
            let now = Instant::now();
            Self {
                home,
                identity,
                due: now,
                deadline: now + WATCH_WINDOW,
                final_read: false,
                stamp: None,
            }
        }

        pub fn refresh(&mut self, ended: bool) {
            let now = Instant::now();
            if ended || now >= self.deadline {
                self.due = now;
            }
            self.deadline = now + WATCH_WINDOW;
            self.final_read = ended;
        }

        pub fn ready(&self, now: Instant) -> bool {
            now >= self.due && (now < self.deadline || self.final_read)
        }

        pub fn request(&mut self, session: String, run: String, now: Instant) -> Request {
            self.due = now + POLL_INTERVAL;
            if self.final_read {
                self.deadline = now;
                self.final_read = false;
            }
            Request {
                session,
                run,
                identity: self.identity.clone(),
                home: self.home.clone(),
                previous: self.stamp,
            }
        }

        pub fn accept(&mut self, read: ReadIndex) -> Option<String> {
            match read {
                ReadIndex::Unchanged => None,
                ReadIndex::Missing => {
                    self.stamp = None;
                    None
                }
                ReadIndex::Read(stamp, title) => {
                    self.stamp = Some(stamp);
                    title
                }
            }
        }

        pub fn retry(&mut self) {
            self.stamp = None;
        }
    }

    pub(crate) struct Request {
        session: String,
        run: String,
        identity: String,
        home: PathBuf,
        previous: Option<Stamp>,
    }

    pub(crate) struct Observation {
        pub session: String,
        pub run: String,
        pub identity: String,
        pub read: ReadIndex,
    }

    #[derive(Default)]
    pub(crate) struct Reader {
        worker: Option<(SyncSender<Request>, Receiver<Observation>)>,
        busy: bool,
    }

    impl Reader {
        pub fn idle(&self) -> bool {
            !self.busy
        }

        pub fn submit(&mut self, request: Request) {
            if self.worker.is_none() {
                self.worker = start_reader().ok();
            }
            let Some((requests, _)) = &self.worker else {
                return;
            };
            if requests.try_send(request).is_ok() {
                self.busy = true;
            } else {
                self.worker = None;
            }
        }

        pub fn poll(&mut self) -> Option<Observation> {
            if !self.busy {
                return None;
            }
            match self.worker.as_ref()?.1.try_recv() {
                Ok(result) => {
                    self.busy = false;
                    Some(result)
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.busy = false;
                    None
                }
            }
        }
    }

    fn start_reader() -> io::Result<(SyncSender<Request>, Receiver<Observation>)> {
        let (requests, incoming) = mpsc::sync_channel::<Request>(1);
        let (outgoing, replies) = mpsc::channel();
        thread::Builder::new()
            .name("codex-title".into())
            .spawn(move || {
                let Ok(dates) = Connection::open_in_memory() else {
                    return;
                };
                for request in incoming {
                    let read = read_index(&request, &dates).unwrap_or(ReadIndex::Unchanged);
                    if outgoing
                        .send(Observation {
                            session: request.session,
                            run: request.run,
                            identity: request.identity,
                            read,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok((requests, replies))
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Stamp {
        size: u64,
        modified: SystemTime,
        #[cfg(unix)]
        identity: (u64, u64, i64, i64),
    }

    impl Stamp {
        fn new(meta: &Metadata) -> io::Result<Self> {
            #[cfg(unix)]
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                size: meta.len(),
                modified: meta.modified()?,
                #[cfg(unix)]
                identity: (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec()),
            })
        }
    }

    pub(crate) enum ReadIndex {
        Unchanged,
        Missing,
        Read(Stamp, Option<String>),
    }

    fn read_index(request: &Request, dates: &Connection) -> io::Result<ReadIndex> {
        let path = request.home.canonicalize()?.join("session_index.jsonl");
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) => return Ok(ReadIndex::Missing),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ReadIndex::Missing),
            Err(error) => return Err(error),
        };
        let stamp = Stamp::new(&meta)?;
        if request.previous == Some(stamp) {
            return Ok(ReadIndex::Unchanged);
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        let opened = file.metadata()?;
        if !opened.is_file() || Stamp::new(&opened)? != stamp {
            // An append or replacement raced the open. Leave the stamp unconsumed.
            return Ok(ReadIndex::Unchanged);
        }
        let mut reader = BufReader::new(file.take(stamp.size));
        let title = latest_title(&mut reader, &request.identity, dates)?;
        Ok(ReadIndex::Read(stamp, title))
    }

    fn latest_title(
        reader: &mut impl BufRead,
        identity: &str,
        dates: &Connection,
    ) -> io::Result<Option<String>> {
        let mut newest: Option<(i64, String)> = None;
        let mut line = Vec::new();
        let mut timestamp = dates
            .prepare_cached("SELECT CAST(ROUND(unixepoch(?1, 'subsec') * 1000) AS INTEGER)")
            .map_err(io::Error::other)?;
        loop {
            line.clear();
            let size = reader.take(MAX_LINE + 1).read_until(b'\n', &mut line)?;
            if size == 0 {
                break;
            }
            if size as u64 > MAX_LINE {
                if !line.ends_with(b"\n") {
                    reader.skip_until(b'\n')?;
                }
                continue;
            }
            let Ok(row) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if row["id"].as_str() != Some(identity) {
                continue;
            }
            let Some(title) = row["thread_name"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let Some(date) = row["updated_at"]
                .as_str()
                .filter(|date| date.as_bytes().get(4) == Some(&b'-'))
            else {
                continue;
            };
            if let Ok(Some(at)) = timestamp.query_row([date], |row| row.get(0))
                && newest.as_ref().is_none_or(|(previous, _)| at >= *previous)
            {
                newest = Some((at, title.to_owned()));
            }
        }
        Ok(newest.map(|(_, title)| title))
    }
}
