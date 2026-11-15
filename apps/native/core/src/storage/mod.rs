mod migrations;
mod transaction;

use anyhow::Result;
use rusqlite::Connection;

pub use migrations::{MigrationReport, migrate, migrate_with_backup, supported_schema_version};
pub use transaction::{
    CommandMetadata, CommandReplayConflictError, DomainCommit, DomainEventInput, execute,
};

/// Configure a caller-owned writable connection before migrating it.
/// The runtime owner supplies the generation and owns the database lease.
pub fn configure_connection(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA synchronous = FULL;
         PRAGMA busy_timeout = 5000;
         PRAGMA trusted_schema = OFF;",
    )?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS _runtime_meta (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL
        ) STRICT;",
    )?;
    Ok(())
}
