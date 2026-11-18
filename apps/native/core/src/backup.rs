use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use rusqlite::{
    Connection, MAIN_DB, OpenFlags,
    backup::{Backup, NO_PROGRESS, StepResult},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const DEFAULT_MAX_BACKUPS: usize = 7;
pub const CLEAN_EXIT_BACKUP_TIMEOUT: Duration = Duration::from_millis(5_000);
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug)]
pub struct CleanExitBackupTimeout {
    timeout: Duration,
}

impl std::fmt::Display for CleanExitBackupTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "clean shutdown backup timed out after {}ms",
            self.timeout.as_millis()
        )
    }
}

impl std::error::Error for CleanExitBackupTimeout {}

#[derive(Clone, Copy)]
struct Budget {
    deadline: Instant,
    timeout: Duration,
}

impl Budget {
    fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    fn error(&self) -> CleanExitBackupTimeout {
        CleanExitBackupTimeout {
            timeout: self.timeout,
        }
    }
}

fn check_budget(budget: Option<&Budget>) -> Result<()> {
    if let Some(budget) = budget {
        if budget.expired() {
            return Err(budget.error().into());
        }
    }
    Ok(())
}

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
        self.create_with_budget(connection, reason, now, None)
    }

    /// Create and rotate with one cooperative stage budget. All work finishes on
    /// this thread before returning; the caller retains its connection and lease.
    /// A single blocking filesystem call or backup step cannot be preempted.
    pub fn backup_for_clean_exit(
        &mut self,
        connection: &Connection,
        now: i64,
        timeout: Duration,
    ) -> Result<BackupDescriptor> {
        let budget = Budget {
            deadline: Instant::now()
                .checked_add(timeout)
                .context("clean shutdown backup timeout is too large")?,
            timeout,
        };
        let descriptor =
            self.create_with_budget(connection, BackupReason::CleanExit, now, Some(&budget))?;
        #[cfg(test)]
        test_stage(TestStage::AfterCreate, &budget);
        self.rotate_with_budget(DEFAULT_MAX_BACKUPS, Some(&budget))?;
        Ok(descriptor)
    }

    fn create_with_budget(
        &mut self,
        connection: &Connection,
        reason: BackupReason,
        now: i64,
        budget: Option<&Budget>,
    ) -> Result<BackupDescriptor> {
        check_budget(budget)?;
        fs::create_dir_all(&self.directory)?;
        check_budget(budget)?;
        let created_at = self.next_timestamp(now)?;
        let schema_version = read_schema_version(connection)?;
        check_budget(budget)?;
        let id = format!("matou-{created_at}-{}-v{schema_version}", reason.name());
        let path = self.directory.join(format!("{id}.sqlite"));
        let manifest = self.directory.join(format!("{id}.json"));
        let partial = add_suffix(&path, ".partial");
        let partial_manifest = add_suffix(&manifest, ".partial");

        let result = (|| {
            online_backup(connection, &partial, budget)?;
            assert_integrity_with_budget(&partial, budget)?;
            let (size, sha256) = hash_file_with_budget(&partial, budget)?;
            let descriptor = BackupDescriptor {
                id,
                path,
                created_at,
                reason,
                schema_version,
                size,
                sha256,
            };
            #[cfg(test)]
            if let Some(budget) = budget {
                test_stage(TestStage::BeforeSqlitePublish, budget);
            }
            check_budget(budget)?;
            fs::rename(&partial, &descriptor.path)?;
            check_budget(budget)?;
            write_manifest(&partial_manifest, &descriptor)?;
            #[cfg(test)]
            if let Some(budget) = budget {
                test_stage(TestStage::BeforeManifestPublish, budget);
            }
            check_budget(budget)?;
            fs::rename(&partial_manifest, manifest)?;
            check_budget(budget)?;
            Ok(descriptor)
        })();
        if result.is_err() {
            let _ = remove_file_if_present(&partial);
            let _ = remove_file_if_present(&partial_manifest);
        }
        result
    }

    pub fn list_valid(&self) -> Result<Vec<BackupDescriptor>> {
        self.list_valid_with_budget(None)
    }

    fn list_valid_with_budget(&self, budget: Option<&Budget>) -> Result<Vec<BackupDescriptor>> {
        check_budget(budget)?;
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                check_budget(budget)?;
                return Ok(Vec::new());
            }
            Err(error) => return Err(error.into()),
        };

        let mut valid = Vec::new();
        for entry in entries {
            check_budget(budget)?;
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
                check_budget(budget)?;
                continue;
            };
            check_budget(budget)?;
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
            match verify_backup_with_budget(&path, &descriptor, budget) {
                Ok(()) => valid.push(descriptor),
                Err(error) if error.is::<CleanExitBackupTimeout>() => return Err(error),
                Err(_) => {}
            }
            check_budget(budget)?;
        }
        valid.sort_unstable_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        check_budget(budget)?;
        Ok(valid)
    }

    pub fn rotate(&self, max_count: usize) -> Result<()> {
        self.rotate_with_budget(max_count, None)
    }

    fn rotate_with_budget(&self, max_count: usize, budget: Option<&Budget>) -> Result<()> {
        ensure!(
            max_count as u64 <= MAX_SAFE_INTEGER,
            "backup retention count must be a non-negative integer"
        );
        for descriptor in self
            .list_valid_with_budget(budget)?
            .into_iter()
            .skip(max_count)
        {
            #[cfg(test)]
            if let Some(budget) = budget {
                test_stage(TestStage::BeforeRotateDelete, budget);
            }
            check_budget(budget)?;
            remove_file_if_present(&descriptor.path)?;
            check_budget(budget)?;
            remove_file_if_present(&self.directory.join(format!("{}.json", descriptor.id)))?;
        }
        check_budget(budget)?;
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

fn online_backup(connection: &Connection, path: &Path, budget: Option<&Budget>) -> Result<()> {
    let Some(budget) = budget else {
        return Ok(connection.backup(MAIN_DB, path, NO_PROGRESS)?);
    };
    check_budget(Some(budget))?;
    let mut destination = Connection::open(path)?;
    // Backup drops before destination; both are gone before validation or the
    // caller's partial-file cleanup, on every error/timeout return as well.
    let backup = Backup::new(connection, &mut destination)?;
    loop {
        check_budget(Some(budget))?;
        let step = backup.step(100)?;
        #[cfg(test)]
        test_stage(TestStage::BackupStep, budget);
        check_budget(Some(budget))?;
        match step {
            StepResult::Done => return Ok(()),
            StepResult::More => {}
            StepResult::Busy | StepResult::Locked => {
                let code = if step == StepResult::Busy {
                    rusqlite::ffi::SQLITE_BUSY
                } else {
                    rusqlite::ffi::SQLITE_LOCKED
                };
                // Match Connection::backup: no new busy retry or sleep policy.
                return Err(
                    rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None).into(),
                );
            }
            _ => unreachable!("unsupported backup step result"),
        }
    }
}

fn assert_integrity(path: &Path) -> Result<()> {
    assert_integrity_with_budget(path, None)
}

fn assert_integrity_with_budget(path: &Path, budget: Option<&Budget>) -> Result<()> {
    check_budget(budget)?;
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if let Some(budget) = budget.copied() {
        // This private connection owns the hook and drops it with the connection.
        // The source connection's hooks and pragmas are never touched.
        connection.progress_handler(
            1_000,
            Some(move || {
                #[cfg(test)]
                test_stage(TestStage::IntegrityProgress, &budget);
                budget.expired()
            }),
        )?;
    }
    let result = (|| -> Result<()> {
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
    })();
    if let Err(error) = result {
        if let Some(budget) = budget.filter(|budget| budget.expired()) {
            // Retain SQLITE_INTERRUPT as the cause while making deadline errors
            // distinguishable from corrupt candidates in list_valid.
            return Err(error.context(budget.error()));
        }
        return Err(error);
    }
    check_budget(budget)
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    hash_file_with_budget(path, None)
}

fn hash_file_with_budget(path: &Path, budget: Option<&Budget>) -> Result<(u64, String)> {
    check_budget(budget)?;
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        check_budget(budget)?;
        let amount = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(amount) => amount,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        #[cfg(test)]
        if let Some(budget) = budget {
            test_stage(TestStage::HashBlock, budget);
        }
        check_budget(budget)?;
        size += amount as u64;
        digest.update(&buffer[..amount]);
        check_budget(budget)?;
    }
    check_budget(budget)?;
    Ok((size, format!("{:x}", digest.finalize())))
}

fn verify_backup(path: &Path, descriptor: &BackupDescriptor) -> Result<()> {
    verify_backup_with_budget(path, descriptor, None)
}

fn verify_backup_with_budget(
    path: &Path,
    descriptor: &BackupDescriptor,
    budget: Option<&Budget>,
) -> Result<()> {
    check_budget(budget)?;
    let metadata = fs::metadata(path)?;
    check_budget(budget)?;
    ensure!(
        metadata.is_file() && metadata.len() == descriptor.size,
        "backup size does not match its manifest"
    );
    let (size, sha256) = hash_file_with_budget(path, budget)?;
    ensure!(
        size == descriptor.size,
        "backup size does not match its manifest"
    );
    ensure!(
        sha256 == descriptor.sha256,
        "backup checksum does not match its manifest"
    );
    assert_integrity_with_budget(path, budget)
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestStage {
    BackupStep,
    IntegrityProgress,
    HashBlock,
    BeforeSqlitePublish,
    BeforeManifestPublish,
    AfterCreate,
    BeforeRotateDelete,
}

#[cfg(test)]
fn test_stage(stage: TestStage, budget: &Budget) {
    tests::STAGE_HOOK.with_borrow_mut(|hook| {
        if let Some(hook) = hook {
            hook(stage, budget);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    type StageHook = Box<dyn FnMut(TestStage, &Budget)>;
    thread_local! {
        pub(super) static STAGE_HOOK: RefCell<Option<StageHook>> = RefCell::new(None);
    }

    struct HookGuard;
    impl Drop for HookGuard {
        fn drop(&mut self) {
            STAGE_HOOK.with_borrow_mut(|hook| *hook = None);
        }
    }

    fn hook(callback: impl FnMut(TestStage, &Budget) + 'static) -> HookGuard {
        STAGE_HOOK.with_borrow_mut(|hook| {
            assert!(hook.is_none());
            *hook = Some(Box::new(callback));
        });
        HookGuard
    }

    // Synchronize at a real production stage, then let that invocation's actual
    // deadline expire. No mock clock, alternate SQL or production delay option.
    fn expire_at_stage(budget: &Budget) {
        std::thread::sleep(budget.deadline.saturating_duration_since(Instant::now()));
        assert!(budget.expired());
    }

    fn timeout(error: &anyhow::Error) {
        assert!(error.is::<CleanExitBackupTimeout>(), "{error:#}");
    }

    fn interrupted(error: &anyhow::Error) {
        timeout(error);
        assert_eq!(
            error
                .downcast_ref::<rusqlite::Error>()
                .and_then(|error| error.sqlite_error_code()),
            Some(rusqlite::ErrorCode::OperationInterrupted),
            "{error:#}"
        );
        println!("real integrity SQL returned SQLITE_INTERRUPT with clean-exit timeout: {error:#}");
    }

    fn large_source(root: &Path) -> Result<Connection> {
        let source = save_value(&root.join("source.sqlite"), "committed")?;
        source.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20000) INSERT INTO rows SELECT printf('%0100d', x) FROM n;")?;
        Ok(source)
    }

    fn assert_no_partial(service: &BackupService) -> Result<()> {
        for entry in fs::read_dir(&service.directory)? {
            assert!(!entry?.file_name().to_string_lossy().ends_with(".partial"));
        }
        Ok(())
    }

    #[test]
    fn clean_exit_online_backup_timeout_finishes_handles_before_partial_cleanup() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = large_source(root.path())?;
        assert!(source.query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))? > 100);
        let reached = Rc::new(Cell::new(false));
        let seen = reached.clone();
        let guard = hook(move |stage, budget| {
            if stage == TestStage::BackupStep {
                seen.set(true);
                expire_at_stage(budget);
            }
        });
        let mut service = BackupService::new(root.path())?;
        let error = service
            .backup_for_clean_exit(&source, 1, Duration::from_millis(250))
            .unwrap_err();
        drop(guard);
        assert!(reached.get());
        timeout(&error);
        assert_eq!(fs::read_dir(&service.directory)?.count(), 0);
        assert_eq!(
            source.query_row("SELECT count(*) FROM rows", [], |row| row.get::<_, i64>(0))?,
            20_001
        );
        let saved = service.backup_for_clean_exit(&source, 2, CLEAN_EXIT_BACKUP_TIMEOUT)?;
        assert_eq!(service.list_valid()?, vec![saved]);
        println!(
            "online backup stopped after first 100-page step; partial removed and immediate retry succeeded"
        );
        Ok(())
    }

    #[test]
    fn clean_exit_integrity_timeout_is_real_sql_interrupt_and_keeps_source_hook() -> Result<()> {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let root = tempfile::tempdir()?;
        let mut source = large_source(root.path())?;
        let source_progress = Arc::new(AtomicUsize::new(0));
        let progress = source_progress.clone();
        source.progress_handler(
            1,
            Some(move || {
                progress.fetch_add(1, Ordering::Relaxed);
                false
            }),
        )?;
        let reached = Rc::new(Cell::new(false));
        let observed = reached.clone();
        let caller = std::thread::current().id();
        let guard = hook(move |stage, budget| {
            assert_eq!(std::thread::current().id(), caller);
            if stage == TestStage::IntegrityProgress {
                observed.set(true);
                expire_at_stage(budget);
            }
        });
        let mut service = BackupService::new(root.path())?;
        let error = service
            .backup_for_clean_exit(&source, 1, Duration::from_millis(250))
            .unwrap_err();
        drop(guard);
        assert!(reached.get(), "never reached real integrity SQL");
        interrupted(&error);
        assert_no_partial(&service)?;
        assert!(service.list_valid()?.is_empty());
        source_progress.store(0, Ordering::Relaxed);
        assert_eq!(
            source.query_row("SELECT count(*) FROM rows", [], |row| row.get::<_, i64>(0))?,
            20_001
        );
        assert!(
            source_progress.load(Ordering::Relaxed) > 0,
            "source progress handler was replaced"
        );
        // Immediate retry exercises released backup/validation handles after the
        // failed call. There is no deferred cleanup task to wait for.
        let saved = service.backup_for_clean_exit(&source, 2, CLEAN_EXIT_BACKUP_TIMEOUT)?;
        assert_eq!(service.list_valid()?, vec![saved]);
        Ok(())
    }

    #[test]
    fn clean_exit_hash_and_publish_boundaries_stop_later_publication() -> Result<()> {
        for target in [
            TestStage::HashBlock,
            TestStage::BeforeSqlitePublish,
            TestStage::BeforeManifestPublish,
        ] {
            let root = tempfile::tempdir()?;
            let source = save_value(&root.path().join("source.sqlite"), "committed")?;
            let mut service = BackupService::new(root.path())?;
            let previous = service.create(&source, BackupReason::PreMigration, 1)?;
            let bytes = fs::read(&previous.path)?;
            let reached = Rc::new(Cell::new(false));
            let observed = reached.clone();
            let guard = hook(move |stage, budget| {
                if stage == target {
                    observed.set(true);
                    expire_at_stage(budget);
                }
            });
            let error = service
                .backup_for_clean_exit(&source, 2, Duration::from_millis(250))
                .unwrap_err();
            drop(guard);
            assert!(reached.get(), "did not reach {target:?}");
            timeout(&error);
            assert_eq!(service.list_valid()?, vec![previous.clone()]);
            assert_eq!(fs::read(previous.path)?, bytes);
            let sqlite_published = target == TestStage::BeforeManifestPublish;
            assert_eq!(
                service
                    .directory
                    .join("matou-2-clean-exit-v0.sqlite")
                    .exists(),
                sqlite_published
            );
            assert_eq!(
                fs::read_dir(&service.directory)?.count(),
                if sqlite_published { 3 } else { 2 }
            );
            assert_no_partial(&service)?;
        }
        Ok(())
    }

    #[test]
    fn clean_exit_rotation_sql_timeout_propagates_and_retains_all_published_backups() -> Result<()>
    {
        let root = tempfile::tempdir()?;
        let source = large_source(root.path())?;
        let mut service = BackupService::new(root.path())?;
        for now in 1..=8 {
            service.create(&source, BackupReason::PreMigration, now)?;
        }
        let rotating = Rc::new(Cell::new(false));
        let reached = Rc::new(Cell::new(false));
        let seen = reached.clone();
        let first_deadline = Rc::new(Cell::new(None));
        let guard = hook(move |stage, budget| {
            let initial = first_deadline.get().unwrap_or(budget.deadline);
            first_deadline.set(Some(initial));
            assert_eq!(budget.deadline, initial, "rotation reset create's deadline");
            if stage == TestStage::AfterCreate {
                rotating.set(true);
            }
            if stage == TestStage::IntegrityProgress && rotating.get() {
                seen.set(true);
                expire_at_stage(budget);
            }
        });
        let error = service
            .backup_for_clean_exit(&source, 9, Duration::from_millis(500))
            .unwrap_err();
        drop(guard);
        assert!(reached.get(), "did not reach rotation's real candidate SQL");
        interrupted(&error);
        assert_eq!(
            service.list_valid()?.len(),
            9,
            "timeout must not be swallowed as an invalid backup or proceed to deletion"
        );
        assert_no_partial(&service)?;
        Ok(())
    }

    #[test]
    fn clean_exit_expiration_before_rotation_delete_preserves_candidates() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = save_value(&root.path().join("source.sqlite"), "committed")?;
        let mut service = BackupService::new(root.path())?;
        for now in 1..=8 {
            service.create(&source, BackupReason::PreMigration, now)?;
        }
        let reached = Rc::new(Cell::new(false));
        let seen = reached.clone();
        let guard = hook(move |stage, budget| {
            if stage == TestStage::BeforeRotateDelete {
                seen.set(true);
                expire_at_stage(budget);
            }
        });
        let error = service
            .backup_for_clean_exit(&source, 9, Duration::from_millis(250))
            .unwrap_err();
        drop(guard);
        assert!(reached.get());
        timeout(&error);
        assert_eq!(service.list_valid()?.len(), 9);
        assert_no_partial(&service)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn clean_exit_rotation_io_failure_keeps_previous_backups_and_source() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir()?;
        let source = save_value(&root.path().join("source.sqlite"), "committed")?;
        let mut service = BackupService::new(root.path())?;
        let mut previous = Vec::new();
        for now in 1..=8 {
            let descriptor = service.create(&source, BackupReason::PreMigration, now)?;
            previous.push((descriptor.path.clone(), fs::read(&descriptor.path)?));
        }
        let directory = service.directory.clone();
        let permissions = fs::metadata(&directory)?.permissions();
        let hook_directory = directory.clone();
        let guard = hook(move |stage, _| {
            if stage == TestStage::BeforeRotateDelete {
                fs::set_permissions(&hook_directory, fs::Permissions::from_mode(0o500)).unwrap();
            }
        });
        let result = service.backup_for_clean_exit(&source, 9, CLEAN_EXIT_BACKUP_TIMEOUT);
        drop(guard);
        fs::set_permissions(directory, permissions)?;
        let error = result.unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().map(io::Error::kind),
            Some(io::ErrorKind::PermissionDenied),
            "{error:#}"
        );
        for (path, bytes) in previous {
            assert_eq!(fs::read(path)?, bytes);
        }
        assert_eq!(service.list_valid()?.len(), 9);
        assert_eq!(load_value(&root.path().join("source.sqlite"))?, "committed");
        assert_eq!(
            source.query_row("SELECT count(*) FROM rows", [], |row| row.get::<_, i64>(0))?,
            1
        );
        assert_no_partial(&service)?;
        println!("real rotation remove failed with PermissionDenied; all previous backups remain");
        Ok(())
    }

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
