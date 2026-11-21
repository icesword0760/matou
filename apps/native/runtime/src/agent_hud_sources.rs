use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, Instant},
};

use regex::Regex;
use serde_json::Value;

use crate::{
    agent_hud_projection::{AgentActivity, AgentSource, AgentState, ConfigCounts, timestamp_ms},
    codex_events,
    codex_permissions::next_root_statement,
    provider_hook::nonempty,
};

fn read_config(path: &Path) -> Option<String> {
    const MAX_BYTES: u64 = 1024 * 1024;
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return None;
    }
    let mut text = String::new();
    file.take(MAX_BYTES + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= MAX_BYTES).then_some(text)
}

fn read_json(path: &Path) -> Value {
    read_config(path)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn add_json_hooks(hooks: &mut BTreeSet<String>, config: &Value) {
    if let Some(events) = config["hooks"].as_object().or_else(|| {
        config
            .as_object()
            .filter(|object| object.values().all(Value::is_array))
    }) {
        hooks.extend(events.keys().cloned());
    }
}

pub(crate) fn codex_config(root: Option<&Path>) -> (ConfigCounts, Vec<String>) {
    let mut mcp = BTreeSet::new();
    let mut hooks: BTreeSet<_> = codex_events::EVENTS
        .into_iter()
        .map(|(name, _)| name.to_owned())
        .collect();
    if let Some(root) = root {
        add_json_hooks(&mut hooks, &read_json(&root.join("hooks.json")));
        if let Some(text) = read_config(&root.join("config.toml")) {
            codex_toml_names(&text, &mut mcp, &mut hooks);
        }
    }
    (
        ConfigCounts {
            mcps: mcp.len(),
            hooks: hooks.len(),
            ..ConfigCounts::default()
        },
        hooks.into_iter().collect(),
    )
}

fn codex_toml_names(text: &str, mcp: &mut BTreeSet<String>, hooks: &mut BTreeSet<String>) {
    static KEY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"^(?:([A-Za-z0-9_-]+)|"((?:[^"\\]|\\.)*)"|'([^']*)')\s*"#).unwrap()
    });
    let path = |text: &str| -> Option<Vec<String>> {
        let mut keys = Vec::new();
        let mut rest = text.trim();
        loop {
            let key = KEY.captures(rest)?;
            keys.push(if let Some(basic) = key.get(2) {
                serde_json::from_str(&format!("\"{}\"", basic.as_str())).ok()?
            } else {
                key.get(1).or_else(|| key.get(3))?.as_str().to_owned()
            });
            rest = &rest[key.get(0)?.end()..];
            if rest.is_empty() {
                return Some(keys);
            }
            rest = rest.strip_prefix('.')?.trim_start();
        }
    };
    let mut section = Vec::new();
    let mut remaining = text.trim_start();
    while !remaining.is_empty() {
        let Some((statement, rest)) = next_root_statement(remaining) else {
            break;
        };
        remaining = rest.trim_start();
        let statement = statement.trim();
        if statement.is_empty() {
            continue;
        }
        let names = if let Some(table) = statement
            .strip_prefix('[')
            .and_then(|text| text.strip_suffix(']'))
        {
            let table = table
                .strip_prefix('[')
                .and_then(|text| text.strip_suffix(']'))
                .unwrap_or(table);
            section = path(table).unwrap_or_default();
            section.clone()
        } else if let Some((key, _)) = statement.split_once('=') {
            let mut keys = section.clone();
            keys.extend(path(key).unwrap_or_default());
            keys
        } else {
            continue;
        };
        if let [namespace, name, ..] = names.as_slice() {
            if namespace == "mcp_servers" {
                mcp.insert(name.clone());
            } else if namespace == "hooks" {
                hooks.insert(name.clone());
            }
        }
    }
}

fn add_antigravity_hooks(hooks: &mut BTreeSet<String>, config: &Value) {
    add_json_hooks(hooks, config);
    if config.get("hooks").is_none() {
        for namespace in config
            .as_object()
            .into_iter()
            .flat_map(|object| object.values())
        {
            add_json_hooks(hooks, namespace);
        }
    }
}

pub(crate) fn antigravity_config(roots: &[PathBuf]) -> (ConfigCounts, Vec<String>) {
    let mut mcp = BTreeSet::new();
    let mut hooks = BTreeSet::new();
    for root in roots {
        for file in [
            "mcp_config.json",
            "settings.json",
            "hooks.json",
            "hooks/hooks.json",
        ] {
            let config = read_json(&root.join(file));
            if let Some(servers) = config["mcpServers"]
                .as_object()
                .or_else(|| config["mcp_servers"].as_object())
            {
                mcp.extend(servers.keys().cloned());
            }
            if file.ends_with("hooks.json") || config.get("hooks").is_some() {
                add_antigravity_hooks(&mut hooks, &config);
            }
        }
        for plugin in fs::read_dir(root.join("plugins"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        {
            for file in ["hooks.json", "hooks/hooks.json"] {
                add_antigravity_hooks(&mut hooks, &read_json(&plugin.path().join(file)));
            }
        }
    }
    (
        ConfigCounts {
            mcps: mcp.len(),
            hooks: hooks.len(),
            ..ConfigCounts::default()
        },
        hooks.into_iter().collect(),
    )
}

#[derive(Default)]
pub(crate) struct ExternalAgents {
    refreshed: Option<Instant>,
    entries: BTreeMap<String, ExternalStatus>,
}

struct ExternalStatus {
    activity: AgentActivity,
    state: String,
    updated_at: f64,
    pid: Option<u32>,
    interrupted_at: Option<f64>,
}

impl ExternalAgents {
    pub fn read(&mut self, directory: &Path, now: f64) -> Vec<AgentActivity> {
        self.read_with(directory, now, process_alive)
    }

    fn read_with(
        &mut self,
        directory: &Path,
        now: f64,
        alive: impl Fn(u32) -> Option<bool>,
    ) -> Vec<AgentActivity> {
        if self
            .refreshed
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(2))
        {
            self.refreshed = Some(Instant::now());
            if let Ok(files) = fs::read_dir(directory) {
                let mut found = BTreeSet::new();
                for file in files.flatten() {
                    let Some(name) = file.file_name().into_string().ok() else {
                        continue;
                    };
                    let Some(id) = name.strip_suffix(".json").filter(|id| {
                        !id.is_empty()
                            && id.len() <= 80
                            && id
                                .bytes()
                                .all(|ch| ch.is_ascii_alphanumeric() || b"._-".contains(&ch))
                    }) else {
                        continue;
                    };
                    if !file.file_type().is_ok_and(|kind| kind.is_file()) {
                        continue;
                    }
                    found.insert(name.clone());
                    if let Some(mut status) = ExternalStatus::parse(id, &read_json(&file.path())) {
                        status.interrupted_at = self
                            .entries
                            .get(&name)
                            .filter(|previous| previous.pid == status.pid)
                            .and_then(|previous| previous.interrupted_at);
                        self.entries.insert(name, status);
                    }
                }
                self.entries.retain(|name, _| found.contains(name));
            }
        }
        self.entries
            .values_mut()
            .filter_map(|status| status.project(now, &alive))
            .collect()
    }
}

impl ExternalStatus {
    fn parse(id: &str, value: &Value) -> Option<Self> {
        let label = nonempty(&value["label"])?
            .lines()
            .next()?
            .trim()
            .chars()
            .take(60)
            .collect::<String>();
        if value["version"] != 1
            || label.is_empty()
            || value
                .get("detail")
                .is_some_and(|detail| !detail.is_string())
        {
            return None;
        }
        let state = nonempty(&value["state"])?;
        let activity = AgentActivity {
            id: format!("external:{id}"),
            label,
            source: AgentSource::External,
            state: match state {
                "running" | "completed" | "stopped" => AgentState::Running,
                "waiting" => AgentState::Waiting,
                "warning" => AgentState::Warning,
                "error" => AgentState::Error,
                _ => return None,
            },
            started_at: Some(timestamp_ms(&value["startedAt"])?),
            detail: value["detail"]
                .as_str()
                .and_then(|text| text.lines().next())
                .map(|text| text.trim().chars().take(120).collect()),
        };
        Some(Self {
            activity,
            state: state.to_owned(),
            updated_at: timestamp_ms(&value["updatedAt"])?,
            pid: value["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .filter(|pid| *pid > 0),
            interrupted_at: None,
        })
    }

    fn project(&mut self, now: f64, alive: &impl Fn(u32) -> Option<bool>) -> Option<AgentActivity> {
        let age = now - self.updated_at;
        if matches!(self.state.as_str(), "completed" | "stopped")
            || self.state == "error" && age >= 300_000.
        {
            return None;
        }
        let mut activity = self.activity.clone();
        if self.state != "error" && self.pid.and_then(alive) == Some(false) {
            let interrupted = *self.interrupted_at.get_or_insert(now);
            if now - interrupted >= 300_000. {
                return None;
            }
            activity.state = AgentState::Error;
            activity.detail = Some("监工进程已退出，任务中断".into());
        } else {
            self.interrupted_at = None;
            if self.state != "error" && age > 60_000. {
                activity.state = AgentState::Error;
                activity.detail = Some("状态已停止更新".into());
            }
        }
        Some(activity)
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> Option<bool> {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return Some(false);
    };
    // Signal zero only checks whether the process exists.
    Some(
        (unsafe { libc::kill(pid, 0) }) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM),
    )
}

#[cfg(windows)]
fn process_alive(pid: u32) -> Option<bool> {
    use std::{
        ffi::c_void,
        os::windows::io::{FromRawHandle, OwnedHandle},
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, exit_code: *mut u32) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    const STILL_ACTIVE: u32 = 259;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 258;

    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(ERROR_INVALID_PARAMETER) => Some(false),
            _ => None,
        };
    }
    // Own the handle returned by OpenProcess so every path closes it.
    let _process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut exit_code = 0;
    if unsafe { GetExitCodeProcess(raw, &mut exit_code) } == 0 {
        return None;
    }
    if exit_code != STILL_ACTIVE {
        return Some(false);
    }
    // A process can exit with code 259. A signalled process handle resolves that ambiguity.
    match unsafe { WaitForSingleObject(raw, 0) } {
        WAIT_OBJECT_0 => Some(false),
        WAIT_TIMEOUT => Some(true),
        _ => None,
    }
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_pid: u32) -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use serde_json::json;

    #[test]
    fn codex_uses_configured_root_and_unique_event_names() -> Result<()> {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        fs::write(
            root.path().join("config.toml"),
            r#"
documentation = """
[mcp_servers.fake]
[hooks.Fake]
"""
[mcp_servers."one.with.dots"]
command = "first"
[mcp_servers."one.with.dots".env]
PORT = "1234"
[mcp_servers.second] # protocol config
url = "https://example.invalid"
[[hooks.PostToolUse]]
matcher = "read"
[hooks]
PreCompact = []
[other.hooks.Ignored]
skip = true
"#,
        )?;
        fs::write(
            root.path().join("hooks.json"),
            r#"{"hooks":{"PostToolUse":[],"SubagentStop":[]}}"#,
        )?;
        let (counts, events) = codex_config(Some(root.path()));
        assert_eq!(counts.mcps, 2);
        assert_eq!(counts.hooks, codex_events::EVENTS.len() + 2);
        assert!(events.contains(&"PreCompact".into()));
        assert!(events.contains(&"SubagentStop".into()));
        assert!(!events.contains(&"Fake".into()));
        Ok(())
    }

    #[test]
    fn antigravity_reads_mcp_settings_and_plugin_hooks() -> Result<()> {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let plugin = root.path().join("plugins/test/hooks");
        fs::create_dir_all(&plugin)?;
        fs::write(
            root.path().join("mcp_config.json"),
            r#"{"mcpServers":{"a":{},"b":{}}}"#,
        )?;
        fs::write(
            root.path().join("settings.json"),
            r#"{"mcpServers":{"b":{}},"hooks":{"PreToolUse":[]}}"#,
        )?;
        fs::write(
            plugin.join("hooks.json"),
            r#"{"plugin-name":{"PreToolUse":[],"Stop":[]}}"#,
        )?;
        let (counts, hooks) = antigravity_config(&[root.path().into()]);
        assert_eq!(
            counts,
            ConfigCounts {
                mcps: 2,
                hooks: 2,
                claude_md: 0,
                rules: 0
            }
        );
        assert_eq!(hooks, ["PreToolUse", "Stop"]);
        Ok(())
    }

    #[test]
    fn external_agent_observes_session_directory_lifecycle() -> Result<()> {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let status = root.path().join("worker.json");
        fs::write(
            &status,
            json!({"version":1,"label":"worker","state":"running",
            "startedAt":"2026-11-21T12:00:00Z","updatedAt":"2026-11-21T12:00:00Z","pid":10})
            .to_string(),
        )?;
        let mut reader = ExternalAgents::default();
        let now = timestamp_ms(&json!("2026-11-21T12:00:00Z")).unwrap();
        let running = reader.read_with(root.path(), now, |_| Some(true));
        assert_eq!(running[0].id, "external:worker");
        assert_eq!(running[0].state, AgentState::Running);
        assert_eq!(
            reader.read_with(root.path(), now + 61_000., |_| None)[0]
                .detail
                .as_deref(),
            Some("状态已停止更新")
        );
        assert_eq!(
            reader.read_with(root.path(), now, |_| Some(false))[0]
                .detail
                .as_deref(),
            Some("监工进程已退出，任务中断")
        );
        fs::remove_file(status)?;
        reader.refreshed = None;
        assert!(
            reader
                .read_with(root.path(), now, |_| Some(false))
                .is_empty()
        );
        Ok(())
    }
}
