//! Exclusive ownership of a runtime database, before SQLite is opened.
use std::{
    fmt,
    fs::{self, Metadata, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior};
use serde::{Deserialize, Serialize};

const CONFLICT: &str = "database is already owned by a live Runtime";
const RECOVERY_CODE: &str = "OWNERSHIP_RECOVERY_REQUIRED";
const MAX_SAFE_PID: f64 = 9_007_199_254_740_991.0;
static NEXT_CLAIM: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerRecord {
    pub pid: u64,
    pub runtime_generation: String,
}

#[derive(Debug)]
pub struct OwnerRecoveryError {
    pub code: &'static str,
    pub issue: &'static str,
    pub state_path: PathBuf,
    cause: anyhow::Error,
}

impl OwnerRecoveryError {
    fn new(issue: &'static str, path: &Path, cause: impl Into<anyhow::Error>) -> Self {
        Self {
            code: RECOVERY_CODE,
            issue,
            state_path: path.to_owned(),
            cause: cause.into(),
        }
    }
}

impl fmt::Display for OwnerRecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "database ownership state requires recovery: {} at {}: {}",
            self.issue,
            self.state_path.display(),
            self.cause
        )
    }
}

impl std::error::Error for OwnerRecoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

#[derive(Debug)]
pub struct DatabaseOwner {
    owner_path: PathBuf,
    runtime_generation: String,
    released: bool,
}

impl DatabaseOwner {
    pub fn acquire(database_path: &Path, generation: &str) -> Result<Self> {
        ensure!(!generation.is_empty(), "owner runtime generation is empty");
        fs::create_dir_all(parent(database_path))?;
        let owner_path = append_suffix(database_path, ".owner");
        let record = OwnerRecord {
            pid: std::process::id().into(),
            runtime_generation: generation.to_owned(),
        };
        match publish(&owner_path, &record) {
            Ok(()) => assert_canonical(&owner_path, &record)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                match inspect(&owner_path) {
                    Inspection::Absent => return Err(anyhow!(CONFLICT)),
                    Inspection::Valid(_, existing) if is_live(existing.pid) => {
                        return Err(anyhow!(CONFLICT));
                    }
                    _ => take_over(&owner_path, &record)?,
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(Self {
            owner_path,
            runtime_generation: record.runtime_generation,
            released: false,
        })
    }

    pub fn release(&mut self) -> Result<()> {
        if self.released {
            return Ok(());
        }
        release_matching(&self.owner_path, &self.runtime_generation)?;
        self.released = true;
        Ok(())
    }
}

impl Drop for DatabaseOwner {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

pub fn read_database_owner(owner_path: &Path) -> Option<OwnerRecord> {
    match inspect(owner_path) {
        Inspection::Valid(_, record) => Some(record),
        _ => None,
    }
}

/// Read-only startup checks the live owner before inspecting user data.
pub fn assert_no_live_owner(database_path: &Path) -> Result<()> {
    check_no_live_owner(database_path, true)
}

pub(crate) fn assert_no_live_owner_read_only(database_path: &Path) -> Result<()> {
    check_no_live_owner(database_path, false)
}

fn check_no_live_owner(database_path: &Path, repair: bool) -> Result<()> {
    let owner_path = append_suffix(database_path, ".owner");
    if !owner_path.exists() {
        return Ok(());
    }
    match inspect_owner(&owner_path, repair) {
        Inspection::Malformed(error) => {
            Err(OwnerRecoveryError::new("owner-record-malformed", &owner_path, error).into())
        }
        Inspection::Absent => Err(anyhow!(CONFLICT)),
        Inspection::Valid(_, record) if is_live(record.pid) => Err(anyhow!(CONFLICT)),
        Inspection::Valid(..) => Ok(()),
    }
}

/// The existing recovery-action fence is separate from the live runtime lease.
/// Operation errors retain their original cause (unlike stale-owner takeover).
pub(crate) fn with_recovery_action_fence<T>(
    database_path: &Path,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let path = append_suffix(database_path, ".recovery-action.sqlite");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => repair_mode(&path, &metadata),
        Ok(_) => {
            return Err(OwnerRecoveryError::new(
                "takeover-sidecar-unusable",
                &path,
                anyhow!("takeover sidecar is not a regular file"),
            )
            .into());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(OwnerRecoveryError::new("takeover-sidecar-unusable", &path, error).into());
        }
    }
    let sqlite_path = parent(&path).canonicalize()?.join(
        path.file_name()
            .context("missing recovery fence filename")?,
    );
    let mut connection = Connection::open_with_flags(
        sqlite_path,
        OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    repair_mode(&path, &fs::symlink_metadata(&path)?);
    connection.busy_timeout(Duration::from_millis(5000))?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
    let result = operation()?;
    transaction.commit()?;
    connection.close().map_err(|(_, error)| error)?;
    Ok(result)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OwnerFormat {
    File,
    LegacyDirectory,
}

enum Inspection {
    Absent,
    Valid(OwnerFormat, OwnerRecord),
    Malformed(anyhow::Error),
}

fn inspect(path: &Path) -> Inspection {
    inspect_owner(path, true)
}

fn inspect_owner(path: &Path, repair: bool) -> Inspection {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Inspection::Absent,
        Err(error) => return Inspection::Malformed(error.into()),
    };
    let (format, payload) = if metadata.is_dir() {
        (
            OwnerFormat::LegacyDirectory,
            fs::read(path.join("owner.json")),
        )
    } else if metadata.is_file() {
        if repair {
            repair_mode(path, &metadata);
        }
        (OwnerFormat::File, fs::read(path))
    } else {
        return Inspection::Malformed(anyhow!("owner path is not a regular file"));
    };
    match payload
        .map_err(anyhow::Error::from)
        .and_then(|bytes| parse(&bytes))
    {
        Ok(record) => Inspection::Valid(format, record),
        Err(error) => Inspection::Malformed(error),
    }
}

fn parse(bytes: &[u8]) -> Result<OwnerRecord> {
    let text = String::from_utf8_lossy(bytes);
    let value: serde_json::Value = serde_json::from_str(&text)?;
    let invalid = || anyhow!("owner payload is invalid");
    let pid = value
        .get("pid")
        .and_then(serde_json::Value::as_f64)
        .filter(|&v| v > 0.0 && v <= MAX_SAFE_PID && v.fract() == 0.0)
        .ok_or_else(invalid)?;
    let generation = value
        .get("runtimeGeneration")
        .and_then(serde_json::Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(invalid)?;
    Ok(OwnerRecord {
        pid: pid as u64,
        runtime_generation: generation.to_owned(),
    })
}

fn assert_canonical(path: &Path, record: &OwnerRecord) -> Result<()> {
    ensure!(
        matches!(inspect(path), Inspection::Valid(OwnerFormat::File, actual) if &actual == record),
        "database ownership claim lost before publication"
    );
    Ok(())
}

struct Claim(PathBuf);

impl Drop for Claim {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn publish(path: &Path, record: &OwnerRecord) -> io::Result<()> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let sequence = NEXT_CLAIM.fetch_add(1, Ordering::Relaxed);
    let temporary = append_suffix(path, &format!(".claim-{}-{stamp}-{sequence}", record.pid));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&temporary)?;
    let claim = Claim(temporary);
    let mut file = file;
    serde_json::to_writer(&mut file, record).map_err(io::Error::other)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::hard_link(&claim.0, path)?;
    sync_directory(parent(path))
}

fn take_over(owner_path: &Path, record: &OwnerRecord) -> Result<()> {
    let sidecar = append_suffix(owner_path, ".takeover.sqlite");
    let result = with_takeover_lock(&sidecar, || {
        remove_legacy_takeover(&append_suffix(owner_path, ".takeover"))?;
        match inspect(owner_path) {
            Inspection::Absent => {}
            Inspection::Malformed(error) => {
                return Err(
                    OwnerRecoveryError::new("owner-record-malformed", owner_path, error).into(),
                );
            }
            Inspection::Valid(_, existing) if is_live(existing.pid) => {
                return Err(anyhow!(CONFLICT));
            }
            Inspection::Valid(format, _) => remove_owner(owner_path, format)?,
        }
        match publish(owner_path, record) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(anyhow!(CONFLICT)),
            other => other?,
        }
        assert_canonical(owner_path, record)
    });
    if result
        .as_ref()
        .is_err_and(|error| error.downcast_ref::<OwnerRecoveryError>().is_some())
    {
        let _ = release_matching(owner_path, &record.runtime_generation);
    }
    result
}

fn with_takeover_lock(path: &Path, operation: impl FnOnce() -> Result<()>) -> Result<()> {
    let unusable = |error| OwnerRecoveryError::new("takeover-sidecar-unusable", path, error);
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => repair_mode(path, &metadata),
        Ok(_) => return Err(unusable(anyhow!("takeover sidecar is not a regular file")).into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(unusable(error.into()).into()),
    }
    let filename = path
        .file_name()
        .ok_or_else(|| unusable(anyhow!("missing sidecar filename")))?;
    // SQLite NOFOLLOW checks every component. Resolve existing parent aliases
    // but leave the final component for SQLite to validate.
    let sqlite_path = parent(path)
        .canonicalize()
        .map_err(|e| unusable(e.into()))?
        .join(filename);
    let flags = OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let mut connection =
        Connection::open_with_flags(sqlite_path, flags).map_err(|e| unusable(e.into()))?;
    let metadata = fs::symlink_metadata(path).map_err(|e| unusable(e.into()))?;
    if !metadata.is_file() {
        return Err(unusable(anyhow!("takeover sidecar is not a regular file")).into());
    }
    repair_mode(path, &metadata);
    connection
        .busy_timeout(Duration::from_millis(5000))
        .map_err(|e| unusable(e.into()))?;
    let transaction = match connection.transaction_with_behavior(TransactionBehavior::Exclusive) {
        Ok(transaction) => transaction,
        Err(error)
            if matches!(
                error.sqlite_error_code(),
                Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
            ) =>
        {
            return Err(error.into());
        }
        Err(error) => return Err(unusable(error.into()).into()),
    };
    if let Err(error) = operation() {
        // Transaction's Drop rolls back without replacing the primary error.
        if error.downcast_ref::<OwnerRecoveryError>().is_some() || error.to_string() == CONFLICT {
            return Err(error);
        }
        return Err(unusable(error).into());
    }
    transaction.commit().map_err(|e| unusable(e.into()))?;
    connection
        .close()
        .map_err(|(_, error)| unusable(error.into()))?;
    Ok(())
}

fn release_matching(path: &Path, generation: &str) -> Result<()> {
    if matches!(inspect(path), Inspection::Valid(OwnerFormat::File, record)
        if record.runtime_generation == generation)
    {
        remove_owner(path, OwnerFormat::File).with_context(|| {
            format!("failed to remove database owner lock at {}", path.display())
        })?;
        sync_directory(parent(path))?;
    }
    Ok(())
}

fn remove_owner(path: &Path, format: OwnerFormat) -> io::Result<()> {
    let result = match format {
        OwnerFormat::File => fs::remove_file(path),
        OwnerFormat::LegacyDirectory => fs::remove_dir_all(path),
    };
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn remove_legacy_takeover(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => remove_owner(path, OwnerFormat::LegacyDirectory),
        Ok(_) => remove_owner(path, OwnerFormat::File),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut result = path.as_os_str().to_owned();
    result.push(suffix);
    result.into()
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
fn repair_mode(path: &Path, metadata: &Metadata) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if metadata.uid() == unsafe { libc::getuid() } && metadata.mode() & 0o777 != 0o600 {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

#[cfg(windows)]
fn repair_mode(_path: &Path, _metadata: &Metadata) {
    // Node exposes no getuid on Windows. The inherited directory ACL applies.
}

#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
    // CreateFileW needs BACKUP_SEMANTICS for directories and GENERIC_WRITE
    // for FlushFileBuffers. Preserve any filesystem or access error.
    OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

#[cfg(unix)]
fn is_live(pid: u64) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    alive || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn is_live(pid: u64) -> bool {
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_ACCESS_DENIED, STILL_ACTIVE, WAIT_TIMEOUT},
        System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
        },
    };

    let Ok(pid) = u32::try_from(pid) else {
        return false;
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED as i32);
    }
    let mut code = 0;
    let live = if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
        io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED as i32)
    } else {
        code == STILL_ACTIVE as u32 && unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT
    };
    unsafe { CloseHandle(handle) };
    live
}
