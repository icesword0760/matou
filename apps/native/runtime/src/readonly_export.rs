use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use matou_native_core::startup::RecoveryRequired;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::host::DatabaseRecoveryDetails;

pub(crate) fn export(data_root: &Path, timestamp: i64) -> Result<PathBuf> {
    let destination = match env::var_os("MATOU_RECOVERY_EXPORT_DIR") {
        Some(path) => PathBuf::from(path),
        None => env::home_dir()
            .context("无法确定用户主目录")?
            .join("Downloads/Matou-Recovery"),
    };
    let destination = outside_data_root(data_root, &destination)?;
    let sources =
        ["matou.sqlite", "matou.sqlite-wal", "matou.sqlite-shm"].map(|name| data_root.join(name));
    export_bundle(&destination, timestamp, sources, false, |names| {
        ensure!(!names.is_empty(), "没有可导出的数据库文件");
        Ok(json!({
            "mode": "read-only",
            "sourceDatabasePath": data_root.join("matou.sqlite"),
            "exportedAt": timestamp,
            "exportedFiles": names,
        }))
    })
}

pub(crate) fn export_pending(recovery: &RecoveryRequired, now: fn() -> i64) -> Result<PathBuf> {
    let database = &recovery.durable_database_path;
    let quarantine = &recovery.quarantined_path;
    let destination = database
        .parent()
        .context("数据库恢复目录无效")?
        .join("recovery-exports");
    ensure!(
        !destination.is_symlink(),
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("导出位置是符号链接 {}", destination.display()),
        )
    );
    let suffix = |path: &Path, suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    // For an ownership failure the recorded evidence location is the original
    // database path. For a quarantined bundle its separate path is authoritative.
    let sources = [
        recovery.marker_path.clone(),
        quarantine.clone(),
        suffix(quarantine, "-wal"),
        suffix(quarantine, "-shm"),
        suffix(database, ".owner"),
        suffix(database, ".owner.takeover.sqlite"),
    ];
    let mut seen = std::collections::HashSet::new();
    let sources = sources
        .into_iter()
        .filter(|source| seen.insert(source.clone()));
    export_bundle(&destination, now(), sources, true, |names| {
        let backups = DatabaseRecoveryDetails::from(recovery).backups;
        let mut manifest = json!({
            "exportedAt": now(),
            "reason": recovery.reason,
            "durableDatabasePath": database,
            "quarantinedPath": quarantine,
            "backupCount": backups.len(),
            "backups": backups,
            "exportedFiles": names,
        });
        if let Some(issue) = &recovery.ownership_issue {
            manifest["ownershipIssue"] = json!(issue);
        }
        Ok(manifest)
    })
}

fn export_bundle(
    destination: &Path,
    timestamp: i64,
    sources: impl IntoIterator<Item = PathBuf>,
    copy_evidence: bool,
    manifest: impl FnOnce(Vec<String>) -> Result<Value>,
) -> Result<PathBuf> {
    fs::create_dir_all(destination)
        .with_context(|| format!("创建导出位置 {}", destination.display()))?;
    let export_path = destination.join(format!("{timestamp}-{}", Uuid::new_v4()));
    fs::create_dir(&export_path)
        .with_context(|| format!("创建导出目录 {}", export_path.display()))?;

    let mut names = Vec::new();
    for source in sources {
        match source.metadata() {
            Ok(metadata) if copy_evidence || metadata.is_file() => {
                let name = source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .context("导出文件名无效")?;
                let target = export_path.join(name);
                (if copy_evidence {
                    copy_evidence_entry(&source, &target)
                } else {
                    fs::copy(&source, &target).map(|_| ())
                })
                .with_context(|| format!("复制 {} 到 {}", source.display(), target.display()))?;
                names.push(name.to_owned());
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("读取 {}", source.display())),
        }
    }
    write_manifest(&export_path.join("manifest.json"), manifest(names)?)?;
    Ok(export_path)
}

fn write_manifest(path: &Path, contents: Value) -> Result<()> {
    (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut manifest = options.open(path)?;
        serde_json::to_writer_pretty(&mut manifest, &contents)?;
        manifest.flush()?;
        Ok(())
    })()
    .with_context(|| format!("写入 {}", path.display()))
}

fn copy_evidence_entry(source: &Path, target: &Path) -> io::Result<()> {
    let kind = fs::symlink_metadata(source)?.file_type();
    if kind.is_symlink() {
        let link = source.with_file_name(fs::read_link(source)?);
        #[cfg(unix)]
        return std::os::unix::fs::symlink(link, target);
        #[cfg(windows)]
        {
            use std::os::windows::fs::{FileTypeExt, symlink_dir, symlink_file};
            return if kind.is_symlink_dir() {
                symlink_dir(link, target)
            } else {
                symlink_file(link, target)
            };
        }
    }
    if kind.is_file() {
        fs::copy(source, target)?;
    } else if kind.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_evidence_entry(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("无法复制恢复证据 {}", source.display()),
        ));
    }
    Ok(())
}

fn outside_data_root(data_root: &Path, destination: &Path) -> Result<PathBuf> {
    let source = data_root.canonicalize()?;
    let absolute = std::path::absolute(destination)?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    let mut resolved = loop {
        match ancestor.canonicalize() {
            Ok(resolved) => break resolved,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // An unresolved symlink could otherwise point into the source.
                if ancestor.symlink_metadata().is_ok() {
                    return Err(error)
                        .with_context(|| format!("解析导出位置 {}", ancestor.display()));
                }
                missing.push(ancestor.file_name().context("无效的导出位置")?);
                ancestor = ancestor.parent().context("无效的导出位置")?;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("解析导出位置 {}", ancestor.display()));
            }
        }
    };
    resolved.extend(missing.into_iter().rev());
    ensure!(
        !resolved.starts_with(source),
        "导出位置必须位于数据库目录之外"
    );
    Ok(resolved)
}
