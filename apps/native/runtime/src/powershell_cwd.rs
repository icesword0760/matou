use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use matou_native_core::pty::{LaunchProfile, ShellLaunch};

const REPORT_ENV: &str = "MATOU_POWERSHELL_CWD_REPORT";
const MAX_REPORT_BYTES: usize = 128 * 1024;

const INSTALL_PROMPT: &str = r#"
& {
    $reportPath = $env:MATOU_POWERSHELL_CWD_REPORT
    $env:MATOU_POWERSHELL_CWD_REPORT = $null
    if (-not $reportPath) { return }

    try {
        $global:__MatouPowerShellCwd = @{
            Prompt = ${function:prompt}
            Path = $reportPath
            Stage = $reportPath + '.stage'
            Utf8 = [Text.UTF8Encoding]::new($false)
            Last = $null
        }

        function global:prompt {
            # Let the original prompt observe the last command's status first.
            try {
                & $global:__MatouPowerShellCwd.Prompt
            } finally {
                try {
                    $report = $global:__MatouPowerShellCwd
                    $cwd = $ExecutionContext.SessionState.Path.CurrentFileSystemLocation.ProviderPath
                    if ($cwd -and -not [string]::Equals($cwd, $report.Last, [StringComparison]::Ordinal)) {
                        [IO.File]::WriteAllText($report.Stage, $cwd, $report.Utf8)
                        [IO.File]::Replace($report.Stage, $report.Path, [NullString]::Value)
                        $report.Last = $cwd
                    }
                } catch {}
            }
        }
    } catch {}
}
"#;

pub(crate) struct PowerShellCwd {
    _directory: tempfile::TempDir,
    report: PathBuf,
}

impl PowerShellCwd {
    pub(crate) fn attach(launch: &mut ShellLaunch) -> Result<Option<Self>> {
        if launch.profile != LaunchProfile::Shell || !launch.argv.is_empty() {
            return Ok(None);
        }
        let Some(executable) = launch.executable.file_name().and_then(|name| name.to_str()) else {
            return Ok(None);
        };
        if !["powershell.exe", "pwsh.exe", "powershell", "pwsh"]
            .iter()
            .any(|name| name.eq_ignore_ascii_case(executable))
        {
            return Ok(None);
        }

        let directory = tempfile::Builder::new()
            .prefix("matou-powershell-cwd-")
            .tempdir()
            .context("create PowerShell cwd report directory")?;
        let report = directory.path().join("cwd");
        File::create(&report).context("create PowerShell cwd report")?;

        let command: Vec<u8> = INSTALL_PROMPT
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        launch.argv = vec![
            "-NoExit".into(),
            "-EncodedCommand".into(),
            STANDARD.encode(command),
        ];
        launch.env.retain(|key, _| {
            !key.to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(REPORT_ENV))
        });
        launch
            .env
            .insert(REPORT_ENV.into(), report.as_os_str().to_owned());

        Ok(Some(Self {
            _directory: directory,
            report,
        }))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.report
    }
}

pub(crate) fn read_report(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_REPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.is_empty() || bytes.len() > MAX_REPORT_BYTES || bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(profile: LaunchProfile, executable: &str, argv: &[&str]) -> ShellLaunch {
        ShellLaunch {
            profile,
            executable: Path::new("shell binaries").join(executable),
            argv: argv.iter().map(|arg| (*arg).into()).collect(),
            cwd: PathBuf::new(),
            data_root: PathBuf::new(),
            inherited_env: Default::default(),
            env: Default::default(),
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn attaches_to_bare_powershell_launches_with_a_session_report() -> Result<()> {
        for executable in ["PowerShell.EXE", "pWsH.exe", "powershell", "pwsh"] {
            let mut launch = launch(LaunchProfile::Shell, executable, &[]);
            let report = PowerShellCwd::attach(&mut launch)?.unwrap();
            let path = report.path().to_owned();
            assert!(path.is_file());
            assert_eq!(read_report(&path), None);
            assert_eq!(
                launch.env[std::ffi::OsStr::new(REPORT_ENV)].as_os_str(),
                path.as_os_str()
            );
            assert_eq!(launch.argv.len(), 3);
            assert_eq!(&launch.argv[..2], &["-NoExit", "-EncodedCommand"]);
            let bytes = STANDARD.decode(&launch.argv[2])?;
            let script = String::from_utf16(
                &bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect::<Vec<_>>(),
            )?;
            assert_eq!(script, INSTALL_PROMPT);
            assert!(script.contains("$env:MATOU_POWERSHELL_CWD_REPORT = $null"));
            assert!(script.contains("CurrentFileSystemLocation.ProviderPath"));
            assert!(script.contains("[IO.File]::Replace($report.Stage, $report.Path"));
            drop(report);
            assert!(!path.exists());
        }
        Ok(())
    }

    #[test]
    fn leaves_other_shells_and_explicit_commands_untouched() -> Result<()> {
        for mut launch in [
            launch(LaunchProfile::Shell, "cmd.exe", &[]),
            launch(LaunchProfile::Shell, "pwsh.exe", &["-NoProfile"]),
            launch(LaunchProfile::Codex, "pwsh.exe", &[]),
        ] {
            let argv = launch.argv.clone();
            assert!(PowerShellCwd::attach(&mut launch)?.is_none());
            assert_eq!(launch.argv, argv);
            assert!(launch.env.is_empty());
        }
        Ok(())
    }

    #[test]
    fn reads_paths_verbatim_and_rejects_invalid_reports() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let report = temp.path().join("cwd");
        let unc = r"\\host\share\ 项目目录 ";
        std::fs::write(&report, unc)?;
        assert_eq!(read_report(&report).as_deref(), Some(unc));
        for invalid in [
            Vec::new(),
            vec![b'C', b':', 0],
            vec![0xff],
            vec![b'a'; MAX_REPORT_BYTES + 1],
        ] {
            std::fs::write(&report, invalid)?;
            assert_eq!(read_report(&report), None);
        }
        Ok(())
    }
}
