//! Per-agent configuration and capability descriptors for the built-in
//! context and KV-cache optimizer.
//!
//! Persisted as `<data_root>/context-optimizer.json` using atomic temp-file
//! replacement. Missing or malformed files degrade safely to defaults without
//! mutating disk state.

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

const CONFIG_FILE_NAME: &str = "context-optimizer.json";
const CONFIG_VERSION: u32 = 1;

pub const KNOWN_AGENT_IDS: [&str; 4] = ["claude-code", "antigravity", "codex", "dsh"];
pub const DEFAULT_MAX_LOG_BYTES: usize = 16_384;
pub const MIN_MAX_LOG_BYTES: usize = 4_096;
pub const MAX_MAX_LOG_BYTES: usize = 131_072;
pub const DEFAULT_KEEPALIVE_MAX_PER_TURN: u8 = 1;
pub const MAX_KEEPALIVE_PER_TURN: u8 = 5;

/// Runtime capability flags used when computing effective settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOptimizerCapabilities {
    pub sync_hook_intercept: bool,
    pub cache_monitor: bool,
    pub cache_keepalive: bool,
    pub cli_tools: bool,
}

impl AgentOptimizerCapabilities {
    pub fn for_agent(agent_id: &str) -> Self {
        match agent_id {
            "claude-code" | "antigravity" | "codex" => Self {
                sync_hook_intercept: true,
                cache_monitor: true,
                cache_keepalive: true,
                cli_tools: true,
            },
            "dsh" => Self {
                sync_hook_intercept: false,
                cache_monitor: true,
                cache_keepalive: false,
                cli_tools: true,
            },
            _ => Self {
                sync_hook_intercept: false,
                cache_monitor: true,
                cache_keepalive: false,
                cli_tools: true,
            },
        }
    }
}

/// Per-agent optimization switches.
///
/// The unqualified defaults also cover missing fields in older configurations.
/// New known agents use the agent-specific recommendations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOptimizerSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub noise_filter: bool,
    #[serde(default)]
    pub read_dedup: bool,
    #[serde(default = "default_true")]
    pub cache_monitor: bool,
    #[serde(default)]
    pub cache_expiry_notify: bool,
    #[serde(default)]
    pub cache_keepalive: bool,
    #[serde(default = "default_keepalive_max")]
    pub cache_keepalive_max_per_turn: u8,
    #[serde(default = "default_true")]
    pub cli_tools: bool,
    #[serde(default = "default_true")]
    pub block_sensitive_reads: bool,
    #[serde(default = "default_max_log_bytes")]
    pub max_log_bytes: usize,
}

fn default_true() -> bool {
    true
}

fn default_keepalive_max() -> u8 {
    DEFAULT_KEEPALIVE_MAX_PER_TURN
}

fn default_max_log_bytes() -> usize {
    DEFAULT_MAX_LOG_BYTES
}

impl Default for AgentOptimizerSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            noise_filter: false,
            read_dedup: false,
            cache_monitor: true,
            cache_expiry_notify: false,
            cache_keepalive: false,
            cache_keepalive_max_per_turn: DEFAULT_KEEPALIVE_MAX_PER_TURN,
            cli_tools: false,
            block_sensitive_reads: true,
            max_log_bytes: DEFAULT_MAX_LOG_BYTES,
        }
    }
}

impl AgentOptimizerSettings {
    pub fn recommended_for(agent_id: &str) -> Self {
        let interception = matches!(agent_id, "claude-code" | "antigravity");
        Self {
            noise_filter: interception,
            read_dedup: interception,
            block_sensitive_reads: interception,
            cli_tools: true,
            ..Self::default()
        }
    }

    /// Clamp numeric knobs and mask out switches unsupported by `agent_id`
    /// when computing effective runtime behavior.
    pub fn effective_for(&self, agent_id: &str) -> Self {
        let caps = AgentOptimizerCapabilities::for_agent(agent_id);
        let mut normalized = self.clone().normalized();
        if !normalized.enabled {
            normalized.noise_filter = false;
            normalized.read_dedup = false;
            normalized.cache_monitor = false;
            normalized.cache_expiry_notify = false;
            normalized.cache_keepalive = false;
            normalized.cli_tools = false;
            return normalized;
        }
        if !caps.sync_hook_intercept {
            normalized.noise_filter = false;
            normalized.read_dedup = false;
        }
        if !caps.cache_monitor {
            normalized.cache_monitor = false;
            normalized.cache_expiry_notify = false;
        }
        if !caps.cache_keepalive {
            normalized.cache_keepalive = false;
        }
        if !caps.cli_tools {
            normalized.cli_tools = false;
        }
        normalized
    }

    pub fn normalized(mut self) -> Self {
        self.cache_keepalive_max_per_turn = self
            .cache_keepalive_max_per_turn
            .clamp(1, MAX_KEEPALIVE_PER_TURN);
        self.max_log_bytes = self
            .max_log_bytes
            .clamp(MIN_MAX_LOG_BYTES, MAX_MAX_LOG_BYTES);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOptimizerEntry {
    pub agent_id: String,
    pub capabilities: AgentOptimizerCapabilities,
    pub settings: AgentOptimizerSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimizerToggle {
    MasterEnabled,
    NoiseFilter,
    ReadDedup,
    CacheMonitor,
    CacheExpiryNotify,
    CacheKeepalive,
    CliTools,
    BlockSensitiveReads,
}

impl OptimizerToggle {
    pub fn checked(self, settings: &AgentOptimizerSettings) -> bool {
        match self {
            Self::MasterEnabled => settings.enabled,
            Self::NoiseFilter => settings.noise_filter,
            Self::ReadDedup => settings.read_dedup,
            Self::CacheMonitor => settings.cache_monitor,
            Self::CacheExpiryNotify => settings.cache_expiry_notify,
            Self::CacheKeepalive => settings.cache_keepalive,
            Self::CliTools => settings.cli_tools,
            Self::BlockSensitiveReads => settings.block_sensitive_reads,
        }
    }
}

/// Availability in the settings panel. Keep this separate from the runtime
/// flags so disabling a control does not change saved or effective settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimizerAvailability {
    Available,
    Pending,
    Unsupported,
}

impl AgentOptimizerEntry {
    /// Reapply the available recommendations without discarding preferences
    /// for features that may become available in a later version.
    pub fn restored_recommendations(&self) -> AgentOptimizerSettings {
        use OptimizerToggle::*;

        let mut defaults = AgentOptimizerSettings::recommended_for(&self.agent_id);
        if self.availability(NoiseFilter) != OptimizerAvailability::Available {
            defaults.noise_filter = self.settings.noise_filter;
            defaults.max_log_bytes = self.settings.max_log_bytes;
        }
        if self.availability(ReadDedup) != OptimizerAvailability::Available {
            defaults.read_dedup = self.settings.read_dedup;
        }
        if self.availability(BlockSensitiveReads) != OptimizerAvailability::Available {
            defaults.block_sensitive_reads = self.settings.block_sensitive_reads;
        }
        if self.availability(CacheKeepalive) != OptimizerAvailability::Available {
            defaults.cache_keepalive = self.settings.cache_keepalive;
            defaults.cache_keepalive_max_per_turn = self.settings.cache_keepalive_max_per_turn;
        }
        if self.availability(CliTools) != OptimizerAvailability::Available {
            defaults.cli_tools = self.settings.cli_tools;
        }
        defaults
    }

    pub fn availability(&self, toggle: OptimizerToggle) -> OptimizerAvailability {
        use OptimizerAvailability::{Available, Pending, Unsupported};
        use OptimizerToggle::*;

        let available = match toggle {
            MasterEnabled => true,
            NoiseFilter | ReadDedup | BlockSensitiveReads => {
                return match self.agent_id.as_str() {
                    "claude-code" | "antigravity" => Available,
                    "codex" => Pending,
                    _ => Unsupported,
                };
            }
            CacheKeepalive => return Pending,
            CliTools => self.capabilities.cli_tools,
            CacheMonitor | CacheExpiryNotify => self.capabilities.cache_monitor,
        };
        if available { Available } else { Unsupported }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextOptimizerSnapshot {
    pub version: u32,
    pub exists: bool,
    pub path: PathBuf,
    pub agents: BTreeMap<String, AgentOptimizerEntry>,
}

#[derive(Debug)]
pub enum ContextOptimizerConfigError {
    InvalidAgentId(String),
    Io(io::Error),
    Serde(serde_json::Error),
}

impl fmt::Display for ContextOptimizerConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAgentId(id) => write!(f, "Invalid agent id for context optimizer: {id}"),
            Self::Io(err) => err.fmt(f),
            Self::Serde(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for ContextOptimizerConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidAgentId(_) => None,
            Self::Io(err) => Some(err),
            Self::Serde(err) => Some(err),
        }
    }
}

impl From<io::Error> for ContextOptimizerConfigError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for ContextOptimizerConfigError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serde(err)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedConfigFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    agents: BTreeMap<String, AgentOptimizerSettings>,
}

fn default_version() -> u32 {
    CONFIG_VERSION
}

/// File-backed store for per-agent context & cache optimizer preferences.
pub struct ContextOptimizerConfig {
    path: PathBuf,
}

impl ContextOptimizerConfig {
    pub fn for_data_root(data_root: impl AsRef<Path>) -> Self {
        Self {
            path: data_root.as_ref().join(CONFIG_FILE_NAME),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the current snapshot. Missing or malformed JSON safely falls back
    /// to defaults without writing to disk.
    pub fn snapshot(&self) -> io::Result<ContextOptimizerSnapshot> {
        let (exists, persisted) = self.read_persisted()?;
        Ok(self.build_snapshot(exists, persisted.agents))
    }

    /// Returns the effective settings for a single agent (clamped & capability-masked).
    pub fn effective_for(&self, agent_id: &str) -> io::Result<AgentOptimizerSettings> {
        let snap = self.snapshot()?;
        let raw = snap
            .agents
            .get(agent_id)
            .map(|entry| entry.settings.clone())
            .unwrap_or_default();
        Ok(raw.effective_for(agent_id))
    }

    /// Persists settings for one agent atomically and returns the updated snapshot.
    pub fn set_agent_settings(
        &self,
        agent_id: &str,
        settings: AgentOptimizerSettings,
    ) -> Result<ContextOptimizerSnapshot, ContextOptimizerConfigError> {
        let clean_id = normalize_agent_id(agent_id)?;
        let (_, mut persisted) = self.read_persisted()?;
        persisted.version = CONFIG_VERSION;
        persisted
            .agents
            .insert(clean_id.clone(), settings.normalized());
        self.write_persisted(&persisted)?;
        Ok(self.build_snapshot(true, persisted.agents))
    }

    fn read_persisted(&self) -> io::Result<(bool, PersistedConfigFile)> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok((false, PersistedConfigFile::default()));
            }
            Err(err) => return Err(err),
        };
        match serde_json::from_slice::<PersistedConfigFile>(&bytes) {
            Ok(parsed) if parsed.version == CONFIG_VERSION => Ok((true, parsed)),
            _ => Ok((false, PersistedConfigFile::default())),
        }
    }

    fn write_persisted(
        &self,
        persisted: &PersistedConfigFile,
    ) -> Result<(), ContextOptimizerConfigError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let encoded = serde_json::to_vec_pretty(persisted)?;
        let tmp_name = format!(".{CONFIG_FILE_NAME}.{}.tmp", Uuid::new_v4());
        let tmp_path = self.path.with_file_name(tmp_name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        file.write_all(&encoded)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }

    fn build_snapshot(
        &self,
        exists: bool,
        mut saved: BTreeMap<String, AgentOptimizerSettings>,
    ) -> ContextOptimizerSnapshot {
        let mut agents = BTreeMap::new();
        for id in KNOWN_AGENT_IDS {
            let settings = saved
                .remove(id)
                .unwrap_or_else(|| AgentOptimizerSettings::recommended_for(id))
                .normalized();
            agents.insert(
                id.to_owned(),
                AgentOptimizerEntry {
                    agent_id: id.to_owned(),
                    capabilities: AgentOptimizerCapabilities::for_agent(id),
                    settings,
                },
            );
        }
        // Preserve any future/custom agent ids already stored on disk.
        for (id, settings) in saved {
            agents.insert(
                id.clone(),
                AgentOptimizerEntry {
                    capabilities: AgentOptimizerCapabilities::for_agent(&id),
                    agent_id: id,
                    settings: settings.normalized(),
                },
            );
        }
        ContextOptimizerSnapshot {
            version: CONFIG_VERSION,
            exists,
            path: self.path.clone(),
            agents,
        }
    }
}

fn normalize_agent_id(raw: &str) -> Result<String, ContextOptimizerConfigError> {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed.is_empty()
        || trimmed.len() > 48
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(ContextOptimizerConfigError::InvalidAgentId(raw.to_owned()));
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_recommendations_for_all_known_agents() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextOptimizerConfig::for_data_root(dir.path());
        let snap = store.snapshot().unwrap();
        assert!(!snap.exists);
        for id in KNOWN_AGENT_IDS {
            let entry = snap.agents.get(id).expect("known agent present");
            assert!(entry.settings.enabled);
            assert!(entry.settings.cache_monitor);
            let availability = match id {
                "claude-code" | "antigravity" => OptimizerAvailability::Available,
                "codex" => OptimizerAvailability::Pending,
                _ => OptimizerAvailability::Unsupported,
            };
            let interception = availability == OptimizerAvailability::Available;
            for toggle in [
                OptimizerToggle::NoiseFilter,
                OptimizerToggle::ReadDedup,
                OptimizerToggle::BlockSensitiveReads,
            ] {
                assert_eq!(entry.availability(toggle), availability);
            }
            assert_eq!(entry.settings.noise_filter, interception);
            assert_eq!(entry.settings.read_dedup, interception);
            assert_eq!(entry.settings.block_sensitive_reads, interception);
            assert!(!entry.settings.cache_keepalive);
            assert!(!entry.settings.cache_expiry_notify);
            assert!(entry.settings.cli_tools);
        }
        assert!(snap.agents["claude-code"].capabilities.sync_hook_intercept);
        assert!(snap.agents["antigravity"].capabilities.sync_hook_intercept);
        assert!(snap.agents["codex"].capabilities.sync_hook_intercept);
        assert!(!snap.agents["dsh"].capabilities.sync_hook_intercept);
    }

    #[test]
    fn persists_per_agent_settings_and_clamps_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextOptimizerConfig::for_data_root(dir.path());
        let updated = AgentOptimizerSettings {
            enabled: true,
            noise_filter: true,
            read_dedup: true,
            cache_monitor: true,
            cache_expiry_notify: true,
            cache_keepalive: true,
            cache_keepalive_max_per_turn: 99,
            cli_tools: true,
            block_sensitive_reads: true,
            max_log_bytes: 999_999,
        };
        let snap = store.set_agent_settings("codex", updated).unwrap();
        assert!(snap.exists);
        let codex = &snap.agents["codex"].settings;
        assert!(codex.noise_filter);
        assert!(codex.cli_tools);
        assert_eq!(codex.cache_keepalive_max_per_turn, MAX_KEEPALIVE_PER_TURN);
        assert_eq!(codex.max_log_bytes, MAX_MAX_LOG_BYTES);

        // Other agents remain untouched.
        assert_eq!(
            snap.agents["claude-code"].settings,
            AgentOptimizerSettings::recommended_for("claude-code")
        );

        // Reload from disk verifies persistence.
        let reloaded = store.snapshot().unwrap();
        assert_eq!(reloaded.agents["codex"].settings, *codex);
    }

    #[test]
    fn old_settings_keep_saved_choices_and_default_newly_exposed_switches() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextOptimizerConfig::for_data_root(dir.path());
        fs::write(
            store.path(),
            r#"{"version":1,"agents":{"claude-code":{"noiseFilter":true,"cacheKeepalive":true},"codex":{"readDedup":true,"blockSensitiveReads":false}}}"#,
        )
        .unwrap();

        let snapshot = store.snapshot().unwrap();
        assert!(snapshot.exists);
        let claude = &snapshot.agents["claude-code"].settings;
        assert!(claude.noise_filter);
        assert!(claude.cache_keepalive);
        assert!(!claude.cache_expiry_notify);
        assert!(claude.block_sensitive_reads);
        assert!(claude.cli_tools);
        let codex = &snapshot.agents["codex"].settings;
        assert!(codex.read_dedup);
        assert!(!codex.block_sensitive_reads);
    }

    #[test]
    fn legacy_settings_load_without_reemitting_removed_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextOptimizerConfig::for_data_root(dir.path());
        fs::write(
            store.path(),
            r#"{"version":1,"agents":{"claude-code":{"stallRescue": true,"cacheMonitor":false}}}"#,
        )
        .unwrap();

        let snapshot = store.snapshot().unwrap();
        assert!(snapshot.exists);
        let entry = &snapshot.agents["claude-code"];
        assert!(!entry.settings.cache_monitor);
        let serialized = serde_json::to_value(entry).unwrap();
        assert!(serialized["settings"].get("stallRescue").is_none());
        assert!(serialized["capabilities"].get("stallRescue").is_none());

        store
            .set_agent_settings("claude-code", entry.settings.clone())
            .unwrap();
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        let settings = &persisted["agents"]["claude-code"];
        assert_eq!(settings["cacheMonitor"], false);
        assert!(settings.get("stallRescue").is_none());
    }

    #[test]
    fn master_switch_off_and_capabilities_mask_effective_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextOptimizerConfig::for_data_root(dir.path());

        let dsh_all_on = AgentOptimizerSettings {
            enabled: true,
            noise_filter: true,
            read_dedup: true,
            cache_keepalive: true,
            cli_tools: true,
            ..Default::default()
        };
        store.set_agent_settings("dsh", dsh_all_on).unwrap();
        let eff_dsh = store.effective_for("dsh").unwrap();
        assert!(!eff_dsh.noise_filter);
        assert!(!eff_dsh.read_dedup);
        assert!(!eff_dsh.cache_keepalive);
        assert!(eff_dsh.cli_tools);

        let claude_off = AgentOptimizerSettings {
            enabled: false,
            noise_filter: true,
            cache_monitor: true,
            ..Default::default()
        };
        store.set_agent_settings("claude-code", claude_off).unwrap();
        let eff_claude = store.effective_for("claude-code").unwrap();
        assert!(!eff_claude.enabled);
        assert!(!eff_claude.noise_filter);
        assert!(!eff_claude.cache_monitor);
    }

    #[test]
    fn rejects_invalid_agent_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContextOptimizerConfig::for_data_root(dir.path());
        assert!(
            store
                .set_agent_settings("../escape", AgentOptimizerSettings::default())
                .is_err()
        );
    }
}
