use anyhow::{Result, bail, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CATALOG: &str = include_str!("../../schema/migrations.json");

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    pub applied_versions: Vec<i64>,
    pub current_version: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Migration {
    version: i64,
    name: String,
    sql: String,
    checksum: String,
    legacy_checksum: String,
    #[serde(default)]
    accepted_legacy_checksums: Vec<String>,
    #[serde(default)]
    foreign_keys_off: bool,
}

impl Migration {
    fn accepts_checksum(&self, stored: &str) -> bool {
        if stored.starts_with("v2:") {
            self.checksum == stored
        } else {
            self.legacy_checksum == stored
                || self
                    .accepted_legacy_checksums
                    .iter()
                    .any(|legacy| legacy == stored)
        }
    }
}

/// Apply the frozen source migrations. The caller owns backups and recovery.
pub fn migrate(connection: &mut Connection, applied_at: i64) -> Result<MigrationReport> {
    let mut migrations: Vec<Migration> = serde_json::from_str(CATALOG)?;
    migrations.sort_unstable_by_key(|migration| migration.version);
    validate_catalog(&migrations)?;

    let history_exists = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    let applied = if history_exists {
        let mut statement = connection
            .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let supported_version = migrations.last().map_or(0, |migration| migration.version);
    let current_version = applied.last().map_or(0, |(version, _, _)| *version);
    ensure!(
        current_version <= supported_version,
        "database schema version {current_version} is newer than supported version {supported_version}"
    );

    let mut history_updates = Vec::new();
    for (version, name, checksum) in &applied {
        let migration = migrations
            .binary_search_by_key(version, |migration| migration.version)
            .ok()
            .map(|index| &migrations[index])
            .filter(|migration| migration.accepts_checksum(checksum));
        let Some(migration) = migration else {
            bail!("checksum mismatch for applied migration {version}");
        };
        if migration.name != *name || migration.checksum != *checksum {
            history_updates.push(migration);
        }
    }

    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
        version INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        checksum TEXT NOT NULL,
        applied_at INTEGER NOT NULL
      ) STRICT;",
    )?;
    if !history_updates.is_empty() {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for migration in history_updates {
            transaction.execute(
                "UPDATE schema_migrations SET name = ?, checksum = ? WHERE version = ?",
                params![migration.name, migration.checksum, migration.version],
            )?;
        }
        transaction.commit()?;
    }

    let mut applied_versions = Vec::new();
    for migration in migrations
        .iter()
        .filter(|migration| migration.version > current_version)
    {
        if migration.foreign_keys_off {
            let result = (|| {
                connection
                    .execute_batch("PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON;")?;
                apply(connection, migration, applied_at)
            })();
            // The transaction has already committed or rolled back; the foreign_keys
            // pragma has no effect inside a transaction.
            let restored = connection
                .execute_batch("PRAGMA legacy_alter_table = OFF; PRAGMA foreign_keys = ON;");
            result?;
            restored?;
        } else {
            apply(connection, migration, applied_at)?;
        }
        applied_versions.push(migration.version);
    }

    Ok(MigrationReport {
        current_version: applied_versions.last().copied().unwrap_or(current_version),
        applied_versions,
    })
}

fn apply(connection: &mut Connection, migration: &Migration, applied_at: i64) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(&migration.sql)?;
    if migration.foreign_keys_off {
        check_foreign_keys(&transaction, migration.version)?;
    }
    transaction.execute(
        "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?, ?, ?, ?)",
        params![
            migration.version,
            migration.name,
            migration.checksum,
            applied_at
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn check_foreign_keys(transaction: &Transaction<'_>, version: i64) -> Result<()> {
    let mut statement = transaction.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    let mut examples = Vec::new();
    while let Some(row) = rows.next()? {
        count += 1;
        if examples.len() < 5 {
            examples.push(serde_json::json!({
                "table": row.get::<_, String>(0)?,
                "rowid": row.get::<_, Option<i64>>(1)?,
                "parent": row.get::<_, String>(2)?,
                "fkid": row.get::<_, i64>(3)?,
            }));
        }
    }
    if count > 0 {
        let examples = serde_json::to_string(&examples)?;
        bail!("migration {version} left {count} foreign key violation(s): {examples}");
    }
    Ok(())
}

fn validate_catalog(migrations: &[Migration]) -> Result<()> {
    let mut previous = 0;
    for migration in migrations {
        let Migration {
            version,
            name,
            sql,
            checksum,
            legacy_checksum,
            ..
        } = migration;
        ensure!(
            (1..=9_007_199_254_740_991).contains(version),
            "invalid migration version {version}"
        );
        ensure!(
            *version > previous,
            "duplicate or unordered migration version {version}"
        );
        ensure!(
            !name.trim().is_empty() && !sql.trim().is_empty(),
            "migration {version} must have a name and SQL"
        );
        let content = Sha256::digest(format!("{version}\0{sql}"));
        let legacy = Sha256::digest(format!("{version}\0{name}\0{sql}"));
        ensure!(
            *checksum == format!("v2:{content:x}") && *legacy_checksum == format!("{legacy:x}"),
            "checksum mismatch in bundled migration {version}"
        );
        previous = *version;
    }
    Ok(())
}
