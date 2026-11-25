//! Local SQLite (+ FTS5) store for truncated terminal logs and indexed
//! documents (`<data_root>/context_guard.sqlite`).
//!
//! Kept strictly separate from Matou's main state database in WAL mode so
//! heavy log writes never contend with session/UI transactions.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use matou_native_core::context_optimizer_config::KNOWN_AGENT_IDS;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;

const DB_FILE_NAME: &str = "context_guard.sqlite";
const DEFAULT_RETENTION_SECS: i64 = 7 * 24 * 60 * 60; // 7 days
const CHUNK_MAX_BYTES: usize = 3_500;
const MAX_SEARCH_SNIPPET_BYTES: usize = 900;
const SAVED_STATS_QUEUE_CAPACITY: usize = 256;
const SAVED_STATS_REPLY_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentSavedStats {
    bytes7d: u64,
    events7d: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SavedStats {
    // Output replacements use model-visible bytes. Read dedup uses the file
    // slice and diff byte counts, so clients should display the total as approximate.
    agents: BTreeMap<&'static str, AgentSavedStats>,
}

impl Default for SavedStats {
    fn default() -> Self {
        Self {
            agents: KNOWN_AGENT_IDS
                .into_iter()
                .map(|id| (id, AgentSavedStats::default()))
                .collect(),
        }
    }
}

enum SavedStatsRequest {
    Record {
        data_root: PathBuf,
        agent_id: &'static str,
        bytes: u64,
        now: SystemTime,
    },
    Query {
        data_root: PathBuf,
        now: SystemTime,
        reply: mpsc::Sender<SavedStats>,
    },
    Flush(mpsc::Sender<()>),
}

static SAVED_STATS_WORKER: OnceLock<Option<SyncSender<SavedStatsRequest>>> = OnceLock::new();
static DROPPED_SAVED_STATS: AtomicUsize = AtomicUsize::new(0);

// Initialize outside the hook request path. The worker runs only in response
// to requests and queries are ordered after previously queued records.
pub(super) fn prepare_saved_stats_writer() {
    SAVED_STATS_WORKER.get_or_init(|| {
        let (send, receive) = mpsc::sync_channel(SAVED_STATS_QUEUE_CAPACITY);
        match thread::Builder::new()
            .name("context-saved-stats".into())
            .spawn(move || {
                let mut failed_writes = HashSet::new();
                for request in receive {
                    let dropped = DROPPED_SAVED_STATS.swap(0, Ordering::Relaxed);
                    if dropped != 0 {
                        eprintln!("context guard: dropped {dropped} saved statistics events");
                    }
                    match request {
                        SavedStatsRequest::Record {
                            data_root,
                            agent_id,
                            bytes,
                            now,
                        } => match record_saved_stats(&data_root, agent_id, bytes, now) {
                            Ok(()) => {
                                failed_writes.remove(&data_root);
                            }
                            Err(error) => {
                                eprintln!(
                                    "context guard: could not record saved statistics: {error:#}"
                                );
                                failed_writes.insert(data_root);
                            }
                        },
                        SavedStatsRequest::Query {
                            data_root,
                            now,
                            reply,
                        } => {
                            let stats = if failed_writes.contains(&data_root) {
                                SavedStats::default()
                            } else {
                                query_saved_stats_or_empty(&data_root, now)
                            };
                            let _ = reply.send(stats);
                        }
                        SavedStatsRequest::Flush(reply) => {
                            let _ = reply.send(());
                        }
                    }
                }
            }) {
            Ok(_) => Some(send),
            Err(error) => {
                eprintln!("context guard: could not start saved statistics writer: {error}");
                None
            }
        }
    });
}

pub(super) fn enqueue_saved_stats(
    data_root: PathBuf,
    agent_id: &'static str,
    bytes: u64,
    now: SystemTime,
) {
    let Some(Some(send)) = SAVED_STATS_WORKER.get() else {
        return;
    };
    if send
        .try_send(SavedStatsRequest::Record {
            data_root,
            agent_id,
            bytes,
            now,
        })
        .is_err()
    {
        DROPPED_SAVED_STATS.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) fn get_saved_stats(data_root: &Path, now: SystemTime) -> SavedStats {
    let Some(worker) = SAVED_STATS_WORKER.get() else {
        return query_saved_stats_or_empty(data_root, now);
    };
    let (reply, receive) = mpsc::channel();
    if worker.as_ref().is_some_and(|send| {
        send.try_send(SavedStatsRequest::Query {
            data_root: data_root.to_owned(),
            now,
            reply,
        })
        .is_ok()
    }) {
        return receive
            .recv_timeout(SAVED_STATS_REPLY_TIMEOUT)
            .unwrap_or_default();
    }
    SavedStats::default()
}

pub(crate) fn flush_saved_stats() {
    let Some(Some(send)) = SAVED_STATS_WORKER.get() else {
        return;
    };
    let (reply, receive) = mpsc::channel();
    if send.try_send(SavedStatsRequest::Flush(reply)).is_ok() {
        let _ = receive.recv_timeout(SAVED_STATS_REPLY_TIMEOUT);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredLogRecord {
    pub id: String,
    pub session_id: String,
    pub tool_name: String,
    pub command: String,
    pub raw_bytes: usize,
    pub created_at_sec: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub source_id: String,
    pub title: String,
    pub chunk_index: usize,
    pub snippet: String,
}

pub struct ContextGuardStore {
    db_path: PathBuf,
    fts5_enabled: bool,
    busy_timeout: Duration,
}

impl ContextGuardStore {
    pub fn open(data_root: &Path) -> Result<Self> {
        Self::open_with_timeout(data_root, Duration::from_millis(80))
    }

    pub(super) fn open_for_hook(data_root: &Path) -> Result<Self> {
        Self::open_with_timeout(data_root, Duration::ZERO)
    }

    fn open_with_timeout(data_root: &Path, busy_timeout: Duration) -> Result<Self> {
        fs::create_dir_all(data_root)?;
        let db_path = data_root.join(DB_FILE_NAME);
        let conn = open_connection(&db_path, busy_timeout)?;
        let fts5_enabled = init_schema(&conn)?;
        Ok(Self {
            db_path,
            fts5_enabled,
            busy_timeout,
        })
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    pub fn fts5_enabled(&self) -> bool {
        self.fts5_enabled
    }

    /// Saves a full (already secret-redacted) terminal output or document and
    /// splits it into searchable chunks. Returns the assigned `source_id`.
    pub fn save_log(
        &self,
        session_id: &str,
        tool_name: &str,
        command: &str,
        content: &str,
    ) -> Result<StoredLogRecord> {
        self.save_log_before(session_id, tool_name, command, content, None)
    }

    pub(super) fn save_log_before(
        &self,
        session_id: &str,
        tool_name: &str,
        command: &str,
        content: &str,
        deadline: Option<Instant>,
    ) -> Result<StoredLogRecord> {
        let now_sec = unix_now_sec();
        let mut conn = open_connection(&self.db_path, self.busy_timeout)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Hook writes have their own deadline. Foreground CLI writes perform
        // maintenance so an expired backlog cannot prevent a hook replacement.
        if deadline.is_none() {
            delete_expired(&tx, now_sec - DEFAULT_RETENTION_SECS, self.fts5_enabled)?;
        }
        let seq: i64 = tx.query_row(
            "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'log_sources'), 0) + 1",
            [],
            |row| row.get(0),
        )?;
        let id = format!("log_{seq:04}");
        let raw_bytes = content.len();

        tx.execute(
            "INSERT INTO log_sources (id, session_id, tool_name, command, raw_bytes, created_at_sec)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, session_id, tool_name, command, raw_bytes as i64, now_sec],
        )?;

        for (idx, chunk) in log_chunks(content).enumerate() {
            anyhow::ensure!(
                deadline.is_none_or(|time| Instant::now() < time),
                "log write expired"
            );
            tx.execute(
                "INSERT INTO log_chunks (source_id, chunk_index, heading, body)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, idx as i64, command, chunk],
            )?;
            if self.fts5_enabled {
                let indexed = tx.execute(
                    "INSERT INTO log_chunks_fts (source_id, chunk_index, heading, body)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![id, idx as i64, command, chunk],
                );
                if deadline.is_some() {
                    indexed?;
                }
            }
        }
        anyhow::ensure!(
            deadline.is_none_or(|time| Instant::now() < time),
            "log write expired"
        );
        tx.commit()?;

        Ok(StoredLogRecord {
            id,
            session_id: session_id.to_owned(),
            tool_name: tool_name.to_owned(),
            command: command.to_owned(),
            raw_bytes,
            created_at_sec: now_sec,
        })
    }

    /// Reads the complete stored text for a given `source_id`.
    pub fn get_full_log(&self, source_id: &str) -> Result<Option<String>> {
        let conn = open_connection(&self.db_path, self.busy_timeout)?;
        let raw_bytes = conn
            .query_row(
                "SELECT raw_bytes FROM log_sources WHERE id = ?1 AND created_at_sec >= ?2",
                params![source_id, unix_now_sec() - DEFAULT_RETENTION_SECS],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let Some(raw_bytes) = raw_bytes else {
            return Ok(None);
        };
        let mut stmt = conn
            .prepare("SELECT body FROM log_chunks WHERE source_id = ?1 ORDER BY chunk_index ASC")?;
        let rows = stmt
            .query_map(params![source_id], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows.is_empty() {
            Ok(None)
        } else {
            // Older rows omitted the separator between chunks.
            let separator = if rows.iter().map(String::len).sum::<usize>() as i64 == raw_bytes {
                ""
            } else {
                "\n"
            };
            Ok(Some(rows.join(separator)))
        }
    }

    /// Full-text search across stored logs/documents, optionally scoped to one source.
    pub fn search(
        &self,
        query: &str,
        source_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        self.search_logs(query, source_id, None, limit)
    }

    pub(super) fn search_session(
        &self,
        query: &str,
        source_id: Option<&str>,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        self.search_logs(query, source_id, Some(session_id), limit)
    }

    fn search_logs(
        &self,
        query: &str,
        source_id: Option<&str>,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let clean = query.trim();
        if clean.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 20) as i64;
        let mut conn = open_connection(&self.db_path, self.busy_timeout)?;
        let tx = conn.transaction()?;
        let cutoff = unix_now_sec() - DEFAULT_RETENTION_SECS;
        if let Some(id) = source_id
            && session_id.is_some()
        {
            check_log_source(&tx, id, cutoff)?;
        }
        let session_id = session_id.filter(|_| source_id.is_none());

        if self.fts5_enabled {
            let fts_query = sanitize_fts_query(clean);
            if !fts_query.is_empty() {
                let mut stmt = tx.prepare(
                    "SELECT source_id, heading, chunk_index, body
                     FROM log_chunks_fts JOIN log_sources ON log_sources.id = source_id
                     WHERE log_chunks_fts MATCH ?1 AND (?2 IS NULL OR source_id = ?2)
                       AND (?3 IS NULL OR session_id = ?3) AND created_at_sec >= ?4
                     ORDER BY bm25(log_chunks_fts)
                     LIMIT ?5",
                )?;
                let hits = collect_hits(
                    &mut stmt,
                    params![fts_query, source_id, session_id, cutoff, limit],
                    clean,
                )?;
                if !hits.is_empty() {
                    return Ok(hits);
                }
            }
        }

        // Deterministic substring fallback (also catches punctuation-heavy error tokens).
        let like_pattern = format!(
            "%{}%",
            clean
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut stmt = tx.prepare(
            "SELECT source_id, heading, chunk_index, body
             FROM log_chunks JOIN log_sources ON log_sources.id = source_id
             WHERE body LIKE ?1 ESCAPE '\\' AND (?2 IS NULL OR source_id = ?2)
               AND (?3 IS NULL OR session_id = ?3) AND created_at_sec >= ?4
             ORDER BY log_sources.seq DESC, chunk_index ASC
             LIMIT ?5",
        )?;
        collect_hits(
            &mut stmt,
            params![like_pattern, source_id, session_id, cutoff, limit],
            clean,
        )
    }

    /// Removes logs older than `retention_secs` (defaults to 7 days).
    pub fn prune_expired(&self, retention_secs: Option<i64>) -> Result<usize> {
        let cutoff = unix_now_sec() - retention_secs.unwrap_or(DEFAULT_RETENTION_SECS);
        let mut conn = open_connection(&self.db_path, self.busy_timeout)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = delete_expired(&tx, cutoff, self.fts5_enabled)?;
        tx.commit()?;
        Ok(removed)
    }

    pub(super) fn require_log_source(&self, source_id: &str) -> Result<()> {
        let conn = open_connection(&self.db_path, self.busy_timeout)?;
        check_log_source(&conn, source_id, unix_now_sec() - DEFAULT_RETENTION_SECS)
    }

    pub(super) fn mark_manual_resume(&self, session_id: &str) -> Result<()> {
        let conn = open_connection(&self.db_path, self.busy_timeout)?;
        conn.execute(
            "INSERT INTO session_resume (session_id, manual_resume_at) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET manual_resume_at = max(manual_resume_at, excluded.manual_resume_at)",
            params![session_id, unix_now_sec()],
        )?;
        Ok(())
    }

    pub fn list_session_logs(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredLogRecord>> {
        let conn = open_connection(&self.db_path, self.busy_timeout)?;
        let mut stmt = conn.prepare(
            "SELECT id, session_id, tool_name, command, raw_bytes, created_at_sec
             FROM log_sources
             WHERE session_id = ?1 AND created_at_sec >= ?2
             ORDER BY seq DESC
             LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(
                params![
                    session_id,
                    unix_now_sec() - DEFAULT_RETENTION_SECS,
                    limit.clamp(1, 50) as i64
                ],
                |row| {
                    Ok(StoredLogRecord {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        tool_name: row.get(2)?,
                        command: row.get(3)?,
                        raw_bytes: row.get::<_, i64>(4)?.max(0) as usize,
                        created_at_sec: row.get(5)?,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn latest_source_id(&self, session_id: &str) -> Result<Option<String>> {
        let conn = open_connection(&self.db_path, self.busy_timeout)?;
        conn.query_row(
            "SELECT id FROM log_sources WHERE session_id = ?1 AND created_at_sec >= ?2 ORDER BY seq DESC LIMIT 1",
            params![session_id, unix_now_sec() - DEFAULT_RETENTION_SECS],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }
}

fn delete_expired(conn: &Connection, cutoff: i64, fts5_enabled: bool) -> Result<usize> {
    let expired = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM log_sources WHERE created_at_sec < ?1)",
        [cutoff],
        |row| row.get::<_, bool>(0),
    )?;
    if !expired {
        return Ok(0);
    }
    if fts5_enabled {
        conn.execute(
            "DELETE FROM log_chunks_fts WHERE source_id IN
                (SELECT id FROM log_sources WHERE created_at_sec < ?1)",
            [cutoff],
        )?;
    }
    conn.execute(
        "DELETE FROM log_chunks WHERE source_id IN
            (SELECT id FROM log_sources WHERE created_at_sec < ?1)",
        [cutoff],
    )?;
    Ok(conn.execute(
        "DELETE FROM log_sources WHERE created_at_sec < ?1",
        [cutoff],
    )?)
}

fn check_log_source(conn: &Connection, id: &str, cutoff: i64) -> Result<()> {
    let (created, high_water): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT (SELECT created_at_sec FROM log_sources WHERE id = ?1),
                (SELECT seq FROM sqlite_sequence WHERE name = 'log_sources')",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if created.is_some_and(|time| time >= cutoff) {
        return Ok(());
    }
    // IDs are assigned from AUTOINCREMENT in the insert transaction. Its
    // high-water mark also covers rows cleaned up by older versions.
    let was_allocated = id
        .strip_prefix("log_")
        .and_then(|digits| digits.parse::<i64>().ok())
        .is_some_and(|seq| seq > 0 && Some(seq) <= high_water && id == format!("log_{seq:04}"));
    if created.is_some() || was_allocated {
        anyhow::bail!("Log `{id}` has expired (7-day retention) and is no longer available.");
    }
    anyhow::bail!("Log `{id}` was not found.");
}

fn open_connection(path: &Path, busy_timeout: Duration) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(busy_timeout)?;
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    Ok(conn)
}

fn open_saved_stats(data_root: &Path, now: SystemTime) -> Result<(Connection, String)> {
    fs::create_dir_all(data_root)?;
    let conn = open_connection(&data_root.join(DB_FILE_NAME), Duration::from_millis(80))?;
    let mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    anyhow::ensure!(mode == "wal", "saved statistics require WAL mode");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS saved_stats (
            day TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            bytes INTEGER NOT NULL,
            events INTEGER NOT NULL,
            PRIMARY KEY(day, agent_id)
        );",
    )?;
    let seconds = i64::try_from(now.duration_since(UNIX_EPOCH)?.as_secs())?;
    let today = conn.query_row(
        "SELECT date(?1, 'unixepoch', 'localtime')",
        [seconds],
        |row| row.get(0),
    )?;
    Ok((conn, today))
}

pub(super) fn record_saved_stats(
    data_root: &Path,
    agent_id: &str,
    bytes: u64,
    now: SystemTime,
) -> Result<()> {
    let (conn, day) = open_saved_stats(data_root, now)?;
    let bytes = i64::try_from(bytes)?;
    conn.execute(
        "INSERT INTO saved_stats (day, agent_id, bytes, events) VALUES (?1, ?2, ?3, 1)
         ON CONFLICT(day, agent_id) DO UPDATE SET
            bytes = bytes + excluded.bytes, events = events + excluded.events",
        params![day, agent_id, bytes],
    )?;
    Ok(())
}

fn query_saved_stats_or_empty(data_root: &Path, now: SystemTime) -> SavedStats {
    query_saved_stats(data_root, now).unwrap_or_else(|error| {
        eprintln!("context guard: could not read saved statistics: {error:#}");
        SavedStats::default()
    })
}

fn query_saved_stats(data_root: &Path, now: SystemTime) -> Result<SavedStats> {
    let (conn, today) = open_saved_stats(data_root, now)?;
    conn.execute(
        "DELETE FROM saved_stats WHERE day < date(?1, '-30 days')",
        [&today],
    )?;
    let mut query = conn.prepare(
        "SELECT agent_id, sum(bytes), sum(events) FROM saved_stats
         WHERE day BETWEEN date(?1, '-6 days') AND ?1 GROUP BY agent_id",
    )?;
    let mut rows = query.query([&today])?;
    let mut stats = SavedStats::default();
    while let Some(row) = rows.next()? {
        let agent_id: String = row.get(0)?;
        if let Some(agent) = stats.agents.get_mut(agent_id.as_str()) {
            *agent = AgentSavedStats {
                bytes7d: u64::try_from(row.get::<_, i64>(1)?)?,
                events7d: u64::try_from(row.get::<_, i64>(2)?)?,
            };
        }
    }
    Ok(stats)
}

fn init_schema(conn: &Connection) -> Result<bool> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS log_sources (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            session_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            command TEXT NOT NULL,
            raw_bytes INTEGER NOT NULL,
            created_at_sec INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_log_sources_session
            ON log_sources (session_id, seq DESC);
        CREATE INDEX IF NOT EXISTS idx_log_sources_created
            ON log_sources (created_at_sec);
        CREATE TABLE IF NOT EXISTS session_resume (
            session_id TEXT PRIMARY KEY,
            manual_resume_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS log_chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_id TEXT NOT NULL,
            chunk_index INTEGER NOT NULL,
            heading TEXT NOT NULL,
            body TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_log_chunks_source
            ON log_chunks (source_id, chunk_index ASC);",
    )?;
    let fts_ok = conn
        .execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS log_chunks_fts USING fts5(
                source_id UNINDEXED,
                chunk_index UNINDEXED,
                heading,
                body,
                tokenize='porter unicode61'
            );",
        )
        .is_ok();
    Ok(fts_ok)
}

fn collect_hits<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
    query: &str,
) -> Result<Vec<SearchHit>> {
    let rows = stmt
        .query_map(params, |row| {
            let source_id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let chunk_index: i64 = row.get(2)?;
            let body: String = row.get(3)?;
            Ok(SearchHit {
                source_id,
                title,
                chunk_index: chunk_index.max(0) as usize,
                snippet: extract_snippet(&body, query, MAX_SEARCH_SNIPPET_BYTES),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn log_chunks(text: &str) -> impl Iterator<Item = &str> {
    let mut remaining = Some(text);
    std::iter::from_fn(move || {
        let text = remaining.take()?;
        if text.len() <= CHUNK_MAX_BYTES {
            return Some(text);
        }
        let limit = text.floor_char_boundary(CHUNK_MAX_BYTES);
        let end = text[..limit].rfind('\n').map_or(limit, |index| index + 1);
        remaining = Some(&text[end..]);
        Some(&text[..end])
    })
}

fn sanitize_fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter_map(|word| {
            let cleaned: String = word
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!cleaned.is_empty()).then(|| format!("\"{cleaned}\""))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_snippet(body: &str, query: &str, max_bytes: usize) -> String {
    let lower_body = body.to_ascii_lowercase();
    let needle = query
        .split_whitespace()
        .next()
        .unwrap_or(query)
        .to_ascii_lowercase();
    let match_pos = lower_body.find(&needle).unwrap_or(0);
    let mut start = match_pos.saturating_sub(max_bytes / 3);
    while start > 0 && !body.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + max_bytes).min(body.len());
    while end > start && !body.is_char_boundary(end) {
        end -= 1;
    }
    let slice = body[start..end].trim();
    if start > 0 && end < body.len() {
        format!("...{slice}...")
    } else if start > 0 {
        format!("...{slice}")
    } else if end < body.len() {
        format!("{slice}...")
    } else {
        slice.to_owned()
    }
}

fn unix_now_sec() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_time(value: &str) -> SystemTime {
        let conn = Connection::open_in_memory().unwrap();
        let seconds: i64 = conn
            .query_row("SELECT unixepoch(?1, 'utc')", [value], |row| row.get(0))
            .unwrap();
        UNIX_EPOCH + Duration::from_secs(u64::try_from(seconds).unwrap())
    }

    #[test]
    fn saved_stats_accumulate_and_prune_by_local_calendar_day() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let now = local_time("2026-02-10 00:00:00");
        assert_eq!(query_saved_stats(root, now)?, SavedStats::default());

        for (time, agent, bytes) in [
            ("2026-01-10 12:00:00", "claude-code", 1),
            ("2026-01-11 12:00:00", "claude-code", 2),
            ("2026-02-03 23:59:59", "claude-code", 4),
            ("2026-02-04 00:00:00", "claude-code", 8),
            ("2026-02-09 23:59:59", "claude-code", 16),
            ("2026-02-10 00:00:00", "claude-code", 32),
            ("2026-02-10 00:00:01", "claude-code", 64),
            ("2026-02-10 00:00:01", "codex", 128),
            ("2026-02-11 00:00:00", "claude-code", 256),
        ] {
            record_saved_stats(root, agent, bytes, local_time(time))?;
        }

        let mut expected = SavedStats::default();
        expected.agents.insert(
            "claude-code",
            AgentSavedStats {
                bytes7d: 120,
                events7d: 4,
            },
        );
        expected.agents.insert(
            "codex",
            AgentSavedStats {
                bytes7d: 128,
                events7d: 1,
            },
        );
        assert_eq!(query_saved_stats(root, now)?, expected);

        let conn = Connection::open(root.join(DB_FILE_NAME))?;
        let mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        assert_eq!(mode, "wal");
        let totals = |day| {
            conn.query_row(
                "SELECT bytes, events FROM saved_stats WHERE day = ?1 AND agent_id = 'claude-code'",
                [day],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
        };
        assert_eq!(totals("2026-01-10")?, None);
        assert_eq!(totals("2026-01-11")?, Some((2, 1)));
        assert_eq!(totals("2026-02-09")?, Some((16, 1)));
        assert_eq!(totals("2026-02-10")?, Some((96, 2)));
        Ok(())
    }

    #[test]
    fn saved_stats_return_all_zeros_when_database_cannot_be_used() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let now = local_time("2026-02-10 12:00:00");
        let path = root.join(DB_FILE_NAME);
        fs::create_dir(&path)?;
        assert_eq!(get_saved_stats(root, now), SavedStats::default());
        fs::remove_dir(&path)?;
        let conn = Connection::open(path)?;
        conn.execute_batch("CREATE TABLE saved_stats (unrecognized INTEGER)")?;
        assert_eq!(get_saved_stats(root, now), SavedStats::default());
        Ok(())
    }

    #[test]
    fn saved_stats_hide_historical_totals_while_writes_are_failing() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let now = local_time("2026-02-10 12:00:00");
        prepare_saved_stats_writer();
        record_saved_stats(root, "claude-code", 22, now)?;
        let conn = Connection::open(root.join(DB_FILE_NAME))?;
        conn.execute_batch(
            "CREATE TRIGGER reject_stats BEFORE INSERT ON saved_stats
            BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;",
        )?;
        enqueue_saved_stats(root.to_owned(), "claude-code", 3, now);
        assert_eq!(get_saved_stats(root, now), SavedStats::default());
        assert_eq!(
            query_saved_stats(root, now)?.agents["claude-code"].bytes7d,
            22
        );

        conn.execute_batch("DROP TRIGGER reject_stats")?;
        enqueue_saved_stats(root.to_owned(), "claude-code", 4, now);
        assert_eq!(
            get_saved_stats(root, now).agents["claude-code"],
            AgentSavedStats {
                bytes7d: 26,
                events7d: 2
            }
        );
        Ok(())
    }

    #[test]
    fn saves_searches_and_prunes_logs() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextGuardStore::open(dir.path()).unwrap();
        assert!(store.fts5_enabled());

        let log_text = "running 200 tests\nok test_alpha\nFAILED test_router_timeout: connection refused at line 42\nok test_beta\n";
        let saved = store
            .save_log("sess-1", "Bash", "cargo test", log_text)
            .unwrap();
        assert_eq!(saved.id, "log_0001");

        let hits = store.search("router_timeout", Some(&saved.id), 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("connection refused"));

        let full = store.get_full_log(&saved.id).unwrap().unwrap();
        assert!(full.contains("test_alpha"));

        let pruned = store.prune_expired(Some(-1)).unwrap();
        assert_eq!(pruned, 1);
        assert!(store.get_full_log(&saved.id).unwrap().is_none());
    }
}
