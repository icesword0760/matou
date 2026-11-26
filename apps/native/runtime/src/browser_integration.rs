//! Installation diagnostics for the controlled sign-in browser.

use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use matou_native_core::sessions::InterfaceLocale;

pub const DEFAULT_BROWSER_EXECUTABLE: &str = "/Applications/Mantle.app/Contents/MacOS/Chromium";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserDiagnostic {
    Missing,
    InvalidPath,
    PermissionDenied,
    ProbeFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserStatus {
    Installed { version: Option<String> },
    Unavailable(BrowserDiagnostic),
}

impl BrowserStatus {
    pub fn label(&self, locale: InterfaceLocale) -> String {
        match self {
            Self::Installed { version } => {
                let installed = locale.text("已安装", "Installed");
                match version {
                    Some(version) => format!("{installed} · {version}"),
                    None => installed.to_owned(),
                }
            }
            Self::Unavailable(_) => locale.text("未安装", "Not installed").to_owned(),
        }
    }
}

pub fn browser_executable(path_override: Option<&OsStr>) -> PathBuf {
    path_override
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| OsStr::new(DEFAULT_BROWSER_EXECUTABLE))
        .into()
}

/// The file probe returns false for a non-file and PermissionDenied when it
/// cannot be executed. The version probe receives the bundle's Info.plist path.
pub fn detect_browser(
    executable: &Path,
    executable_probe: impl FnOnce(&Path) -> io::Result<bool>,
    version_probe: impl FnOnce(&Path) -> io::Result<String>,
) -> BrowserStatus {
    use BrowserDiagnostic::{InvalidPath, Missing, PermissionDenied, ProbeFailed};

    if executable.as_os_str().is_empty() || executable.as_os_str().as_encoded_bytes().contains(&0) {
        return BrowserStatus::Unavailable(InvalidPath);
    }
    let diagnostic = match executable_probe(executable) {
        Ok(true) => {
            let version_file = executable
                .parent()
                .and_then(Path::parent)
                .unwrap_or_else(|| Path::new(""))
                .join("Info.plist");
            let version = version_probe(&version_file)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty());
            return BrowserStatus::Installed { version };
        }
        Ok(false) => InvalidPath,
        Err(error) => match error.kind() {
            io::ErrorKind::NotFound => Missing,
            io::ErrorKind::InvalidInput | io::ErrorKind::NotADirectory => InvalidPath,
            io::ErrorKind::PermissionDenied => PermissionDenied,
            _ => ProbeFailed,
        },
    };
    BrowserStatus::Unavailable(diagnostic)
}

/// Desktop installation check with the frozen stat/file and X_OK semantics.
pub fn inspect_executable(path: &Path) -> io::Result<bool> {
    if !fs::metadata(path)?.is_file() {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};

        let path = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: CString owns a valid nul-terminated path for the call.
        if unsafe { libc::access(path.as_ptr(), libc::X_OK) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(true)
}

/// Open a trusted URL through the controlled browser shared by the desktop
/// and subscription setup. A result reports whether the OS accepted launch.
pub fn open_controlled_url(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let browser = Path::new(DEFAULT_BROWSER_EXECUTABLE);
        if !browser.is_file() {
            return false;
        }
        return run_url_opener("/usr/bin/open", &["-a", "/Applications/Mantle.app", url]);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        false
    }
}

pub fn prefers_controlled_browser(data_root: &Path) -> bool {
    data_root.join("use-controlled-browser").is_file()
}

pub fn set_controlled_browser_preference(data_root: &Path, enabled: bool) -> io::Result<()> {
    let preference = data_root.join("use-controlled-browser");
    if enabled {
        fs::OpenOptions::new().create(true).append(true).open(preference)?;
    } else if let Err(error) = fs::remove_file(preference)
        && error.kind() != io::ErrorKind::NotFound
    {
        return Err(error);
    }
    Ok(())
}

pub fn open_system_url(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    return run_url_opener("/usr/bin/open", &[url]);
    #[cfg(windows)]
    {
        let target = url
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        return crate::update_download::shell_open(&target) > 32;
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    return run_url_opener("/usr/bin/xdg-open", &[url]);
}

#[cfg(not(windows))]
fn run_url_opener(program: &str, args: &[&str]) -> bool {
    use std::process::{Command, Stdio};
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use BrowserDiagnostic::*;
    use InterfaceLocale::{Chinese, English};

    #[test]
    fn resolves_the_frozen_default_and_explicit_overrides() {
        let default = Path::new(DEFAULT_BROWSER_EXECUTABLE);
        assert_eq!(browser_executable(None), default);
        assert_eq!(browser_executable(Some(OsStr::new(""))), default);
        let custom = OsStr::new("/test/Test Browser.app/Contents/MacOS/test-browser");
        assert_eq!(browser_executable(Some(custom)), Path::new(custom));
    }

    #[test]
    fn detects_installed_browser_and_trims_version_metadata() {
        let executable = browser_executable(None);
        let status = detect_browser(
            &executable,
            |path| {
                assert_eq!(path, executable);
                Ok(true)
            },
            |path| {
                assert_eq!(
                    path,
                    Path::new("/Applications/Mantle.app/Contents/Info.plist")
                );
                Ok(" \nv148.0.1\t\n".to_owned())
            },
        );
        assert_eq!(
            status,
            BrowserStatus::Installed {
                version: Some("v148.0.1".to_owned())
            }
        );
        assert_eq!(status.label(Chinese), "已安装 · v148.0.1");
        assert_eq!(status.label(English), "Installed · v148.0.1");
    }

    #[test]
    fn missing_or_blank_version_still_means_installed() {
        for output in [
            Err(io::ErrorKind::PermissionDenied.into()),
            Ok(" \n".to_owned()),
        ] {
            let status = detect_browser(
                Path::new("test-browser"),
                |_| Ok(true),
                |path| {
                    assert_eq!(path, Path::new("Info.plist"));
                    output
                },
            );
            assert_eq!(status, BrowserStatus::Installed { version: None });
            assert_eq!(status.label(Chinese), "已安装");
            assert_eq!(status.label(English), "Installed");
        }
    }

    #[test]
    fn diagnoses_unavailable_executables_without_probing_versions() {
        for (probe, diagnostic) in [
            (Err(io::ErrorKind::NotFound.into()), Missing),
            (Ok(false), InvalidPath),
            (Err(io::ErrorKind::InvalidInput.into()), InvalidPath),
            (Err(io::ErrorKind::NotADirectory.into()), InvalidPath),
            (
                Err(io::ErrorKind::PermissionDenied.into()),
                PermissionDenied,
            ),
            (Err(io::ErrorKind::Other.into()), ProbeFailed),
        ] {
            let status = detect_browser(
                Path::new("test-browser"),
                |_| probe,
                |_| panic!("unexpected version check"),
            );
            assert_eq!(status, BrowserStatus::Unavailable(diagnostic));
            assert_eq!(status.label(Chinese), "未安装");
            assert_eq!(status.label(English), "Not installed");
        }
        for path in ["", "invalid\0path"] {
            assert_eq!(
                detect_browser(
                    Path::new(path),
                    |_| panic!("invalid path probed"),
                    |_| panic!("invalid path probed")
                ),
                BrowserStatus::Unavailable(InvalidPath)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn verifies_real_files_and_execute_permission_without_executing_anything() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("browser");
        fs::write(&binary, b"test fixture").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            inspect_executable(&binary).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(inspect_executable(&binary).unwrap());
        assert!(!inspect_executable(root.path()).unwrap());
        let link = root.path().join("link");
        symlink(&binary, &link).unwrap();
        assert!(inspect_executable(&link).unwrap());
        fs::remove_file(&binary).unwrap();
        assert_eq!(
            inspect_executable(&link).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
}
