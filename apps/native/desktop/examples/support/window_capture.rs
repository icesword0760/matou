use std::{path::Path, process::Command};

pub(super) fn capture_png(window_number: isize, output: &Path) -> anyhow::Result<()> {
    let result = Command::new("/usr/sbin/screencapture")
        .args(["-x", "-o", "-t", "png", "-l"])
        .arg(window_number.to_string())
        .arg(output)
        .output()?;
    anyhow::ensure!(
        result.status.success() && output.metadata().is_ok_and(|file| file.len() > 0),
        "window capture {}: {}",
        output.display(),
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}
