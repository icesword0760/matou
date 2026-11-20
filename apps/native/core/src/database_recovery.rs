//! Writable recovery while the runtime is parked on a pending marker.
use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::{Connection, ErrorCode, OpenFlags};
use uuid::Uuid;

use crate::{
    backup::BackupService,
    ownership::{
        DatabaseOwner, OwnerRecoveryError, assert_no_live_owner, with_recovery_action_fence,
    },
    recovery_marker,
    startup::{self, RecoveryRequired, append_suffix},
};

pub const RECOVERY_ID_CHANGED: &str = "数据库恢复周期已更新，本次操作已停止";

#[derive(Debug)]
pub struct RecoveryActionError {
    pub stage: &'static str,
    pub cause: anyhow::Error,
}

impl std::fmt::Display for RecoveryActionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {:#}", self.stage, self.cause)
    }
}

impl std::error::Error for RecoveryActionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

#[derive(Default)]
pub struct RecoveryActions {
    // Close SQLite before releasing the lease, including on Drop.
    connection: Option<Connection>,
    lease: Option<DatabaseOwner>,
}

impl RecoveryActions {
    pub fn retry_open(
        &mut self,
        recovery: &RecoveryRequired,
        generation: &str,
        request_id: Uuid,
        expected_recovery_id: &str,
    ) -> std::result::Result<(Connection, DatabaseOwner, PathBuf), RecoveryActionError> {
        let error = |stage, cause| RecoveryActionError { stage, cause };
        if expected_recovery_id != recovery.recovery_id
            || !recovery_marker::valid_recovery_id(expected_recovery_id)
        {
            return Err(error("marker", anyhow!(RECOVERY_ID_CHANGED)));
        }
        let expected = recovery.marker.as_ref().ok_or_else(|| {
            error(
                "marker",
                anyhow!("数据库恢复状态已更新，请使用最新恢复页面重试"),
            )
        })?;
        // Reject a stale, missing or invalid marker before even opening the fence.
        recovery_marker::authorize_action(expected).map_err(|cause| error("marker", cause))?;
        self.cleanup().map_err(|cause| {
            error(
                "cleanup",
                cause.context("数据库恢复失败，资源清理将在下一次操作前重试"),
            )
        })?;

        let database = &expected.durable_database_path;
        let root = database.parent().expect("validated database root");
        let mut stage = "fence";
        let opened = with_recovery_action_fence(database, || {
            stage = "marker";
            let marker = recovery_marker::authorize_action(expected)?;
            stage = "open";
            require_original(database)?;
            ensure!(
                startup::is_writable(root) && startup::is_writable(database),
                io::Error::from(io::ErrorKind::PermissionDenied)
            );
            if let Some(issue) = marker.ownership_issue.as_deref() {
                stage = "ownership-evidence";
                archive_ownership(database, root, request_id, issue)?;
            }
            stage = "lease";
            let lease = self
                .lease
                .insert(DatabaseOwner::acquire(database, generation)?);
            stage = "open";
            startup::assert_wal_bundle_ready(database)?;
            let mut backups = BackupService::new(root)?;
            let sqlite_path = root
                .canonicalize()?
                .join(database.file_name().expect("database filename"));
            self.connection = Some(startup::open_writable(
                &sqlite_path,
                generation,
                lease,
                &mut backups,
                true,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )?);
            stage = "fence-finalization";
            Ok(())
        });
        let settled = opened.map_err(|cause| error(stage, cause)).and_then(|()| {
            recovery_marker::resolve_action(expected, startup::now())
                .map_err(|cause| error("marker-commit", cause))
        });
        if let Err(mut failure) = settled {
            if let Err(cause) = self.cleanup() {
                failure.cause = failure.cause.context(format!(
                    "数据库恢复失败，资源清理将在下一次操作前重试（{cause:#}）"
                ));
            }
            return Err(failure);
        }
        Ok((
            self.connection.take().expect("reopened database"),
            self.lease.take().expect("reopened database lease"),
            root.to_owned(),
        ))
    }

    fn cleanup(&mut self) -> Result<()> {
        if let Some(connection) = self.connection.take() {
            if let Err((connection, error)) = connection.close() {
                self.connection = Some(connection);
                return Err(error.into());
            }
        }
        if let Some(lease) = &mut self.lease {
            lease.release()?;
            self.lease = None;
        }
        Ok(())
    }
}

fn require_original(database: &Path) -> Result<()> {
    const ORIGINAL_MISSING: &str = "原数据库尚未回到可检查的位置，请先恢复备份或导出恢复资料";
    match fs::symlink_metadata(database) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(anyhow!(ORIGINAL_MISSING)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(anyhow!(ORIGINAL_MISSING)),
        Err(error) => Err(error.into()),
    }
}

fn archive_ownership(database: &Path, root: &Path, request_id: Uuid, issue: &str) -> Result<()> {
    let owner = append_suffix(database, ".owner");
    let sidecar = append_suffix(&owner, ".takeover.sqlite");
    let sources = [owner.as_path(), sidecar.as_path()];
    let mut present = Vec::new();
    for path in sources {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() || metadata.is_dir() => present.push(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => {
                return Err(anyhow!(
                    "ownership evidence is not a regular file or directory"
                ));
            }
        }
    }
    if present.is_empty() {
        return Ok(());
    }
    let moving_owner = present.contains(&owner.as_path());
    check_owner_for_archive(database, &owner, &sidecar, issue, moving_owner)?;

    let evidence = root.join("recovery-evidence");
    match fs::create_dir(&evidence) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            ensure!(
                fs::symlink_metadata(&evidence)?.is_dir(),
                "invalid recovery evidence directory"
            );
        }
        Err(error) => return Err(error.into()),
    }
    let directory = evidence.join(request_id.to_string());
    fs::create_dir(&directory)?;

    // Recheck locally immediately before moving the owner. A malformed owner
    // cannot be taken over, and a stale valid owner behind a broken sidecar
    // remains protected until that sidecar is moved last.
    check_owner_for_archive(database, &owner, &sidecar, issue, moving_owner)?;
    for path in present {
        fs::rename(
            path,
            directory.join(path.file_name().expect("managed evidence filename")),
        )
        .with_context(|| format!("archive database ownership evidence {}", path.display()))?;
    }
    Ok(())
}

fn check_owner_for_archive(
    database: &Path,
    owner: &Path,
    sidecar: &Path,
    issue: &str,
    moving_owner: bool,
) -> Result<()> {
    let present = match fs::symlink_metadata(owner) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() || metadata.is_dir(),
                "invalid database owner type"
            );
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    ensure!(!moving_owner || present, "database owner evidence changed");
    match assert_no_live_owner(database) {
        Err(error)
            if error
                .downcast_ref::<OwnerRecoveryError>()
                .is_some_and(|error| error.issue == "owner-record-malformed")
                && !error.chain().any(|cause| cause.is::<io::Error>()) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
        Ok(()) if !present => Ok(()),
        Ok(()) => {
            // A regular takeover sidecar can become usable between discovery and
            // retry. Leave it to the normal lease acquisition unless still broken.
            ensure!(
                issue == "takeover-sidecar-unusable" && takeover_sidecar_is_broken(sidecar)?,
                "数据库恢复状态已更新，请使用最新恢复页面重试"
            );
            Ok(())
        }
    }
}

fn takeover_sidecar_is_broken(path: &Path) -> Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        return Ok(true);
    }
    ensure!(metadata.is_file(), "invalid database takeover sidecar type");
    let result = startup::open_read_only(path);
    match result {
        Ok(_) => Ok(false),
        Err(error)
            if error
                .downcast_ref::<rusqlite::Error>()
                .is_some_and(|error| {
                    matches!(
                        error.sqlite_error_code(),
                        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
                    )
                }) =>
        {
            Ok(true)
        }
        Err(error) => Err(error),
    }
}
