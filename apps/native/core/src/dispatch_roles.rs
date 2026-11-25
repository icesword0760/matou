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
pub const BUILTIN_AGENTS: [&str; 3] = ["codex", "dsh", "agy"];
pub const ROLE_MAX_CHARS: usize = 2000;

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DispatchRolesConfig {
    pub version: u32,
    pub allow_sub_dispatch: bool,
    pub default_agent: String,
    #[serde(serialize_with = "crate::dispatch_config::serialize_minutes")]
    pub idle_min: f64,
    #[serde(serialize_with = "crate::dispatch_config::serialize_minutes")]
    pub max_min: f64,
    #[serde(serialize_with = "crate::dispatch_config::serialize_minutes")]
    pub ask_wait_min: f64,
    pub retries: u8,
    pub agents: Vec<AgentRoleConfig>,
}

impl Default for DispatchRolesConfig {
    fn default() -> Self {
        Self {
            version: 1,
            allow_sub_dispatch: true,
            default_agent: "codex".into(),
            idle_min: 10.,
            max_min: 45.,
            ask_wait_min: 10.,
            retries: 3,
            agents: default_agents(),
        }
    }
}

pub fn default_role_text(id: &str) -> &'static str {
    match id {
        "codex" => {
            "方案定好后的一切落地：细节设计、编码与测试、重构、源码阅读与调研、图片/媒体生成、数据处理与打包；电脑界面操作（含 Blender）自动派 terra 最新版子代理完成。"
        }
        "dsh" => {
            "前端 UI 设计与界面代码实现：页面和组件的视觉设计、布局、样式、交互外观，以及将其落地为前端界面代码。"
        }
        "agy" => {
            "用户点名时接手落地执行：编码、调研与复杂任务执行（Orchestrator 可自派子代理并行推进；额度耗尽或型号不可用时直接停下汇报，不自动换人；纯提问参谋请走 asking-antigravity）。"
        }
        _ => "",
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
            role: default_role_text("codex").into(),
        },
        AgentRoleConfig {
            id: "dsh".into(),
            enabled: true,
            mode: DispatchRoleMode::Auto,
            model: "deepseek-flash".into(),
            effort: "high".into(),
            speed: None,
            role: default_role_text("dsh").into(),
        },
        AgentRoleConfig {
            id: "agy".into(),
            enabled: true,
            mode: DispatchRoleMode::Explicit,
            model: "gemini-pro-orchestrator".into(),
            effort: "high".into(),
            speed: None,
            role: default_role_text("agy").into(),
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
    id == "agy" && model.to_ascii_lowercase().contains("orchestrator")
}

fn valid_model(id: &str, value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed.chars().any(char::is_whitespace) {
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
        "codex" => CODEX_EFFORTS.contains(&value) || ["none", "minimal", "max"].contains(&value),
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
                existing.role.trim().replace("\r\n", "\n")
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
            normalized.push(AgentRoleConfig {
                role: extra.role.replace("\r\n", "\n"),
                ..extra
            });
        }
    }
    input.version = 1;
    input.agents = normalized;
    input
}

impl DispatchRolesConfig {
    pub fn normalize_default_agent(&mut self) {
        let mut enabled = self
            .agents
            .iter_mut()
            .filter(|a| a.enabled && BUILTIN_AGENTS.contains(&a.id.as_str()));
        if let Some(agent) = enabled.find(|a| a.id == self.default_agent) {
            agent.mode = DispatchRoleMode::Auto;
        } else if let Some(agent) = self
            .agents
            .iter_mut()
            .find(|a| a.enabled && BUILTIN_AGENTS.contains(&a.id.as_str()))
        {
            self.default_agent.clone_from(&agent.id);
            agent.mode = DispatchRoleMode::Auto;
        }
    }

    /// Eligible recipients across all enabled senders. A named-only agent
    /// may send work to an auto-dispatch peer when explicitly selected.
    pub fn sub_dispatch_targets(&self) -> impl Iterator<Item = &AgentRoleConfig> {
        let has_peer = self
            .agents
            .iter()
            .filter(|agent| agent.enabled)
            .nth(1)
            .is_some();
        self.agents
            .iter()
            .filter(move |agent| has_peer && agent.enabled && agent.mode == DispatchRoleMode::Auto)
    }
}

pub fn effort_label_zh(effort: &str) -> &str {
    match effort {
        "off" => "关闭",
        "low" => "低",
        "medium" => "中",
        "high" => "高",
        "xhigh" => "超高",
        "max" => "最高",
        "none" => "无",
        "minimal" => "极低",
        other => other,
    }
}

pub fn agy_effort_suffix(model: &str) -> Option<&str> {
    model
        .rsplit_once('-')
        .map(|(_, suffix)| suffix)
        .filter(|suffix| AGY_EFFORTS.contains(suffix))
}

pub fn format_roles_markdown(config: &DispatchRolesConfig) -> String {
    let mut lines = vec![
        "## 用户自定义 Agent 生态位与默认配置（以本节为准）".to_owned(),
        String::new(),
    ];
    for agent in config.agents.iter().filter(|agent| agent.enabled) {
        let label = agent_display_name(&agent.id);
        let tag = if agent.id == config.default_agent {
            "默认兜底主力"
        } else {
            agent.mode.label_zh()
        };
        let mut specs = vec![format!("模型: {}", agent.model)];
        if effort_is_fixed(&agent.id, &agent.model) {
            specs.push("无推理档位".into());
        } else {
            let effort = if agent.id == "agy" {
                agy_effort_suffix(&agent.model).unwrap_or(&agent.effort)
            } else {
                &agent.effort
            };
            specs.push(format!("推理: {}", effort_label_zh(effort)));
        }
        if agent.id == "codex" {
            if let Some((_, label)) = speed_options("codex")
                .iter()
                .find(|(value, _)| *value != "default" && agent.speed.as_deref() == Some(*value))
            {
                specs.push(format!("速度: {label}"));
            }
        }
        let role = if agent.role.trim().is_empty() {
            let default = default_role_text(&agent.id);
            if default.is_empty() {
                "按默认职责执行。"
            } else {
                default
            }
        } else {
            agent.role.trim()
        }
        .replace("\r\n", "\n");
        // Older settings allowed longer roles. Keep the stored text editable
        // and bound only the instructions sent to provider processes.
        let end = role
            .char_indices()
            .nth(ROLE_MAX_CHARS)
            .map_or(role.len(), |(index, _)| index);
        let role = role[..end].replace('\n', "\n  ");
        lines.push(format!(
            "- **{label}**（{tag} | {}）：{role}",
            specs.join(" · ")
        ));
    }
    let disabled = config
        .agents
        .iter()
        .filter(|a| !a.enabled)
        .map(|a| agent_display_name(&a.id))
        .collect::<Vec<_>>();
    if !disabled.is_empty() {
        lines.push(format!("- **已停用（禁止派活）**：{}", disabled.join("、")));
    }
    let auto_names = config
        .sub_dispatch_targets()
        .map(|agent| agent_display_name(&agent.id))
        .collect::<Vec<_>>();
    let policy = if !config.allow_sub_dispatch {
        "已关闭（所有执行者禁止二次转包，只允许顶层监工派活；Orchestrator 模型内部自带的并行子代理不受影响）".to_owned()
    } else if auto_names.is_empty() {
        "当前无可转派协作者（需至少启用两个执行者且目标协作者处于「按职责自动派」，当前仅由顶层监工直接派活）".to_owned()
    } else {
        format!(
            "允许当前执行者遇到其他「启用 + 按职责自动派」协作者（{}）职责范围内的子任务时，跨执行者转派 1 层子任务",
            auto_names.join("、")
        )
    };
    lines.push(format!("- **二次转包策略**：{policy}"));
    lines.join("\n")
}

fn has_enabled_agent(config: &DispatchRolesConfig) -> bool {
    config
        .agents
        .iter()
        .any(|a| a.enabled && BUILTIN_AGENTS.contains(&a.id.as_str()))
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DispatchRolesSnapshot {
    pub exists: bool,
    pub path: PathBuf,
    pub config: DispatchRolesConfig,
}

#[derive(Debug)]
pub enum DispatchRolesError {
    NoEnabledAgent,
    InvalidRange(&'static str, &'static str),
    RoleTooLong(String),
    Io(io::Error),
}

impl fmt::Display for DispatchRolesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoEnabledAgent => write!(f, "至少保留一个启用的执行者"),
            Self::InvalidRange(field, range) => write!(f, "{field}必须在 {range} 范围内"),
            Self::RoleTooLong(id) => write!(
                f,
                "{} 的职责文案最多 {ROLE_MAX_CHARS} 字",
                agent_display_name(id)
            ),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DispatchRolesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
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
        let mut config = normalize_config(parsed);
        if !has_enabled_agent(&config) {
            config.agents[0].enabled = true;
        }
        config.normalize_default_agent();
        let defaults = DispatchRolesConfig::default();
        if !(1. ..=60.).contains(&config.idle_min) {
            config.idle_min = defaults.idle_min;
        }
        if !(5. ..=180.).contains(&config.max_min) {
            config.max_min = defaults.max_min;
        }
        if !(1. ..=60.).contains(&config.ask_wait_min) {
            config.ask_wait_min = defaults.ask_wait_min;
        }
        if config.retries > 5 {
            config.retries = defaults.retries;
        }
        Ok(DispatchRolesSnapshot {
            exists: true,
            path: self.path.clone(),
            config,
        })
    }

    pub fn save(
        &self,
        config: DispatchRolesConfig,
    ) -> Result<DispatchRolesSnapshot, DispatchRolesError> {
        for (valid, field, range) in [
            ((1. ..=60.).contains(&config.idle_min), "没动静判卡", "1–60"),
            ((5. ..=180.).contains(&config.max_min), "单轮最长", "5–180"),
            ((1. ..=60.).contains(&config.ask_wait_min), "等回答", "1–60"),
            (config.retries <= 5, "重试次数", "0–5"),
        ] {
            if !valid {
                return Err(DispatchRolesError::InvalidRange(field, range));
            }
        }
        if let Some(agent) = config
            .agents
            .iter()
            .find(|a| a.role.replace("\r\n", "\n").chars().count() > ROLE_MAX_CHARS)
        {
            return Err(DispatchRolesError::RoleTooLong(agent.id.clone()));
        }
        let mut normalized = normalize_config(config);
        if !has_enabled_agent(&normalized) {
            return Err(DispatchRolesError::NoEnabledAgent);
        }
        normalized.normalize_default_agent();
        write_dispatch_json(&self.path, &normalized)?;
        Ok(DispatchRolesSnapshot {
            exists: true,
            path: self.path.clone(),
            config: normalized,
        })
    }
}

pub(crate) fn write_dispatch_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = format!("{}\n", serde_json::to_string_pretty(value)?);
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let result = (|| {
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
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
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
    fn old_config_recovers_default_agent_and_supervision_fields() {
        let root = tempfile::tempdir().unwrap();
        let store = DispatchRolesStore::for_data_root(root.path());
        let mut legacy = serde_json::to_value(DispatchRolesConfig::default()).unwrap();
        for field in ["defaultAgent", "idleMin", "maxMin", "askWaitMin", "retries"] {
            legacy.as_object_mut().unwrap().remove(field);
        }
        legacy["agents"][0]["enabled"] = false.into();
        legacy["agents"][1]["enabled"] = false.into();
        fs::write(store.path(), legacy.to_string()).unwrap();
        let mut config = store.snapshot().unwrap().config;
        assert_eq!(config.default_agent, "agy");
        assert_eq!(config.agents[2].mode, DispatchRoleMode::Auto);
        assert_eq!(
            (
                config.idle_min,
                config.max_min,
                config.ask_wait_min,
                config.retries
            ),
            (10., 45., 10., 3)
        );
        config.agents[2].role = "第一行\r\n第二行".into();
        config.max_min = 12.5;
        let saved = store.save(config).unwrap().config;
        assert_eq!(saved.agents[2].role, "第一行\n第二行");
        let json = fs::read_to_string(store.path()).unwrap();
        assert!(json.contains("\"idleMin\": 10,"));
        assert!(json.contains("\"maxMin\": 12.5,"));

        legacy["agents"][2]["enabled"] = false.into();
        fs::write(store.path(), legacy.to_string()).unwrap();
        let recovered = store.snapshot().unwrap().config;
        assert_eq!(recovered.default_agent, "codex");
        assert!(recovered.agents[0].enabled);
    }

    #[test]
    fn validates_supervision_and_role_length_without_replacing_saved_file() {
        let root = tempfile::tempdir().unwrap();
        let store = DispatchRolesStore::for_data_root(root.path());
        let mut config = DispatchRolesConfig::default();
        store.save(config.clone()).unwrap();
        config.idle_min = f64::NAN;
        assert!(matches!(
            store.save(config.clone()),
            Err(DispatchRolesError::InvalidRange(..))
        ));
        config.idle_min = 10.;
        config.agents[0].role = "字".repeat(ROLE_MAX_CHARS + 1);
        assert!(matches!(
            store.save(config),
            Err(DispatchRolesError::RoleTooLong(_))
        ));
        assert_eq!(
            store.snapshot().unwrap().config,
            DispatchRolesConfig::default()
        );
    }

    #[test]
    fn keeps_legacy_overlong_role_editable_and_bounds_injection() {
        let root = tempfile::tempdir().unwrap();
        let store = DispatchRolesStore::for_data_root(root.path());
        let mut legacy = DispatchRolesConfig::default();
        legacy.agents[0].role = format!("{}尾", "字".repeat(ROLE_MAX_CHARS));
        fs::write(store.path(), serde_json::to_vec(&legacy).unwrap()).unwrap();
        let snapshot = store.snapshot().unwrap();
        assert_eq!(snapshot.config.agents[0].role, legacy.agents[0].role);
        assert!(!format_roles_markdown(&snapshot.config).contains('尾'));
        assert!(matches!(
            store.save(snapshot.config),
            Err(DispatchRolesError::RoleTooLong(_))
        ));
    }

    #[test]
    fn markdown_renders_foreman_tags_and_effective_effort() {
        let mut config = DispatchRolesConfig::default();
        config.default_agent = "agy".into();
        config.normalize_default_agent();
        config.agents[0].speed = Some("ultrafast".into());
        config.agents[0].role = "编码".into();
        config.agents[1].enabled = false;
        config.agents[2].model = "gemini-4-neon-xhigh".into();
        config.agents[2].role = "执行".into();
        assert_eq!(
            format_roles_markdown(&config),
            concat!(
                "## 用户自定义 Agent 生态位与默认配置（以本节为准）\n\n",
                "- **Codex**（按职责自动派 | 模型: sol · 推理: 高 · 速度: 超快）：编码\n",
                "- **Antigravity（反重力）**（默认兜底主力 | 模型: gemini-4-neon-xhigh · 推理: 超高）：执行\n",
                "- **已停用（禁止派活）**：DSH\n",
                "- **二次转包策略**：允许当前执行者遇到其他「启用 + 按职责自动派」协作者（Codex、Antigravity（反重力））职责范围内的子任务时，跨执行者转派 1 层子任务",
            )
        );
        config.agents[2].model = "gemini-pro-orchestrator-neon".into();
        config.allow_sub_dispatch = false;
        let markdown = format_roles_markdown(&config);
        assert!(markdown.contains("模型: gemini-pro-orchestrator-neon · 无推理档位"));
        assert!(markdown.contains("Orchestrator 模型内部自带的并行子代理不受影响"));
    }

    #[test]
    fn markdown_indents_every_role_continuation_like_foreman() {
        let mut config = DispatchRolesConfig::default();
        config.agents[0].role = "阅读源码\r\n验证行为\n\n记录结果".into();

        let markdown = format_roles_markdown(&config);
        let (codex, _) = markdown
            .strip_prefix("## 用户自定义 Agent 生态位与默认配置（以本节为准）\n\n")
            .unwrap()
            .split_once("\n- **DSH**")
            .unwrap();
        assert_eq!(
            codex,
            "- **Codex**（默认兜底主力 | 模型: sol · 推理: 高）：阅读源码\n  验证行为\n  \n  记录结果"
        );
    }

    #[test]
    fn named_only_sender_can_delegate_to_the_only_auto_peer() {
        let mut config = DispatchRolesConfig::default();
        config.default_agent = "dsh".into();
        config.agents[0].mode = DispatchRoleMode::Explicit;
        config.agents[2].enabled = false;
        assert_eq!(
            config
                .sub_dispatch_targets()
                .map(|agent| agent.id.as_str())
                .collect::<Vec<_>>(),
            ["dsh"]
        );
        assert!(format_roles_markdown(&config).contains("协作者（DSH）职责范围内的子任务"));
        config.agents[0].enabled = false;
        assert!(config.sub_dispatch_targets().next().is_none());
        assert!(format_roles_markdown(&config).contains("当前无可转派协作者"));
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
