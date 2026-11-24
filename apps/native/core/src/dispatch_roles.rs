//! Matou-local ecological niche and per-agent default configuration for task dispatch.
//!
//! Stored inside Matou's own `data_root/dispatch-roles.json` so it only applies
//! inside Matou-managed sessions and never modifies external foreman configs.

use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const ROLES_FILENAME: &str = "dispatch-roles.json";

pub const CODEX_MODELS: [&str; 5] = ["sol", "best", "astra", "terra", "luna"];
pub const CODEX_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];
pub const CODEX_SPEEDS: [&str; 5] = ["default", "fast", "ultrafast", "priority", "flex"];

pub const DSH_MODELS: [&str; 2] = ["deepseek-flash", "deepseek-v4-pro"];
pub const DSH_EFFORTS: [&str; 4] = ["off", "low", "high", "max"];

pub const AGY_MODELS: [&str; 4] = ["gemini-pro-orchestrator", "hydrogen", "argon", "carbon"];
pub const AGY_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DispatchRoleMode {
    Auto,
    Explicit,
}

impl DispatchRoleMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Explicit => "explicit",
        }
    }

    pub const fn label_zh(self) -> &'static str {
        match self {
            Self::Auto => "按职责自动派",
            Self::Explicit => "仅点名时派",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRoleConfig {
    pub id: String,
    pub enabled: bool,
    pub mode: DispatchRoleMode,
    pub model: String,
    pub effort: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchRolesConfig {
    pub version: u32,
    pub allow_sub_dispatch: bool,
    pub agents: Vec<AgentRoleConfig>,
}

impl Default for DispatchRolesConfig {
    fn default() -> Self {
        Self {
            version: 1,
            allow_sub_dispatch: true,
            agents: default_agents(),
        }
    }
}

pub fn default_agents() -> Vec<AgentRoleConfig> {
    vec![
        AgentRoleConfig {
            id: "codex".into(),
            enabled: true,
            mode: DispatchRoleMode::Auto,
            model: "sol".into(),
            effort: "high".into(),
            speed: Some("default".into()),
            role: "方案定好后的编码落地、测试、重构、源码调研、数据处理与打包。".into(),
        },
        AgentRoleConfig {
            id: "dsh".into(),
            enabled: true,
            mode: DispatchRoleMode::Auto,
            model: "deepseek-flash".into(),
            effort: "high".into(),
            speed: None,
            role: "前端 UI 设计与界面实现：页面布局、视觉样式、交互外观与组件实现。".into(),
        },
        AgentRoleConfig {
            id: "agy".into(),
            enabled: true,
            mode: DispatchRoleMode::Explicit,
            model: "gemini-pro-orchestrator".into(),
            effort: "high".into(),
            speed: None,
            role: "深度咨询与方案评审：复杂问题第二意见、跨领域架构对比、长上下文调研。".into(),
        },
    ]
}

pub fn agent_display_name(id: &str) -> &str {
    match id {
        "codex" => "Codex",
        "dsh" => "DSH",
        "agy" => "Antigravity（反重力）",
        other => other,
    }
}

pub fn model_options(id: &str) -> &'static [(&'static str, &'static str)] {
    match id {
        "codex" => &[
            ("sol", "sol（主力最新版）"),
            ("best", "best（跨系列最高）"),
            ("astra", "astra（前沿）"),
            ("terra", "terra（均衡）"),
            ("luna", "luna（快速）"),
        ],
        "dsh" => &[
            ("deepseek-flash", "deepseek-flash（默认）"),
            ("deepseek-v4-pro", "deepseek-v4-pro"),
        ],
        "agy" => &[
            ("gemini-pro-orchestrator", "Orchestrator（默认）"),
            ("hydrogen", "hydrogen（系列最新）"),
            ("argon", "argon（系列最新）"),
            ("carbon", "carbon（系列最新）"),
        ],
        _ => &[],
    }
}

pub fn effort_options(id: &str) -> &'static [(&'static str, &'static str)] {
    match id {
        "dsh" => &[("off", "关闭"), ("low", "低"), ("high", "高"), ("max", "最高")],
        "codex" | "agy" => &[
            ("low", "低"),
            ("medium", "中"),
            ("high", "高"),
            ("xhigh", "超高"),
        ],
        _ => &[],
    }
}

pub fn speed_options(id: &str) -> &'static [(&'static str, &'static str)] {
    match id {
        "codex" => &[
            ("default", "普通（默认）"),
            ("fast", "快速"),
            ("ultrafast", "超快"),
            ("priority", "优先"),
            ("flex", "弹性"),
        ],
        _ => &[],
    }
}

pub fn effort_is_fixed(id: &str, model: &str) -> bool {
    id == "agy" && model == "gemini-pro-orchestrator"
}

fn valid_model(id: &str, value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 64 || trimmed.chars().any(char::is_whitespace) {
        return false;
    }
    match id {
        "codex" => CODEX_MODELS.contains(&trimmed) || !trimmed.is_empty(),
        "dsh" => DSH_MODELS.contains(&trimmed) || !trimmed.is_empty(),
        "agy" => AGY_MODELS.contains(&trimmed) || !trimmed.is_empty(),
        _ => true,
    }
}

fn valid_effort(id: &str, value: &str) -> bool {
    match id {
        "codex" => CODEX_EFFORTS.contains(&value),
        "dsh" => DSH_EFFORTS.contains(&value),
        "agy" => AGY_EFFORTS.contains(&value),
        _ => !value.is_empty(),
    }
}

fn valid_speed(id: &str, value: &str) -> bool {
    match id {
        "codex" => CODEX_SPEEDS.contains(&value),
        _ => false,
    }
}

fn normalize_config(mut input: DispatchRolesConfig) -> DispatchRolesConfig {
    let defaults = default_agents();
    let mut normalized = Vec::with_capacity(defaults.len().max(input.agents.len()));
    for default_agent in defaults {
        if let Some(existing) = input
            .agents
            .iter()
            .find(|candidate| candidate.id == default_agent.id)
        {
            let model = if valid_model(&default_agent.id, &existing.model) {
                existing.model.trim().to_owned()
            } else {
                default_agent.model
            };
            let effort = if valid_effort(&default_agent.id, &existing.effort) {
                existing.effort.clone()
            } else {
                default_agent.effort
            };
            let speed = if default_agent.id == "codex" {
                let candidate = existing.speed.as_deref().unwrap_or("default");
                Some(if valid_speed("codex", candidate) {
                    candidate.to_owned()
                } else {
                    "default".to_owned()
                })
            } else {
                None
            };
            let role = if existing.role.trim().is_empty() {
                default_agent.role
            } else {
                existing.role.trim().to_owned()
            };
            normalized.push(AgentRoleConfig {
                id: default_agent.id,
                enabled: existing.enabled,
                mode: existing.mode,
                model,
                effort,
                speed,
                role,
            });
        } else {
            normalized.push(default_agent);
        }
    }
    // Preserve any future/custom agents appended to the file.
    for extra in input.agents.drain(..) {
        if !normalized.iter().any(|item| item.id == extra.id) && !extra.id.trim().is_empty() {
            normalized.push(extra);
        }
    }
    DispatchRolesConfig {
        version: 1,
        allow_sub_dispatch: input.allow_sub_dispatch,
        agents: normalized,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DispatchRolesSnapshot {
    pub exists: bool,
    pub path: PathBuf,
    pub config: DispatchRolesConfig,
}

#[derive(Debug)]
pub enum DispatchRolesError {
    NoEnabledAgent,
    Io(io::Error),
}

impl fmt::Display for DispatchRolesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoEnabledAgent => write!(f, "至少保留一个启用的执行者"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DispatchRolesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::NoEnabledAgent => None,
        }
    }
}

impl From<io::Error> for DispatchRolesError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct DispatchRolesStore {
    path: PathBuf,
}

impl DispatchRolesStore {
    pub fn for_data_root(data_root: impl AsRef<Path>) -> Self {
        Self {
            path: data_root.as_ref().join(ROLES_FILENAME),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn snapshot(&self) -> io::Result<DispatchRolesSnapshot> {
        let fallback = DispatchRolesSnapshot {
            exists: false,
            path: self.path.clone(),
            config: DispatchRolesConfig::default(),
        };
        let content = match fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(fallback),
            Err(error) => return Err(error),
        };
        let parsed = match serde_json::from_str::<DispatchRolesConfig>(&content) {
            Ok(parsed) if parsed.version == 1 => parsed,
            _ => return Ok(fallback),
        };
        Ok(DispatchRolesSnapshot {
            exists: true,
            path: self.path.clone(),
            config: normalize_config(parsed),
        })
    }

    pub fn save(
        &self,
        config: DispatchRolesConfig,
    ) -> Result<DispatchRolesSnapshot, DispatchRolesError> {
        let normalized = normalize_config(config);
        if !normalized.agents.iter().any(|agent| agent.enabled) {
            return Err(DispatchRolesError::NoEnabledAgent);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = format!(
            "{}\n",
            serde_json::to_string_pretty(&normalized).expect("serialize dispatch roles")
        );
        let temporary = self.path.with_file_name(format!(
            "{}.{}.tmp",
            ROLES_FILENAME,
            Uuid::new_v4()
        ));
        let write_result = (|| -> io::Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(body.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            return Err(DispatchRolesError::Io(error));
        }
        Ok(DispatchRolesSnapshot {
            exists: true,
            path: self.path.clone(),
            config: normalized,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_returns_defaults_and_save_round_trips() {
        let root = tempfile::tempdir().unwrap();
        let store = DispatchRolesStore::for_data_root(root.path());
        let initial = store.snapshot().unwrap();
        assert!(!initial.exists);
        assert!(initial.config.allow_sub_dispatch);
        assert_eq!(initial.config.agents.len(), 3);
        assert_eq!(initial.config.agents[0].id, "codex");
        assert_eq!(initial.config.agents[0].speed.as_deref(), Some("default"));
        assert_eq!(initial.config.agents[2].mode, DispatchRoleMode::Explicit);

        let mut updated = initial.config;
        updated.allow_sub_dispatch = false;
        updated.agents[0].speed = Some("ultrafast".into());
        updated.agents[2].mode = DispatchRoleMode::Auto;
        updated.agents[2].model = "hydrogen".into();
        updated.agents[2].effort = "xhigh".into();
        updated.agents[2].role = "专门负责架构咨询和评审。".into();

        let saved = store.save(updated.clone()).unwrap();
        assert!(saved.exists);
        assert_eq!(saved.config, updated);

        let reread = store.snapshot().unwrap();
        assert!(reread.exists);
        assert_eq!(reread.config, updated);
    }

    #[test]
    fn rejects_disabling_all_agents() {
        let root = tempfile::tempdir().unwrap();
        let store = DispatchRolesStore::for_data_root(root.path());
        let mut config = DispatchRolesConfig::default();
        for agent in &mut config.agents {
            agent.enabled = false;
        }
        assert!(matches!(
            store.save(config),
            Err(DispatchRolesError::NoEnabledAgent)
        ));
    }

    #[test]
    fn preserves_future_agents_and_sanitizes_invalid_values() {
        let root = tempfile::tempdir().unwrap();
        let store = DispatchRolesStore::for_data_root(root.path());
        fs::write(
            store.path(),
            r#"{
              "version": 1,
              "allowSubDispatch": true,
              "agents": [
                { "id": "codex", "enabled": true, "mode": "auto", "model": "sol", "effort": "invalid", "speed": "bogus", "role": "  " },
                { "id": "kimi", "enabled": true, "mode": "auto", "model": "kimi-k2", "effort": "high", "role": "长文档整理" }
              ]
            }"#,
        )
        .unwrap();
        let snap = store.snapshot().unwrap();
        assert!(snap.exists);
        assert_eq!(snap.config.agents.len(), 4);
        assert_eq!(snap.config.agents[0].effort, "high");
        assert_eq!(snap.config.agents[0].speed.as_deref(), Some("default"));
        assert!(!snap.config.agents[0].role.is_empty());
        assert_eq!(snap.config.agents[3].id, "kimi");
    }
}
