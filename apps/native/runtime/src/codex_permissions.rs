//! Codex permissions come from the selected mode and the child process's config root.
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    path::Path,
};

use matou_native_core::pty::get_var;
use serde_json::Value;

use crate::{codex_launch, shell_input::CODEX_BYPASS};

pub(crate) const MODES: [&str; 4] = ["default", "acceptEdits", "plan", "bypassPermissions"];
const DEFAULT_PERMISSIONS: (&str, &str) = ("on-request", "workspace-write");

pub(crate) fn selected_mode(value: Option<&Value>) -> &'static str {
    value
        .and_then(Value::as_str)
        .and_then(|value| MODES.into_iter().find(|mode| *mode == value))
        .unwrap_or("default")
}

pub(crate) fn append_args(
    argv: &mut Vec<String>,
    requested: Option<&Value>,
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> (&'static str, Option<&'static str>) {
    let mode = selected_mode(requested);
    let invalid_mode = requested.is_some_and(|value| value.as_str() != Some(mode));
    let ((approval, sandbox), invalid_config) = match mode {
        "bypassPermissions" => {
            argv.push(CODEX_BYPASS.into());
            return (mode, None);
        }
        "acceptEdits" => (DEFAULT_PERMISSIONS, false),
        "plan" => (("on-request", "read-only"), false),
        _ => default_permissions(cwd, environment),
    };
    argv.extend([
        "--ask-for-approval".into(),
        approval.into(),
        "--sandbox".into(),
        sandbox.into(),
    ]);
    let english = environment
        .get(OsStr::new("MATOU_LOCALE"))
        .is_some_and(|value| value == "en");
    let warning = match (invalid_mode, invalid_config, english) {
        (true, _, true) => Some("Unknown Codex permission mode. Using Default."),
        (true, _, false) => Some("Codex 权限模式无效，已回退到 Default。"),
        (false, true, true) => Some(
            "Codex permission settings could not be read or were invalid. Using defaults for affected settings.",
        ),
        (false, true, false) => {
            Some("Codex 权限配置无法读取或包含无效值，受影响的设置已使用默认值。")
        }
        _ => None,
    };
    (mode, warning)
}

fn default_permissions(
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> ((&'static str, &'static str), bool) {
    let root = codex_launch::config_root(|key| get_var(environment, key))
        .map(|root| cwd.join(root))
        .or_else(|| std::env::home_dir().map(|home| home.join(".codex")));
    let Some(root) = root else {
        return (DEFAULT_PERMISSIONS, false);
    };
    match fs::read_to_string(root.join("config.toml")) {
        Ok(text) => parse_default_permissions(&text).unwrap_or((DEFAULT_PERMISSIONS, true)),
        Err(error) => (DEFAULT_PERMISSIONS, error.kind() != io::ErrorKind::NotFound),
    }
}

fn parse_default_permissions(text: &str) -> Option<((&'static str, &'static str), bool)> {
    let mut selected = DEFAULT_PERMISSIONS;
    let mut seen = [false; 2];
    let mut invalid = false;
    let mut remaining = text.trim();
    // TOML root keys appear before the first table. Scan strings and containers
    // so directives inside documentation or arrays cannot become permissions.
    while !remaining.is_empty() && !remaining.starts_with('[') {
        let (line, rest) = next_root_statement(remaining)?;
        remaining = rest.trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (index, target, supported): (_, &mut &'static str, &'static [&'static str]) =
            match key.trim() {
                "approval_policy" | "\"approval_policy\"" | "'approval_policy'" => (
                    0,
                    &mut selected.0,
                    &["on-request", "never", "untrusted", "on-failure"],
                ),
                "sandbox_mode" | "\"sandbox_mode\"" | "'sandbox_mode'" => (
                    1,
                    &mut selected.1,
                    &["read-only", "workspace-write", "danger-full-access"],
                ),
                _ => continue,
            };
        if std::mem::replace(&mut seen[index], true) {
            return None;
        }
        let value = value.trim();
        let parsed = if value.starts_with('"') {
            serde_json::from_str::<String>(value).ok()
        } else {
            value
                .strip_prefix('\'')
                .and_then(|text| text.strip_suffix('\''))
                .filter(|text| !text.contains('\''))
                .map(str::to_owned)
        };
        if let Some(value) = supported
            .iter()
            .find(|value| parsed.as_deref() == Some(**value))
        {
            *target = value;
        } else {
            invalid = true;
        }
    }
    Some((selected, invalid))
}

pub(crate) fn next_root_statement(text: &str) -> Option<(&str, &str)> {
    let mut cursor = 0;
    let mut depth = 0usize;
    while let Some(&byte) = text.as_bytes().get(cursor) {
        match byte {
            b'"' | b'\'' => {
                let bytes = text.as_bytes();
                let width = if bytes.get(cursor..cursor + 3) == Some(&[byte; 3]) {
                    3
                } else {
                    1
                };
                let delimiter = &bytes[cursor..cursor + width];
                cursor += width;
                loop {
                    if bytes.get(cursor..cursor + width) == Some(delimiter) {
                        cursor += width;
                        break;
                    }
                    match *bytes.get(cursor)? {
                        b'\\' if byte == b'"' => cursor += 2,
                        b'\n' | b'\r' if width == 1 => return None,
                        _ => cursor += 1,
                    }
                }
                continue;
            }
            b'#' => {
                let newline = text[cursor..]
                    .find('\n')
                    .map_or(text.len(), |offset| cursor + offset);
                if depth == 0 {
                    return Some((&text[..cursor], &text[newline..]));
                }
                cursor = newline;
                continue;
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth = depth.checked_sub(1)?,
            b'\n' if depth == 0 => return Some((&text[..cursor], &text[cursor + 1..])),
            _ => {}
        }
        cursor += 1;
    }
    (depth == 0).then_some((text, ""))
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use serde_json::json;

    use super::*;
    use crate::codex_launch;

    #[test]
    fn codex_permission_launch_args_follow_mode_and_effective_config_root() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let cwd = temp.path().join("项目 路径");
        let codex_home = cwd.join("隔离 配置");
        let home = temp.path().join("home");
        fs::create_dir_all(&codex_home)?;
        fs::create_dir_all(home.join(".codex"))?;
        fs::write(
            home.join(".codex/config.toml"),
            "approval_policy = 'never'\n",
        )?;
        let config = r#"
model_instructions = """
approval_policy = "never"
sandbox_mode = "read-only"
"""
extra = [
    { sandbox_mode = "read-only" }, # ignored
]
'approval_policy' = "untrusted"
"sandbox_mode" = 'danger-full-access' # root choice
[profiles.sample]
approval_policy = "never"
sandbox_mode = "read-only"
"#;
        fs::write(codex_home.join("config.toml"), config)?;
        let executable = std::env::current_exe()?;
        let mut env = BTreeMap::from([
            ("CODEX_HOME".into(), OsString::from("隔离 配置")),
            ("HOME".into(), home.clone().into_os_string()),
            ("USERPROFILE".into(), home.into_os_string()),
            ("MATOU_CODEX_COMMAND".into(), executable.into_os_string()),
        ]);
        for (mode, args) in [
            (
                "default",
                [
                    "--ask-for-approval",
                    "untrusted",
                    "--sandbox",
                    "danger-full-access",
                ]
                .as_slice(),
            ),
            (
                "acceptEdits",
                &[
                    "--ask-for-approval",
                    "on-request",
                    "--sandbox",
                    "workspace-write",
                ],
            ),
            (
                "plan",
                &["--ask-for-approval", "on-request", "--sandbox", "read-only"],
            ),
            ("bypassPermissions", &[CODEX_BYPASS]),
        ] {
            let plan =
                codex_launch::prepare(temp.path(), &cwd, Some(&json!(mode)), env.clone(), 80, 24)?;
            assert_eq!(plan.permission_mode, mode);
            assert_eq!(plan.permission_warning, None);
            assert_eq!(
                &plan.launch.argv[..2],
                ["-c", "check_for_update_on_startup=false"]
            );
            assert_eq!(&plan.launch.argv[2..], args);
            assert_eq!(plan.launch.cwd, cwd);
            assert_eq!(plan.launch.env[OsStr::new("CODEX_HOME")], "隔离 配置");
            assert_eq!(plan.launch.env[OsStr::new("TERM")], "xterm-256color");
            assert_eq!(
                plan.launch.env[OsStr::new("OPENAI_BASE_URL")],
                "https://api.openai.com/v1"
            );
            println!("fresh {mode}: {args:?}; config root, cwd and env verified");
        }
        assert_eq!(fs::read_to_string(codex_home.join("config.toml"))?, config);
        env.remove(OsStr::new("CODEX_HOME"));
        let mut args = Vec::new();
        assert_eq!(append_args(&mut args, None, &cwd, &env), ("default", None));
        assert_eq!(
            args,
            [
                "--ask-for-approval",
                "never",
                "--sandbox",
                "workspace-write"
            ]
        );
        println!(
            "unset CODEX_HOME: HOME/.codex supplied approval, absent sandbox used the default"
        );
        Ok(())
    }

    #[test]
    fn codex_permission_defaults_fall_back_and_warn_without_leaking_config() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().to_path_buf();
        let config = home.join("config.toml");
        let env = BTreeMap::from([
            ("CODEX_HOME".into(), home.clone().into_os_string()),
            ("MATOU_LOCALE".into(), "en".into()),
        ]);
        for (name, content, approval, sandbox, warns) in [
            ("missing file", None, "on-request", "workspace-write", false),
            (
                "profile only",
                Some("[profiles.sample]\napproval_policy='never'\nsandbox_mode='read-only'"),
                "on-request",
                "workspace-write",
                false,
            ),
            (
                "invalid sandbox value",
                Some("approval_policy='untrusted'\nsandbox_mode='private-fixture-value'"),
                "untrusted",
                "workspace-write",
                true,
            ),
            (
                "invalid approval value",
                Some("approval_policy=17\nsandbox_mode='read-only'"),
                "on-request",
                "read-only",
                true,
            ),
            (
                "duplicate root keys",
                Some(
                    "approval_policy='never'\n'approval_policy'='on-request'\nsandbox_mode='read-only'",
                ),
                "on-request",
                "workspace-write",
                true,
            ),
            (
                "unterminated config",
                Some("approval_policy='never'\nsandbox_mode='read-only"),
                "on-request",
                "workspace-write",
                true,
            ),
        ] {
            if let Some(content) = content {
                fs::write(&config, content)?;
            }
            let mut args = Vec::new();
            let (mode, warning) = append_args(&mut args, None, &home, &env);
            assert_eq!(mode, "default", "{name}");
            assert_eq!(
                args,
                ["--ask-for-approval", approval, "--sandbox", sandbox],
                "{name}"
            );
            assert_eq!(warning.is_some(), warns, "{name}");
            assert!(
                !warning
                    .unwrap_or_default()
                    .contains("private-fixture-value")
            );
            println!("{name}: {approval}/{sandbox}, warning={warns}");
        }
        let mut args = Vec::new();
        fs::write(&config, "approval_policy='never'\nsandbox_mode='read-only'")?;
        for invalid in [json!("auto"), json!(true), Value::Null] {
            assert_eq!(
                append_args(&mut args, Some(&invalid), &home, &env),
                (
                    "default",
                    Some("Unknown Codex permission mode. Using Default.")
                )
            );
            assert_eq!(
                args,
                ["--ask-for-approval", "never", "--sandbox", "read-only"]
            );
            args.clear();
        }
        println!("unknown stored modes: valid Default argv and a generic warning");
        Ok(())
    }
}
