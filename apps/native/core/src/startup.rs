//! Frozen database bootstrap policy. All callers supply an isolated/resolved data root.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, ErrorCode, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

use crate::{
    backup::{BackupDescriptor, BackupReason, BackupService, DEFAULT_MAX_BACKUPS},
    ownership::{DatabaseOwner, OwnerRecoveryError, assert_no_live_owner_read_only},
    recovery_marker::{self, RecoveryMarker},
    storage::{configure_connection, migrate_with_backup, supported_schema_version},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadOnlyReason {
    FilesystemReadOnly,
    NewerSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRequired {
    #[serde(skip)]
    pub(crate) marker: Option<RecoveryMarker>,
    pub recovery_id: String,
    pub reason: String,
    pub durable_database_path: PathBuf,
    pub quarantined_path: PathBuf,
    pub marker_path: PathBuf,
    pub backups: Vec<BackupDescriptor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ownership_issue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_list_error: Option<RecoveryError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker_error: Option<RecoveryError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub move_error: Option<RecoveryError>,
}

pub enum BootstrapResult {
    // Connection precedes the lease so normal destruction closes SQLite first.
    Writable {
        connection: Connection,
        ownership: DatabaseOwner,
        data_root: PathBuf,
    },
    ReadOnly {
        connection: Connection,
        data_root: PathBuf,
        reason: ReadOnlyReason,
    },
    RecoveryRequired(RecoveryRequired),
}

pub fn open(data_root: &Path, runtime_generation: &str) -> Result<BootstrapResult> {
    let root = lexical_absolute(data_root)?;
    let database_path = root.join("matou.sqlite");
    let mut backups = BackupService::new(&root)?;
    if let Some(pending) = recovery_marker::load_pending(&database_path)? {
        let mut result = recovery_result(pending.marker, &backups);
        result.marker_error = pending
            .marker_error
            .map(|message| recovery_error("RECOVERY_MARKER_FAILED", message));
        return Ok(BootstrapResult::RecoveryRequired(result));
    }

    let mut ownership = None;
    let attempt = (|| {
        let mut integrity_verified = false;
        if database_path.exists() {
            let write_allowed = is_writable(&root) && is_writable(&database_path);
            if !write_allowed {
                assert_no_live_owner_read_only(&database_path)?;
                assert_wal_bundle_ready(&database_path)?;
                let connection = open_recovery_read_only(&database_path)?;
                assert_startup_integrity(&connection)?;
                return Ok(BootstrapResult::ReadOnly {
                    connection,
                    data_root: root.clone(),
                    reason: ReadOnlyReason::FilesystemReadOnly,
                });
            }
            // Classify a newer database before taking a lease or replacing stale
            // ownership evidence. Any inconclusive inspection follows normal startup.
            if assert_no_live_owner_read_only(&database_path).is_ok()
                && let Ok(connection) = open_recovery_read_only(&database_path)
                && read_schema_version(&connection).is_ok_and(|version| {
                    supported_schema_version().is_ok_and(|supported| version > supported)
                })
            {
                assert_no_live_owner_read_only(&database_path)?;
                if let Some(marker) = recovery_marker::read_active(&database_path)? {
                    return Ok(BootstrapResult::RecoveryRequired(recovery_result(
                        marker, &backups,
                    )));
                }
                assert_wal_bundle_ready(&database_path)?;
                assert_startup_integrity(&connection)?;
                return Ok(BootstrapResult::ReadOnly {
                    connection,
                    data_root: root.clone(),
                    reason: ReadOnlyReason::NewerSchema,
                });
            }
            match DatabaseOwner::acquire(&database_path, runtime_generation) {
                Ok(lease) => {
                    ownership = Some(lease);
                    if let Some(marker) = recovery_marker::read_active(&database_path)? {
                        release(&mut ownership)?;
                        return Ok(BootstrapResult::RecoveryRequired(recovery_result(
                            marker, &backups,
                        )));
                    }
                }
                Err(error) => {
                    if let Some(marker) = wait_for_recovery_marker(&database_path)? {
                        return Ok(BootstrapResult::RecoveryRequired(recovery_result(
                            marker, &backups,
                        )));
                    }
                    return Err(error);
                }
            }
            assert_wal_bundle_ready(&database_path)?;
            let inspection = open_recovery_read_only(&database_path)?;
            assert_startup_integrity(&inspection)?;
            integrity_verified = true;
            let version = read_schema_version(&inspection)?;
            if version > supported_schema_version()? {
                release(&mut ownership)?;
                return Ok(BootstrapResult::ReadOnly {
                    connection: inspection,
                    data_root: root.clone(),
                    reason: ReadOnlyReason::NewerSchema,
                });
            }
        } else {
            ownership = Some(DatabaseOwner::acquire(&database_path, runtime_generation)?);
            if let Some(marker) = recovery_marker::read_active(&database_path)? {
                release(&mut ownership)?;
                return Ok(BootstrapResult::RecoveryRequired(recovery_result(
                    marker, &backups,
                )));
            }
            if database_path.exists() {
                release(&mut ownership)?;
                return open(&root, runtime_generation);
            }
        }
        let lease = ownership
            .as_ref()
            .context("missing writable database lease")?;
        let connection = open_writable(
            &database_path,
            runtime_generation,
            lease,
            &mut backups,
            !integrity_verified,
            OpenFlags::default(),
        )?;
        Ok(BootstrapResult::Writable {
            connection,
            ownership: ownership
                .take()
                .context("missing writable database lease")?,
            data_root: root.clone(),
        })
    })();

    match attempt {
        Ok(result) => Ok(result),
        Err(error) => {
            if let Some(error) = error.downcast_ref::<OwnerRecoveryError>() {
                release(&mut ownership)?;
                return publish_recovery(
                    &database_path,
                    &backups,
                    "ownership-recovery-required",
                    Some(error.issue),
                    false,
                );
            }
            if let Some(reason) = recovery_reason(&error) {
                let result = publish_recovery(&database_path, &backups, reason, None, true);
                release(&mut ownership)?;
                return result;
            }
            if is_write_denied(&error) || is_newer_schema(&error) {
                let readonly = (|| {
                    let connection = open_recovery_read_only(&database_path)?;
                    assert_startup_integrity(&connection)?;
                    Ok::<_, anyhow::Error>(connection)
                })();
                match readonly {
                    Ok(connection) => {
                        release(&mut ownership)?;
                        return Ok(BootstrapResult::ReadOnly {
                            connection,
                            data_root: root,
                            reason: if is_newer_schema(&error) {
                                ReadOnlyReason::NewerSchema
                            } else {
                                ReadOnlyReason::FilesystemReadOnly
                            },
                        });
                    }
                    Err(read_error) => {
                        let result = if let Some(reason) = recovery_reason(&read_error) {
                            publish_recovery(&database_path, &backups, reason, None, true)
                        } else {
                            Err(read_error)
                        };
                        release(&mut ownership)?;
                        return result;
                    }
                }
            }
            release(&mut ownership)?;
            Err(error)
        }
    }
}

pub(crate) fn open_writable(
    database_path: &Path,
    generation: &str,
    _lease: &DatabaseOwner,
    backups: &mut BackupService,
    check_integrity: bool,
    flags: OpenFlags,
) -> Result<Connection> {
    let mut connection = Connection::open_with_flags(database_path, flags)?;
    if check_integrity {
        assert_startup_integrity(&connection)?;
    }
    configure_connection(&connection)?;
    connection.execute(
        "INSERT OR REPLACE INTO _runtime_meta (key,value) VALUES ('runtime_generation',?)",
        [generation],
    )?;
    migrate_with_backup(&mut connection, now(), |connection| {
        backups.create(connection, BackupReason::PreMigration, now())?;
        backups.rotate(DEFAULT_MAX_BACKUPS)
    })?;
    Ok(connection)
}

fn publish_recovery(
    path: &Path,
    backups: &BackupService,
    reason: &str,
    issue: Option<&str>,
    move_bundle: bool,
) -> Result<BootstrapResult> {
    let timestamp = now();
    let proposed = RecoveryMarker {
        version: 1,
        state: "required".into(),
        recovery_id: Uuid::new_v4().to_string(),
        reason: reason.into(),
        durable_database_path: path.to_owned(),
        quarantined_path: if move_bundle {
            append_suffix(path, &format!(".corrupt-{timestamp}"))
        } else {
            path.to_owned()
        },
        marker_path: append_suffix(path, ".recovery.json"),
        created_at: timestamp,
        ownership_issue: issue.map(str::to_owned),
    };
    let published = match recovery_marker::publish(proposed) {
        Ok(marker) => marker,
        Err(error) => {
            let Some(marker) = recovery_marker::durable_after_failure(path, &error)? else {
                return Err(error);
            };
            let mut result = recovery_result(marker, backups);
            result.quarantined_path = path.to_owned();
            result.marker_error = Some(recovery_error("RECOVERY_MARKER_FAILED", error.to_string()));
            return Ok(BootstrapResult::RecoveryRequired(result));
        }
    };
    let mut move_error = None;
    if move_bundle {
        if let Err(error) = move_database_bundle(path, &published.quarantined_path) {
            move_error = Some(recovery_error("RECOVERY_MOVE_FAILED", error.to_string()));
        }
    }
    let mut result = recovery_result(published, backups);
    result.move_error = move_error;
    Ok(BootstrapResult::RecoveryRequired(result))
}

fn recovery_result(marker: RecoveryMarker, backups: &BackupService) -> RecoveryRequired {
    let (backups, backup_list_error) = match backups.list_valid() {
        Ok(backups) => (backups, None),
        Err(error) => (
            Vec::new(),
            Some(recovery_error("BACKUP_LIST_FAILED", error.to_string())),
        ),
    };
    RecoveryRequired {
        marker: Some(marker.clone()),
        recovery_id: marker.recovery_id,
        reason: marker.reason,
        durable_database_path: marker.durable_database_path,
        quarantined_path: marker.quarantined_path,
        marker_path: marker.marker_path,
        backups,
        ownership_issue: marker.ownership_issue,
        backup_list_error,
        marker_error: None,
        move_error: None,
    }
}

fn recovery_error(code: &str, message: String) -> RecoveryError {
    RecoveryError {
        code: code.into(),
        message,
        retryable: true,
    }
}

fn move_database_bundle(path: &Path, destination: &Path) -> io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let source = append_suffix(path, suffix);
        if source.exists() {
            fs::rename(source, append_suffix(destination, suffix))?;
        }
    }
    Ok(())
}

fn release(owner: &mut Option<DatabaseOwner>) -> Result<()> {
    if let Some(mut owner) = owner.take() {
        owner.release()?;
    }
    Ok(())
}

pub fn assert_startup_integrity(connection: &Connection) -> Result<()> {
    let mut statement = connection.prepare("PRAGMA quick_check")?;
    let results = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        results.len() == 1 && results[0].eq_ignore_ascii_case("ok"),
        "database corrupt: quick_check failed: {}",
        results
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ")
    );
    Ok(())
}

fn read_schema_version(connection: &Connection) -> Result<i64> {
    let exists: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')", [], |row| row.get(0))?;
    if !exists {
        return Ok(0);
    }
    Ok(connection.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?)
}

pub fn open_read_only(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let inspected = (|| {
        connection.execute_batch("PRAGMA query_only=ON;")?;
        connection.query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))?;
        Ok::<_, rusqlite::Error>(())
    })();
    match inspected {
        Ok(()) => return Ok(connection),
        Err(error) if error.sqlite_error_code() == Some(ErrorCode::ReadOnly) => {}
        Err(error) => return Err(error.into()),
    }
    drop(connection);
    if committed_wal(path) {
        let error = WalRecoveryError("database WAL requires recovery before read-only open".into());
        return Err(error.into());
    }
    open_read_only_uri(path, "immutable=1")
}

fn open_recovery_read_only(path: &Path) -> Result<Connection> {
    // SQLite may update read marks or rebuild a writable SHM even with a
    // read-only main handle. Force read-only SHM for a committed WAL. With
    // no WAL to replay, immutable mode avoids opening or creating sidecars.
    let options = if committed_wal(path) {
        "readonly_shm=1"
    } else {
        "immutable=1"
    };
    open_read_only_uri(path, options)
}

fn open_read_only_uri(path: &Path, options: &str) -> Result<Connection> {
    let path_text = path.to_str().context("SQLite path is not valid Unicode")?;
    #[cfg(windows)]
    let path_text = path_text.replace('\\', "/");
    let mut uri = String::from("file:");
    for byte in path_text.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/:.-_~".contains(byte) {
            uri.push(char::from(*byte));
        } else {
            use std::fmt::Write;
            write!(uri, "%{byte:02X}")?;
        }
    }
    uri.push('?');
    uri.push_str(options);
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    connection.execute_batch("PRAGMA query_only=ON;")?;
    Ok(connection)
}

#[derive(Debug)]
struct WalRecoveryError(String);
impl std::fmt::Display for WalRecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for WalRecoveryError {}

fn committed_wal(path: &Path) -> bool {
    fs::metadata(append_suffix(path, "-wal")).is_ok_and(|metadata| metadata.len() > 32)
}

pub(crate) fn assert_wal_bundle_ready(path: &Path) -> Result<()> {
    if !committed_wal(path) {
        return Ok(());
    }
    let shm = append_suffix(path, "-shm");
    let bytes = (|| {
        let metadata = fs::metadata(&shm)?;
        ensure!(
            metadata.is_file() && metadata.len() >= 32_768,
            "SHM is truncated"
        );
        Ok::<_, anyhow::Error>(fs::read(&shm)?)
    })()
    .map_err(|error| WalRecoveryError(format!("committed WAL has no readable SHM: {error}")))?;
    ensure!(
        bytes.len() >= 96,
        WalRecoveryError("committed WAL has a corrupt SHM header".into())
    );
    if u32::from_le_bytes(bytes[0..4].try_into().unwrap()) != 3_007_000
        || bytes[0..48] != bytes[48..96]
    {
        return Err(WalRecoveryError("committed WAL has a corrupt SHM header".into()).into());
    }
    Ok(())
}

fn recovery_reason(error: &anyhow::Error) -> Option<&'static str> {
    let text = format!("{error:#}").to_lowercase();
    if error.chain().any(|cause| cause.is::<WalRecoveryError>())
        || text.contains("wal requires recovery")
    {
        return Some("wal-recovery-required");
    }
    if [
        "file is not a database",
        "database disk image is malformed",
        "database corrupt",
        "integrity check failed",
    ]
    .iter()
    .any(|part| text.contains(part))
    {
        Some("physical-corruption")
    } else {
        None
    }
}

fn is_write_denied(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}").to_lowercase();
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
    }) || ["readonly", "read-only", "permission denied"]
        .iter()
        .any(|part| text.contains(part))
}
fn is_newer_schema(error: &anyhow::Error) -> bool {
    let text = error.to_string();
    text.contains("database schema version ") && text.contains(" is newer than supported version ")
}

fn wait_for_recovery_marker(path: &Path) -> Result<Option<RecoveryMarker>> {
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        if let Some(marker) = recovery_marker::read_active(path)? {
            return Ok(Some(marker));
        }
        if Instant::now() >= deadline {
            return recovery_marker::read_active(path);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
pub(crate) fn is_writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 })
}
#[cfg(windows)]
pub(crate) fn is_writable(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    unsafe extern "C" {
        fn _waccess(path: *const u16, mode: i32) -> i32;
    }
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe { _waccess(path.as_ptr(), 2) == 0 }
}

pub(crate) fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    path.into()
}
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn lexical_absolute(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    Ok(resolved)
}
