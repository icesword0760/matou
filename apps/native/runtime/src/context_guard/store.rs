//! Local SQLite (+ FTS5) store for truncated terminal logs and indexed
//! documents (`<data_root>/context_guard.sqlite`).
//!
//! Kept strictly separate from Matou's main state database in WAL mode so
//! heavy log writes never contend with session/UI transactions.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

const DB_FILE_NAME: &str = "context_guard.sqlite";
const DEFAULT_RETENTION_SECS: i64 = 7 * 24 * 60 * 60; // 7 days
const CHUNK_MAX_BYTES: usize = 3_500;
const MAX_SEARCH_SNIPPET_BYTES: usize = 900;

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
                "SELECT raw_bytes FROM log_sources WHERE id = ?1",
                params![source_id],
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

    /// Full-text search across stored logs/documents, optionally scoped to one `source_id`.
    pub fn search(
        &self,
        query: &str,
        source_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let clean = query.trim();
        if clean.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 20) as i64;
        let conn = open_connection(&self.db_path, self.busy_timeout)?;

        if self.fts5_enabled {
            let fts_query = sanitize_fts_query(clean);
            if !fts_query.is_empty() {
                let hits = if let Some(src) = source_id {
                    let mut stmt = conn.prepare(
                        "SELECT source_id, heading, chunk_index, body
                         FROM log_chunks_fts
                         WHERE log_chunks_fts MATCH ?1 AND source_id = ?2
                         ORDER BY bm25(log_chunks_fts)
                         LIMIT ?3",
                    )?;
                    collect_hits(&mut stmt, params![fts_query, src, limit], clean)?
                } else {
                    let mut stmt = conn.prepare(
                        "SELECT source_id, heading, chunk_index, body
                         FROM log_chunks_fts
                         WHERE log_chunks_fts MATCH ?1
                         ORDER BY bm25(log_chunks_fts)
                         LIMIT ?2",
                    )?;
                    collect_hits(&mut stmt, params![fts_query, limit], clean)?
                };
                if !hits.is_empty() {
                    return Ok(hits);
                }
            }
        }

        // Deterministic substring fallback (also catches punctuation-heavy error tokens).
        let like_pattern = format!("%{}%", clean.replace('%', "\\%").replace('_', "\\_"));
        let hits = if let Some(src) = source_id {
            let mut stmt = conn.prepare(
                "SELECT source_id, heading, chunk_index, body
                 FROM log_chunks
                 WHERE source_id = ?1 AND body LIKE ?2 ESCAPE '\\'
                 ORDER BY chunk_index ASC
                 LIMIT ?3",
            )?;
            collect_hits(&mut stmt, params![src, like_pattern, limit], clean)?
        } else {
            let mut stmt = conn.prepare(
                "SELECT source_id, heading, chunk_index, body
                 FROM log_chunks
                 WHERE body LIKE ?1 ESCAPE '\\'
                 ORDER BY id DESC
                 LIMIT ?2",
            )?;
            collect_hits(&mut stmt, params![like_pattern, limit], clean)?
        };
        Ok(hits)
    }

    /// Removes logs older than `retention_secs` (defaults to 7 days).
    pub fn prune_expired(&self, retention_secs: Option<i64>) -> Result<usize> {
        let cutoff = unix_now_sec() - retention_secs.unwrap_or(DEFAULT_RETENTION_SECS);
        let conn = open_connection(&self.db_path, self.busy_timeout)?;
        let tx = conn.unchecked_transaction()?;
        let expired_ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM log_sources WHERE created_at_sec < ?1")?;
            stmt.query_map(params![cutoff], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for id in &expired_ids {
            tx.execute("DELETE FROM log_chunks WHERE source_id = ?1", params![id])?;
            if self.fts5_enabled {
                let _ = tx.execute(
                    "DELETE FROM log_chunks_fts WHERE source_id = ?1",
                    params![id],
                );
            }
            tx.execute("DELETE FROM log_sources WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(expired_ids.len())
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
             WHERE session_id = ?1
             ORDER BY seq DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![session_id, limit.clamp(1, 50) as i64], |row| {
                Ok(StoredLogRecord {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    tool_name: row.get(2)?,
                    command: row.get(3)?,
                    raw_bytes: row.get::<_, i64>(4)?.max(0) as usize,
                    created_at_sec: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn latest_source_id(&self, session_id: &str) -> Result<Option<String>> {
        let conn = open_connection(&self.db_path, self.busy_timeout)?;
        conn.query_row(
            "SELECT id FROM log_sources WHERE session_id = ?1 ORDER BY seq DESC LIMIT 1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }
}

fn open_connection(path: &Path, busy_timeout: Duration) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(busy_timeout)?;
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    Ok(conn)
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

        let hits = store
            .search("router_timeout", Some(&saved.id), 5)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("connection refused"));

        let full = store.get_full_log(&saved.id).unwrap().unwrap();
        assert!(full.contains("test_alpha"));

        let pruned = store.prune_expired(Some(-1)).unwrap();
        assert_eq!(pruned, 1);
        assert!(store.get_full_log(&saved.id).unwrap().is_none());
    }
}
