use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, MAIN_DB, OpenFlags, backup::NO_PROGRESS};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const DEFAULT_MAX_BACKUPS: usize = 7;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackupReason {
    PreMigration,
    CleanExit,
}

impl BackupReason {
    fn name(self) -> &'static str {
        match self {
            Self::PreMigration => "pre-migration",
            Self::CleanExit => "clean-exit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupDescriptor {
    pub id: String,
    pub path: PathBuf,
    pub created_at: i64,
    pub reason: BackupReason,
    pub schema_version: i64,
    pub size: u64,
    pub sha256: String,
}

pub struct BackupService {
    directory: PathBuf,
    last_timestamp: i64,
}

impl BackupService {
    /// The caller resolves the data root before acquiring runtime ownership.
    pub fn new(data_root: &Path) -> Result<Self> {
        ensure!(data_root.is_absolute(), "backup data root must be absolute");
        Ok(Self {
            directory: data_root.join("backups"),
            last_timestamp: 0,
        })
    }

    pub fn create(
        &mut self,
        connection: &Connection,
        reason: BackupReason,
        now: i64,
    ) -> Result<BackupDescriptor> {
        fs::create_dir_all(&self.directory)?;
        let created_at = self.next_timestamp(now)?;
        let schema_version = read_schema_version(connection)?;
        let id = format!("matou-{created_at}-{}-v{schema_version}", reason.name());
        let path = self.directory.join(format!("{id}.sqlite"));
        let manifest = self.directory.join(format!("{id}.json"));
        let partial = add_suffix(&path, ".partial");
        let partial_manifest = add_suffix(&manifest, ".partial");

        let result = (|| {
            connection.backup(MAIN_DB, &partial, NO_PROGRESS)?;
            assert_integrity(&partial)?;
            let (size, sha256) = hash_file(&partial)?;
            let descriptor = BackupDescriptor {
                id,
                path,
                created_at,
                reason,
                schema_version,
                size,
                sha256,
            };
            fs::rename(&partial, &descriptor.path)?;
            write_manifest(&partial_manifest, &descriptor)?;
            fs::rename(&partial_manifest, manifest)?;
            Ok(descriptor)
        })();
        if result.is_err() {
            let _ = remove_file_if_present(&partial);
            let _ = remove_file_if_present(&partial_manifest);
        }
        result
    }

    pub fn list_valid(&self) -> Result<Vec<BackupDescriptor>> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };

        let mut valid = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                continue;
            };
            let Some((created_at, reason, schema_version)) = parse_backup_id(id) else {
                continue;
            };
            let path = self.directory.join(format!("{id}.sqlite"));
            let Ok(descriptor) = (|| -> Result<BackupDescriptor> {
                Ok(serde_json::from_reader(File::open(entry.path())?)?)
            })() else {
                continue;
            };
            if descriptor.id != id
                || descriptor.path.as_os_str() != path.as_os_str()
                || descriptor.created_at != created_at
                || descriptor.reason != reason
                || descriptor.schema_version != schema_version
                || descriptor.size > MAX_SAFE_INTEGER
                || !is_sha256(&descriptor.sha256)
            {
                continue;
            }
            if verify_backup(&path, &descriptor).is_ok() {
                valid.push(descriptor);
            }
        }
        valid.sort_unstable_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(valid)
    }

    pub fn rotate(&self, max_count: usize) -> Result<()> {
        ensure!(
            max_count as u64 <= MAX_SAFE_INTEGER,
            "backup retention count must be a non-negative integer"
        );
        for descriptor in self.list_valid()?.into_iter().skip(max_count) {
            remove_file_if_present(&descriptor.path)?;
            remove_file_if_present(&self.directory.join(format!("{}.json", descriptor.id)))?;
        }
        Ok(())
    }

    pub fn restore(&mut self, id: &str, target: &Path, now: i64) -> Result<()> {
        self.restore_with_rename(id, target, now, |from, to| fs::rename(from, to))
    }

    fn restore_with_rename(
        &mut self,
        id: &str,
        target: &Path,
        now: i64,
        mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
    ) -> Result<()> {
        let descriptor = self
            .list_valid()?
            .into_iter()
            .find(|descriptor| descriptor.id == id)
            .with_context(|| format!("backup {id} is missing or invalid"))?;
        let target = lexical_absolute(target)?;
        fs::create_dir_all(
            target
                .parent()
                .context("backup target has no parent directory")?,
        )?;
        let restored_at = self.next_timestamp(now)?;
        let temporary = add_suffix(
            &target,
            &format!(".restore-{restored_at}-{}.partial", Uuid::new_v4()),
        );
        let replaced = add_suffix(&target, &format!(".replaced-{restored_at}"));
        let replaced_partial = add_suffix(&replaced, ".partial");

        let result = (|| {
            // The selected backup is a verified, independently readable file.
            fs::copy(&descriptor.path, &temporary)?;
            verify_backup(&temporary, &descriptor)?;
            let target_exists = match fs::metadata(&target) {
                Ok(metadata) => metadata.is_file(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };

            if target_exists {
                let old = Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                old.backup(MAIN_DB, &replaced_partial, NO_PROGRESS)?;
                drop(old);
                assert_integrity(&replaced_partial)?;
                hash_file(&replaced_partial)?;
                rename(&replaced_partial, &replaced)?;
            }

            if let Err(error) = rename(&temporary, &target) {
                if !target_exists {
                    let fallback = (|| {
                        fs::hard_link(&temporary, &target)?;
                        verify_backup(&target, &descriptor)?;
                        remove_sidecars(&target)
                    })();
                    if let Err(fallback_error) = fallback {
                        return Err(anyhow::Error::from(error))
                            .context(format!("restore fallback also failed: {fallback_error:#}"));
                    }
                }
                return Err(error.into());
            }
            remove_sidecars(&target)
        })();
        let _ = remove_file_if_present(&temporary);
        let _ = remove_file_if_present(&replaced_partial);
        result
    }

    fn next_timestamp(&mut self, now: i64) -> Result<i64> {
        let after_previous = self
            .last_timestamp
            .checked_add(1)
            .context("backup timestamp overflow")?;
        self.last_timestamp = now.max(after_previous);
        Ok(self.last_timestamp)
    }
}

fn parse_backup_id(id: &str) -> Option<(i64, BackupReason, i64)> {
    let (time, rest) = id.strip_prefix("matou-")?.split_once('-')?;
    let (reason, schema) = rest.rsplit_once("-v")?;
    let reason = match reason {
        "pre-migration" => BackupReason::PreMigration,
        "clean-exit" => BackupReason::CleanExit,
        _ => return None,
    };
    if !time.bytes().all(|byte| byte.is_ascii_digit())
        || !schema.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((time.parse().ok()?, reason, schema.parse().ok()?))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn read_schema_version(connection: &Connection) -> Result<i64> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    let version: Option<i64> =
        connection.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    Ok(version.unwrap_or(0))
}

fn assert_integrity(path: &Path) -> Result<()> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut integrity_check = connection.prepare("PRAGMA integrity_check")?;
    let mut rows = integrity_check.query([])?;
    let valid = rows
        .next()?
        .map(|row| row.get::<_, String>(0))
        .transpose()?
        .is_some_and(|value| value.eq_ignore_ascii_case("ok"));
    ensure!(
        valid && rows.next()?.is_none(),
        "SQLite integrity check failed"
    );
    Ok(())
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        let amount = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(amount) => amount,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        size += amount as u64;
        digest.update(&buffer[..amount]);
    }
    Ok((size, format!("{:x}", digest.finalize())))
}

fn verify_backup(path: &Path, descriptor: &BackupDescriptor) -> Result<()> {
    let metadata = fs::metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() == descriptor.size,
        "backup size does not match its manifest"
    );
    let (size, sha256) = hash_file(path)?;
    ensure!(
        size == descriptor.size,
        "backup size does not match its manifest"
    );
    ensure!(
        sha256 == descriptor.sha256,
        "backup checksum does not match its manifest"
    );
    assert_integrity(path)
}

fn write_manifest(path: &Path, descriptor: &BackupDescriptor) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)?
        .write_all(&serde_json::to_vec(descriptor)?)?;
    Ok(())
}

fn remove_sidecars(path: &Path) -> Result<()> {
    remove_file_if_present(&add_suffix(path, "-wal"))?;
    remove_file_if_present(&add_suffix(path, "-shm"))?;
    Ok(())
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn add_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut full = path.as_os_str().to_owned();
    full.push(suffix);
    full.into()
}

fn lexical_absolute(path: &Path) -> io::Result<PathBuf> {
    let mut absolute = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::ParentDir => {
                absolute.pop();
            }
            Component::CurDir => {}
            other => absolute.push(other),
        }
    }
    Ok(absolute)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save_value(path: &Path, value: &str) -> Result<Connection> {
        let database = Connection::open(path)?;
        database.execute_batch("CREATE TABLE rows (value TEXT NOT NULL) STRICT")?;
        database.execute("INSERT INTO rows VALUES (?)", [value])?;
        Ok(database)
    }

    fn load_value(path: &Path) -> Result<String> {
        assert_integrity(path)?;
        let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(database.query_row("SELECT value FROM rows", [], |row| row.get(0))?)
    }

    #[test]
    fn failed_restore_renames_preserve_canonical_and_backup() -> Result<()> {
        let reference: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/backup-reference.json"))?;
        let interruptions = reference["interruptions"].as_array().unwrap();
        assert_eq!(interruptions.len(), 3);
        for case in interruptions {
            let target_exists = case["targetExists"].as_bool().unwrap();
            let fail_rollback = case["phase"] == "old-rollback";
            let expected = case["canonical"]["rows"][0]["value"].as_str().unwrap();
            let injected_message =
                format!("injected {} interruption", case["phase"].as_str().unwrap());
            let directory = tempfile::tempdir()?;
            let source = save_value(&directory.path().join("source.sqlite"), "from-backup")?;
            let descriptor = BackupService::new(directory.path())?.create(
                &source,
                BackupReason::CleanExit,
                3_000,
            )?;
            let original_backup = fs::read(&descriptor.path)?;
            let target = directory.path().join("target.sqlite");
            let rollback = add_suffix(&target, ".replaced-4000");
            if target_exists {
                save_value(&target, "committed-old-wal")?
                    .close()
                    .map_err(|(_, error)| error)?;
            }
            let mut injected = false;
            let error = BackupService::new(directory.path())?
                .restore_with_rename(&descriptor.id, &target, 4_000, |from, to| {
                    if (fail_rollback && to == rollback) || (!fail_rollback && to == target) {
                        injected = true;
                        return Err(io::Error::other(injected_message.clone()));
                    }
                    fs::rename(from, to)
                })
                .unwrap_err();
            assert!(injected);
            assert_eq!(error.to_string(), case["error"].as_str().unwrap());
            assert_eq!(load_value(&target)?, expected);
            assert_eq!(load_value(&descriptor.path)?, "from-backup");
            assert_eq!(fs::read(&descriptor.path)?, original_backup);
            assert_eq!(rollback.exists(), target_exists && !fail_rollback);
            if rollback.exists() {
                assert_eq!(load_value(&rollback)?, "committed-old-wal");
            }
            for entry in fs::read_dir(directory.path())? {
                assert!(!entry?.file_name().to_string_lossy().ends_with(".partial"));
            }
        }
        Ok(())
    }
}
