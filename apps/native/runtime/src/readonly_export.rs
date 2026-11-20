use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::json;
use uuid::Uuid;

pub(crate) fn export(data_root: &Path, timestamp: i64) -> Result<PathBuf> {
    let destination = match env::var_os("MATOU_RECOVERY_EXPORT_DIR") {
        Some(path) => PathBuf::from(path),
        None => env::home_dir()
            .context("无法确定用户主目录")?
            .join("Downloads/Matou-Recovery"),
    };
    let destination = outside_data_root(data_root, &destination)?;
    fs::create_dir_all(&destination)
        .with_context(|| format!("创建导出位置 {}", destination.display()))?;
    let export_path = destination.join(format!("{timestamp}-{}", Uuid::new_v4()));
    fs::create_dir(&export_path)
        .with_context(|| format!("创建导出目录 {}", export_path.display()))?;

    let database = data_root.join("matou.sqlite");
    let mut names = Vec::new();
    for name in ["matou.sqlite", "matou.sqlite-wal", "matou.sqlite-shm"] {
        let source = data_root.join(name);
        match source.metadata() {
            Ok(metadata) if metadata.is_file() => {
                let target = export_path.join(name);
                fs::copy(&source, &target).with_context(|| {
                    format!("复制 {} 到 {}", source.display(), target.display())
                })?;
                names.push(name);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("读取 {}", source.display())),
        }
    }
    ensure!(!names.is_empty(), "没有可导出的数据库文件");
    let manifest_path = export_path.join("manifest.json");
    (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut manifest = options.open(&manifest_path)?;
        serde_json::to_writer_pretty(
            &mut manifest,
            &json!({
                "mode": "read-only",
                "sourceDatabasePath": database,
                "exportedAt": timestamp,
                "exportedFiles": names,
            }),
        )?;
        manifest.flush()?;
        Ok(())
    })()
    .with_context(|| format!("写入 {}", manifest_path.display()))?;
    Ok(export_path)
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
