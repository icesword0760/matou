//! Prepare a Claude Code run from the frozen runtime's V1 settings.
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    claude_role_budget::{
        CLAUDE_ROLES as ROLES, ClaudeBudgetCompaction, ClaudeBudgetTarget, ClaudeRoleBudget,
        MAXIMUM_CONTEXT, claude_role_budget,
    },
    provider_config::custom_context_window,
    pty::{CALLER_SESSION_MARKERS, LaunchProfile, ShellLaunch, get_var, remove_var, set_var},
    sessions::{ClaudeRoleLaunchPreferences, read_claude_role_launch_preferences},
    startup,
};

const OFFICIAL_ID: &str = "anthropic-official";
const OFFICIAL_ENDPOINT: &str = "https://api.anthropic.com";
const CLI_DEFAULT_MODEL: &str = "CLI 默认";
const ZONEINFO: &str = "/usr/share/zoneinfo";
const COMMON_TIME_ZONES: [&str; 8] = [
    "UTC",
    "Asia/Shanghai",
    "Asia/Tokyo",
    "Asia/Singapore",
    "America/Los_Angeles",
    "America/New_York",
    "Europe/London",
    "Europe/Paris",
];

pub fn valid_claude_time_zone(value: &str) -> bool {
    if matches!(value, "auto" | "system") {
        return true;
    }
    let mut parts = value.split('/');
    if value.len() > 100
        || !parts.next().is_some_and(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        })
        || !parts.all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-'))
        })
        || value.starts_with("posix/")
        || value.starts_with("right/")
        || matches!(value, "localtime" | "posixrules")
    {
        return false;
    }
    if COMMON_TIME_ZONES.contains(&value) {
        return true;
    }
    use io::Read;
    let mut magic = [0; 4];
    fs::File::open(Path::new(ZONEINFO).join(value))
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && magic == *b"TZif"
}

pub fn available_time_zones() -> Vec<String> {
    let installed = fs::read_to_string(Path::new(ZONEINFO).join("zone.tab")).unwrap_or_default();
    let mut canonical: Vec<_> = installed
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split('\t').nth(2))
        .filter(|name| !COMMON_TIME_ZONES.contains(name))
        .collect();
    canonical.sort_unstable();
    canonical.dedup();
    COMMON_TIME_ZONES
        .into_iter()
        .chain(canonical)
        .map(str::to_owned)
        .collect()
}

pub fn system_time_zone() -> String {
    std::env::var("TZ")
        .ok()
        .filter(|zone| !matches!(zone.as_str(), "auto" | "system") && valid_claude_time_zone(zone))
        .or_else(|| {
            let zone = fs::canonicalize("/etc/localtime").ok()?;
            let name = zone.to_str()?.rsplit_once("/zoneinfo/")?.1;
            valid_claude_time_zone(name).then(|| name.to_owned())
        })
        .or_else(|| {
            let zone = fs::read_to_string("/etc/timezone").ok()?;
            let name = zone.trim();
            valid_claude_time_zone(name).then(|| name.to_owned())
        })
        .unwrap_or_else(|| "UTC".into())
}

pub fn automatic_time_zone(system: &str, exit_country: Option<&str>) -> Option<&'static str> {
    matches!(
        system,
        "Asia/Shanghai"
            | "Asia/Chongqing"
            | "Asia/Chungking"
            | "Asia/Harbin"
            | "Asia/Urumqi"
            | "Asia/Kashgar"
            | "PRC"
            | "Asia/Hong_Kong"
            | "Hongkong"
            | "Asia/Macau"
            | "Asia/Macao"
    )
    .then(
        || match exit_country.map(str::to_ascii_uppercase).as_deref() {
            Some("TW") => "Asia/Taipei",
            Some("MY") => "Asia/Kuala_Lumpur",
            Some("PH") => "Asia/Manila",
            _ => "Asia/Singapore",
        },
    )
}

pub fn cli_time_zone<'a>(
    profile: &str,
    setting: &'a str,
    system: &str,
    exit_country: Option<&str>,
    official: bool,
) -> Option<&'a str> {
    match (profile, setting) {
        ("claude-code", "system") => None,
        ("claude-code", "auto") | ("codex" | "antigravity", _) if official => {
            automatic_time_zone(system, exit_country)
        }
        ("claude-code", value) if value != "auto" => Some(value),
        _ => None,
    }
}

pub fn project_cli_time_zone(
    launch: &mut ShellLaunch,
    setting: &str,
    system: &str,
    official: bool,
) {
    if let Some(zone) = cli_time_zone(launch.profile.kind(), setting, system, None, official) {
        set_var(&mut launch.env, "TZ".into(), zone.into());
    }
}

pub fn official_time_zone_endpoint(endpoint: Option<&OsStr>, official_host: &str) -> bool {
    endpoint.and_then(OsStr::to_str).is_none_or(|endpoint| {
        let endpoint = endpoint.trim();
        endpoint.is_empty()
            || url::Url::parse(endpoint).is_ok_and(|url| url.host_str() == Some(official_host))
    })
}

pub const EFFORTS: [&str; 6] = ["follow", "low", "medium", "high", "xhigh", "max"];
pub const CLAUDE_PERMISSION_MODES: [&str; 5] = [
    "default",
    "auto",
    "acceptEdits",
    "plan",
    "bypassPermissions",
];

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Provider {
    id: String,
    endpoint: String,
    model: String,
    api_key: Option<String>,
    kind: Option<String>,
    subscription: Option<Value>,
    reasoning_effort: Option<String>,
    #[serde(default)]
    model_mapping: BTreeMap<String, String>,
    #[serde(default)]
    effort_mapping: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeRoleAssignment {
    pub provider_id: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

pub type ClaudeRoleAssignments = BTreeMap<String, ClaudeRoleAssignment>;

fn assignment_field(value: &Value, name: &str, maximum: usize) -> Result<String> {
    let value = value
        .as_str()
        .with_context(|| format!("{name} must be a string"))?;
    // Match JavaScript trim and string.length in the frozen validator.
    let trimmed =
        value.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}');
    ensure!(!trimmed.is_empty(), "{name} is empty");
    ensure!(
        value.encode_utf16().count() <= maximum,
        "{name} exceeds {maximum} characters"
    );
    ensure!(
        !value.chars().any(|c| c.is_ascii_control()),
        "{name} contains a control character"
    );
    Ok(trimmed.to_owned())
}

pub(crate) fn parse_assignment(value: &Value) -> Result<ClaudeRoleAssignment> {
    let target = value
        .as_object()
        .context("Assignment target must be an object")?;
    let provider_id = assignment_field(&value["providerId"], "providerId", 256)?;
    let model = assignment_field(&value["model"], "model", 240)?;
    let effort = target
        .get("effort")
        .map(|effort| {
            effort
                .as_str()
                .filter(|level| EFFORTS.contains(level))
                .map(str::to_owned)
                .context("Invalid reasoning effort")
        })
        .transpose()?;
    Ok(ClaudeRoleAssignment {
        provider_id,
        model,
        effort,
    })
}

pub(crate) fn parse_claude_web_search(value: &Value) -> Result<ClaudeRoleAssignment> {
    let target = parse_assignment(value)?;
    ensure!(
        [&target.provider_id, &target.model].into_iter().all(|id| {
            !id.chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '\u{feff}')
        }),
        "网页搜索供应商和模型不能包含空白或控制字符"
    );
    Ok(target)
}

pub(crate) fn parse_claude_compaction(value: &Value) -> Result<Value> {
    let fields = value
        .as_object()
        .context("Invalid compaction preferences")?;
    let mode = value["mode"]
        .as_str()
        .filter(|mode| matches!(*mode, "follow" | "custom"))
        .context("Invalid compaction mode")?;
    let effort = fields
        .get("effort")
        .map(|effort| {
            effort
                .as_str()
                .filter(|effort| EFFORTS.contains(effort))
                .context("Invalid compaction effort")
        })
        .transpose()?
        .unwrap_or("follow");
    let mut compaction = json!({"mode": mode, "effort": effort});
    for (field, maximum) in [("providerId", 256), ("model", 240)] {
        if mode == "custom" || fields.contains_key(field) {
            let id = assignment_field(&value[field], field, maximum)?;
            ensure!(
                !id.chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '\u{feff}'),
                "Invalid compaction {field}"
            );
            compaction[field] = json!(id);
        }
    }
    Ok(compaction)
}

pub fn parse_claude_role_assignments(value: &Value) -> Result<ClaudeRoleAssignments> {
    let mut assignments = ClaudeRoleAssignments::new();
    for (role, value) in value
        .as_object()
        .context("Role assignments must be an object")?
    {
        ensure!(ROLES.contains(&role.as_str()), "Unknown Claude model role");
        assignments.insert(
            role.clone(),
            parse_assignment(value).with_context(|| format!("{role} assignment"))?,
        );
    }
    Ok(assignments)
}

pub fn validate_claude_role_assignments(
    data_root: &Path,
    value: &Value,
) -> Result<ClaudeRoleAssignments> {
    let assignments = parse_claude_role_assignments(value)?;
    if assignments.is_empty() {
        return Ok(assignments);
    }

    let config = read_provider_config(data_root)?;
    for (role, assignment) in &assignments {
        assignment_provider(&config, assignment).with_context(|| format!("{role} assignment"))?;
    }
    Ok(assignments)
}

pub fn validate_claude_web_search(
    data_root: &Path,
    value: &Value,
    assignments: &ClaudeRoleAssignments,
    load_native_catalog: &dyn Fn() -> Vec<Value>,
) -> Result<ClaudeRoleAssignment> {
    let assignment = parse_claude_web_search(value).context("模型已不可用")?;
    let config = read_provider_config(data_root)?;
    let native_catalog = std::cell::OnceCell::new();
    let catalog = || native_catalog.get_or_init(load_native_catalog).as_slice();
    let (_, metadata) = exact_api_provider(&config, &assignment, &catalog)?;
    validate_assignment_effort(metadata, &assignment).context("当前模型不支持所选推理强度")?;
    if !assignments.is_empty() {
        let source = read_provider(data_root, &config, &Value::Null)?;
        snapshot_api_roles(&config, &source, assignments, &catalog)?;
    }
    Ok(assignment)
}

pub fn validate_claude_compaction(
    data_root: &Path,
    value: &Value,
    load_native_catalog: &dyn Fn() -> Vec<Value>,
) -> Result<Value> {
    let compaction = parse_claude_compaction(value)?;
    if compaction["mode"] == "custom" {
        let config = read_provider_config(data_root)?;
        let native_catalog = std::cell::OnceCell::new();
        let catalog = || native_catalog.get_or_init(load_native_catalog).as_slice();
        let assignment = parse_assignment(&compaction)?;
        let (provider, metadata) = exact_api_provider(&config, &assignment, &catalog)?;
        ensure!(
            gateway_connection(&provider).is_some(),
            "Compaction provider endpoint is unavailable"
        );
        validate_assignment_effort(metadata, &assignment).context("当前模型不支持所选推理强度")?;
    }
    Ok(compaction)
}

pub struct ClaudeLaunch {
    pub launch: ShellLaunch,
    pub provider_config_id: String,
    pub model: String,
    pub permission_mode: String,
    pub assigned_role: Option<&'static str>,
    pub effort: Option<String>,
    pub clear_assignment_environment: bool,
    pub gateway: Option<ClaudeGatewayProjection>,
}

impl ClaudeLaunch {
    pub fn project_time_zone(&mut self, setting: &str, system: &str) {
        let official = official_time_zone_endpoint(
            get_var(&self.launch.env, "ANTHROPIC_BASE_URL"),
            "api.anthropic.com",
        ) || self.gateway.as_ref().is_some_and(|gateway| {
            gateway
                .roles
                .iter()
                .flatten()
                .chain(gateway.web_search.iter())
                .chain(
                    gateway
                        .compaction
                        .iter()
                        .filter_map(|compaction| match compaction {
                            ClaudeGatewayCompaction::Snapshot { target, .. } => Some(target),
                            ClaudeGatewayCompaction::OnFirstRequest(_) => None,
                        }),
                )
                .any(|target| target.connection.endpoint.host_str() == Some("api.anthropic.com"))
        });
        project_cli_time_zone(&mut self.launch, setting, system, official);
    }
}

/// Private launch material. The runtime owns it for at most one CLI run.
/// Never serialize or format it as part of a launch diagnostic.
pub struct ClaudeGatewayProjection {
    pub source: Option<ClaudeGatewayConnection>,
    pub roles: [Option<ClaudeGatewayRole>; 3],
    pub web_search: Option<ClaudeGatewayRole>,
    pub compaction: Option<ClaudeGatewayCompaction>,
    pub unknown_compaction_context: bool,
    source_provider_id: String,
    source_models: [Option<String>; 3],
    source_efforts: [Option<String>; 3],
    source_effort: Option<String>,
    has_role_assignments: bool,
    startup_role: Option<&'static str>,
}

pub enum ClaudeGatewayCompaction {
    Snapshot {
        target: ClaudeGatewayRole,
        inherit_role_effort: bool,
    },
    OnFirstRequest(ClaudeLazyCompaction),
}

#[derive(Clone)]
pub struct ClaudeLazyCompaction {
    data_root: PathBuf,
    loopback_only: bool,
}

impl ClaudeLazyCompaction {
    pub fn resolve(
        &self,
        load_native_catalog: &dyn Fn() -> Vec<Value>,
    ) -> Result<ClaudeGatewayRole> {
        let preferences = {
            let db = startup::open_read_only(&self.data_root.join("matou.sqlite"))?;
            read_claude_role_launch_preferences(&db)
        };
        ensure!(
            preferences.compaction["mode"] == "custom",
            "Custom compaction target is unavailable"
        );
        let config = read_provider_config(&self.data_root)?;
        let native_catalog = std::cell::OnceCell::new();
        let catalog = || native_catalog.get_or_init(load_native_catalog).as_slice();
        let mut target = compaction_target(&config, &preferences.compaction, &catalog)?;
        ensure!(
            !self.loopback_only || loopback_connection(&target.connection),
            "Custom compaction target is unavailable"
        );
        // The old API route has no directory output cap. A missing explicit
        // effort follows the working supplier, not the compaction supplier.
        if preferences.compaction["effort"] == "follow" {
            target.effort = None;
        }
        target.model =
            if target.connection.endpoint.origin().ascii_serialization() == OFFICIAL_ENDPOINT {
                catalog()
                    .iter()
                    .find(|model| model["id"].as_str() == Some(&target.catalog_model))
                    .and_then(|model| model["resolvedModel"].as_str())
                    .unwrap_or(&target.catalog_model)
                    .to_owned()
            } else {
                target.catalog_model.clone()
            };
        target.max_output_tokens = None;
        Ok(target)
    }
}

pub struct ClaudeGatewayConnection {
    pub endpoint: url::Url,
    pub api_key: String,
}

pub struct ClaudeGatewayRole {
    pub connection: ClaudeGatewayConnection,
    pub model: String,
    catalog_model: String,
    provider_id: String,
    pub provider_name: String,
    pub effort: Option<String>,
    pub supported_efforts: Option<Vec<String>>,
    pub max_output_tokens: Option<u64>,
    context_window: Option<u64>,
    max_context_window: Option<u64>,
}

impl ClaudeGatewayRole {
    fn budget_target(&self) -> ClaudeBudgetTarget<'_> {
        ClaudeBudgetTarget {
            provider_id: &self.provider_id,
            provider_name: &self.provider_name,
            context_window: self.context_window,
            max_context_window: self.max_context_window,
        }
    }
}

impl ClaudeGatewayProjection {
    pub fn supports_environment(&self, env: &BTreeMap<OsString, OsString>) -> bool {
        // Preserve the CLI's existing outbound route when an external target
        // might require a proxy. The gateway currently handles direct HTTP(S).
        !gateway_loopback_only(env) || self.connections().all(loopback_connection)
    }

    pub fn connections(&self) -> impl Iterator<Item = &ClaudeGatewayConnection> {
        self.source
            .iter()
            .chain(self.roles.iter().flatten().map(|role| &role.connection))
            .chain(self.web_search.iter().map(|target| &target.connection))
            .chain(
                self.compaction
                    .iter()
                    .filter_map(|compaction| match compaction {
                        ClaudeGatewayCompaction::Snapshot { target, .. } => {
                            Some(&target.connection)
                        }
                        ClaudeGatewayCompaction::OnFirstRequest(_) => None,
                    }),
            )
    }

    pub fn request_role(&self, model: &str) -> Option<usize> {
        if !self.has_role_assignments {
            return None;
        }
        let model = without_context_suffix(model);
        if let Some(alias) = startup_role_alias(model) {
            return ROLES.iter().position(|role| *role == alias);
        }
        self.source_models.iter().position(|candidate| {
            candidate
                .as_deref()
                .is_some_and(|id| without_context_suffix(id) == model)
        })
    }

    pub fn literal_effort(&self, model: &str) -> Option<&str> {
        api_role_index(model)
            .or_else(|| {
                role_with_effort(model, |role| {
                    let index = ROLES.iter().position(|candidate| *candidate == role)?;
                    self.source_models[index].as_deref()
                })
            })
            .and_then(|index| self.source_efforts[index].as_deref())
            .or(self.source_effort.as_deref())
    }

    pub fn literal_model_override(&self, model: &str) -> Option<&str> {
        if self.has_role_assignments {
            return None;
        }
        self.source_models[api_role_index(model)?].as_deref()
    }

    pub fn project_environment(&self, launch: &mut ShellLaunch, base_url: &str) -> Value {
        // The URL prefix is the local capability. The CLI only needs a
        // placeholder API key to select its API transport.
        let mut projected = BTreeMap::from([
            ("ANTHROPIC_BASE_URL".to_owned(), base_url.to_owned()),
            (
                "ANTHROPIC_API_KEY".into(),
                "matou-local-role-gateway".into(),
            ),
            ("ANTHROPIC_AUTH_TOKEN".into(), String::new()),
            ("CLAUDE_CODE_OAUTH_TOKEN".into(), String::new()),
            ("CLAUDE_CODE_GATEWAY_HINT_HEADERS".into(), "1".into()),
        ]);
        let no_proxy = get_var(&launch.env, "no_proxy")
            .or_else(|| get_var(&launch.env, "NO_PROXY"))
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let mut exclusions: Vec<_> = no_proxy
            .split(',')
            .filter(|part| !part.is_empty())
            .collect();
        for local in ["127.0.0.1", "localhost"] {
            if !exclusions.iter().any(|part| part.trim() == local) {
                exclusions.push(local);
            }
        }
        let exclusions = exclusions.join(",");
        projected.insert("NO_PROXY".into(), exclusions.clone());
        projected.insert("no_proxy".into(), exclusions);
        if self.has_role_assignments {
            clear_role_model_environment(&mut launch.env);
        }
        for (alias, index) in [("OPUS", 0), ("SONNET", 1), ("HAIKU", 2), ("FABLE", 0)] {
            let (prefix, name) = if self.has_role_assignments {
                ("matou-role", self.catalog_model(index))
            } else if alias != "FABLE"
                && let Some(model) = self.source_models[index]
                    .as_deref()
                    .filter(|model| !model.is_empty())
            {
                ("matou-api", model)
            } else {
                continue;
            };
            let key = format!("ANTHROPIC_DEFAULT_{alias}_MODEL");
            projected.insert(format!("{key}_NAME"), name.into());
            projected.insert(key, format!("{prefix}/{}", ROLES[index]));
        }
        for (name, value) in &projected {
            set_var(&mut launch.env, name.into(), value.into());
        }
        // A real model ID in --model can be ambiguous when two providers use it.
        // Metadata continues to record the actual selected model for restoration.
        if let Some(role) = self.startup_role {
            let alias = format!("matou-role/{role}");
            if let [flag, model, ..] = launch.argv.as_mut_slice()
                && flag == "--model"
            {
                *model = alias;
            } else {
                launch.argv.splice(..0, ["--model".into(), alias]);
            }
        }
        serde_json::json!(projected)
    }

    pub fn model_picker(&self) -> Option<Value> {
        if !self.has_role_assignments {
            return None;
        }
        let options: [Value; ROLES.len()] = std::array::from_fn(|index| {
            let role = ROLES[index];
            let (first, rest) = role.split_at(1);
            let model = self.catalog_model(index);
            let provider_id = self.roles[index]
                .as_ref()
                .map_or(self.source_provider_id.as_str(), |target| {
                    &target.provider_id
                });
            serde_json::json!({
                "model": role,
                "label": format!("{}{rest} · {model}", first.to_ascii_uppercase()),
                "description": provider_id,
            })
        });
        Some(serde_json::json!({
            "replaceBuiltInOptions": true,
            "options": options,
        }))
    }

    fn catalog_model(&self, index: usize) -> &str {
        self.roles[index]
            .as_ref()
            .map_or(ROLES[index], |target| &target.catalog_model)
    }
}

fn gateway_loopback_only(env: &BTreeMap<OsString, OsString>) -> bool {
    let bypass_all = get_var(env, "no_proxy")
        .or_else(|| get_var(env, "NO_PROXY"))
        .is_some_and(|value| {
            value
                .to_string_lossy()
                .split(',')
                .any(|part| part.trim() == "*")
        });
    !bypass_all
        && [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ]
        .into_iter()
        .any(|name| get_var(env, name).is_some_and(|value| !value.is_empty()))
}

fn loopback_connection(connection: &ClaudeGatewayConnection) -> bool {
    matches!(
        connection.endpoint.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]")
    )
}

impl ClaudeLaunch {
    pub fn capture_restart_gateway(
        &mut self,
        preferences: &ClaudeRoleLaunchPreferences,
        load_native_catalog: &dyn Fn() -> Vec<Value>,
    ) -> Result<()> {
        let data_root = &self.launch.data_root;
        let config = read_provider_config(data_root)?;
        // The bound launch determines the conversation and restored model.
        // The legacy compaction gateway only adds a route to that launch.
        let independent = has_independent_target(preferences);
        let role_snapshot = has_role_snapshot(preferences);
        let source_binding = if independent && !role_snapshot {
            json!({"providerConfigId": self.provider_config_id})
        } else {
            Value::Null
        };
        let source = read_provider(data_root, &config, &source_binding)?;
        let role = self
            .assigned_role
            .or_else(|| model_role(&source, &self.model));
        self.gateway = if independent {
            let (projection, context_window) = independent_gateway_projection(
                &config,
                &source,
                preferences,
                role,
                data_root,
                &self.launch.env,
                load_native_catalog,
            )?;
            if role_snapshot {
                project_context_window(&mut self.launch.env, context_window);
            }
            Some(projection)
        } else {
            gateway_projection(&config, &source, Some(&preferences.assignments), role)
        };
        Ok(())
    }
}

/// Put the private run settings before all provider selection flags.
pub fn attach_claude_settings(launch: &mut ShellLaunch, path: &Path) -> Result<()> {
    let path = path
        .to_str()
        .context("provider settings path is not UTF-8")?;
    launch.argv.splice(..0, ["--settings".into(), path.into()]);
    let has_proxy = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .into_iter()
    .any(|name| get_var(&launch.env, name).is_some_and(|value| !value.is_empty()));
    if has_proxy {
        // Node and curl recognize different case variants. Keep existing
        // exclusions and make the run's private URL a direct request for both.
        let fallback = get_var(&launch.env, "NO_PROXY")
            .or_else(|| get_var(&launch.env, "no_proxy"))
            .map(OsStr::to_owned)
            .unwrap_or_default();
        for name in ["NO_PROXY", "no_proxy"] {
            let mut exclusions = get_var(&launch.env, name)
                .map(OsStr::to_owned)
                .unwrap_or_else(|| fallback.clone());
            if !exclusions
                .to_string_lossy()
                .split(',')
                .any(|entry| matches!(entry.trim(), "*" | "127.0.0.1"))
            {
                if !exclusions.is_empty() {
                    exclusions.push(",");
                }
                exclusions.push("127.0.0.1");
            }
            set_var(&mut launch.env, name.into(), exclusions);
        }
    }
    Ok(())
}

pub fn prepare_claude_launch(
    data_root: &Path,
    cwd: &Path,
    binding: &Value,
    resume_id: Option<&str>,
    environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<ClaudeLaunch> {
    prepare_claude(
        data_root,
        cwd,
        binding,
        resume_id.map(ClaudeTarget::Resume),
        environment,
        cols,
        rows,
    )
}

pub fn prepare_new_claude_launch(
    data_root: &Path,
    cwd: &Path,
    binding: &Value,
    assignments: Option<&ClaudeRoleAssignments>,
    environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<ClaudeLaunch> {
    prepare_claude(
        data_root,
        cwd,
        binding,
        Some(ClaudeTarget::New {
            assignments,
            budget_preferences: None,
        }),
        environment,
        cols,
        rows,
    )
}

pub fn prepare_new_claude_launch_with_budget(
    data_root: &Path,
    cwd: &Path,
    binding: &Value,
    preferences: (&ClaudeRoleLaunchPreferences, &dyn Fn() -> Vec<Value>),
    environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<ClaudeLaunch> {
    prepare_claude(
        data_root,
        cwd,
        binding,
        Some(ClaudeTarget::New {
            assignments: Some(&preferences.0.assignments),
            budget_preferences: Some(preferences),
        }),
        environment,
        cols,
        rows,
    )
}

pub fn prepare_claude_permission_restart(
    data_root: &Path,
    cwd: &Path,
    metadata: &Value,
    remembered: bool,
    environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<ClaudeLaunch> {
    prepare_claude(
        data_root,
        cwd,
        metadata,
        Some(ClaudeTarget::PermissionRestart { remembered }),
        environment,
        cols,
        rows,
    )
}

pub fn prepare_claude_fork(
    data_root: &Path,
    cwd: &Path,
    metadata: &Value,
    source_id: &str,
    environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<ClaudeLaunch> {
    ensure!(
        !source_id.trim().is_empty(),
        "Fork source conversation is missing"
    );
    prepare_claude(
        data_root,
        cwd,
        metadata,
        Some(ClaudeTarget::Fork(source_id)),
        environment,
        cols,
        rows,
    )
}

#[derive(Clone, Copy)]
enum ClaudeTarget<'a> {
    New {
        assignments: Option<&'a ClaudeRoleAssignments>,
        budget_preferences: Option<(&'a ClaudeRoleLaunchPreferences, &'a dyn Fn() -> Vec<Value>)>,
    },
    PermissionRestart {
        remembered: bool,
    },
    Resume(&'a str),
    Fork(&'a str),
}

fn prepare_claude(
    data_root: &Path,
    cwd: &Path,
    binding: &Value,
    target: Option<ClaudeTarget<'_>>,
    mut environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<ClaudeLaunch> {
    validate_cwd(cwd)?;
    let config = read_provider_config(data_root)?;
    let (provider_binding, assignments) = match target {
        Some(ClaudeTarget::New { assignments, .. }) => (&Value::Null, assignments),
        _ => (binding, None),
    };
    let remembered = matches!(
        target,
        Some(ClaudeTarget::PermissionRestart { remembered: true })
    );
    let mut provider = read_provider(data_root, &config, provider_binding)?;
    let bound_role = if matches!(
        target,
        Some(ClaudeTarget::Resume(_) | ClaudeTarget::PermissionRestart { .. })
    ) {
        ROLES
            .into_iter()
            .find(|role| provider_binding["assignedRole"].as_str() == Some(*role))
    } else {
        None
    };
    if bound_role.is_some() {
        // Preserve assignment credential projection for keyless official targets.
        provider.api_key = provider.api_key.filter(|key| !key.trim().is_empty());
    }
    // Subscription source preparation is deferred. A valid subscription source
    // still hits the service error before an API assignment can be applied.
    if provider.kind.as_deref() == Some("subscription")
        && let Some(subscription) = &provider.subscription
    {
        let opus = provider.model_mapping.get("opus").map(String::as_str);
        ensure!(
            opus.is_some()
                && subscription["modelMetadata"]
                    .as_array()
                    .is_some_and(|models| {
                        models.iter().any(|model| model["id"].as_str() == opus)
                    }),
            "Subscription Opus model metadata is missing"
        );
        bail!("Subscription launch service is unavailable");
    }

    let assigned = match assignments {
        Some(assignments) => resolve_startup_assignment(&config, &provider, assignments)?,
        None => None,
    };
    let (gateway, context_window) = match target {
        Some(ClaudeTarget::New {
            budget_preferences: Some((preferences, catalog)),
            ..
        }) if has_independent_target(preferences) => {
            let (projection, context_window) = independent_gateway_projection(
                &config,
                &provider,
                preferences,
                startup_role(&provider),
                data_root,
                &environment,
                catalog,
            )?;
            (Some(projection), context_window)
        }
        Some(ClaudeTarget::New {
            budget_preferences: Some((preferences, _)),
            ..
        }) if assigned.is_some() => (
            gateway_projection(&config, &provider, assignments, startup_role(&provider)),
            new_assigned_context_window(&config, &provider, preferences)?,
        ),
        None | Some(ClaudeTarget::New { .. }) => (
            gateway_projection(&config, &provider, assignments, startup_role(&provider)),
            None,
        ),
        _ => (None, None),
    };
    let model = match provider_binding["model"].as_str() {
        Some(bound)
            if bound.starts_with("matou-chatgpt/") || bound.starts_with("matou-antigravity/") =>
        {
            &provider.model
        }
        Some(bound) => match bound
            .strip_prefix("matou-api/")
            .filter(|role| ROLES.contains(role))
        {
            // The native process connects directly to the configured endpoint.
            // Translate a model ID previously used through the request gateway.
            Some(role) => provider.model_mapping.get(role).unwrap_or(&provider.model),
            None => bound,
        },
        None => &provider.model,
    };
    let (provider, model, effort, assigned_role) = match &assigned {
        Some((role, assignment, provider)) => {
            let effort = assignment
                .effort
                .as_deref()
                .or_else(|| provider.effort_mapping.get(*role).map(String::as_str))
                .or(provider.reasoning_effort.as_deref());
            (provider, assignment.model.as_str(), effort, Some(*role))
        }
        None => {
            let effort = if bound_role.is_some() || remembered {
                provider_binding["effort"].as_str()
            } else {
                reasoning_effort(&provider, model)
            };
            (&provider, model, effort, bound_role)
        }
    };
    let clear_assignment_environment = gateway
        .as_ref()
        .is_some_and(|projection| projection.web_search.is_some())
        || assigned_role.is_some()
        || assignments.is_some_and(|assignments| !assignments.is_empty())
        || (remembered && provider_binding["clearAssignmentEnvironment"] == true);
    if clear_assignment_environment {
        clear_assigned_environment(provider, &mut environment);
    }
    let effort = effort.filter(|effort| !effort.is_empty() && *effort != "follow");
    let argv = claude_argv(model, effort, target, binding["permissionMode"].as_str());
    for key in CALLER_SESSION_MARKERS {
        remove_var(&mut environment, OsStr::new(key));
    }
    let executable = resolve_claude_executable(cwd, &mut environment)?;
    project_environment(provider, &mut environment);
    if let Some(role) = assigned_role {
        // Keep a complete direct launch for targets outside the gateway scope.
        project_assigned_role(provider, role, model, &mut environment);
    }
    let context_window = context_window.or_else(|| {
        provider_entry(&config, &provider.id)
            .and_then(|entry| custom_context_window(&entry["contextWindow"]))
    });
    project_context_window(&mut environment, context_window);
    #[cfg(unix)]
    set_var(&mut environment, "PWD".into(), cwd.as_os_str().into());

    let permission_mode = binding["permissionMode"]
        .as_str()
        .filter(|mode| CLAUDE_PERMISSION_MODES.contains(mode))
        .unwrap_or("default");
    Ok(ClaudeLaunch {
        launch: ShellLaunch {
            profile: LaunchProfile::ClaudeCode,
            executable,
            argv,
            cwd: cwd.to_owned(),
            data_root: data_root.to_owned(),
            inherited_env: BTreeMap::new(),
            env: environment,
            cols,
            rows,
        },
        provider_config_id: provider.id.clone(),
        model: model.trim().to_owned(),
        permission_mode: permission_mode.to_owned(),
        assigned_role,
        effort: effort.map(str::to_owned),
        clear_assignment_environment,
        gateway,
    })
}

fn gateway_projection(
    config: &Value,
    source: &Provider,
    assignments: Option<&ClaudeRoleAssignments>,
    startup_role: Option<&'static str>,
) -> Option<ClaudeGatewayProjection> {
    let mut projection = api_projection(source);
    let Some(assignments) = assignments.filter(|assignments| !assignments.is_empty()) else {
        return (projection.source.is_some()
            && projection
                .source_efforts
                .iter()
                .chain([&projection.source_effort])
                .any(|effort| {
                    effort
                        .as_deref()
                        .is_some_and(|level| EFFORTS.contains(&level) && level != "follow")
                }))
        .then_some(projection);
    };
    // A keyless source can only be replaced when all three selectors have API
    // targets. Otherwise preserve the existing launch for this mixed setup.
    if projection.source.is_none()
        && (startup_role.is_none() || ROLES.iter().any(|role| !assignments.contains_key(*role)))
    {
        return None;
    }
    let providers = provider_entries(config).unwrap_or_default();
    let target = |provider: &Provider, role: &str, model: &str, effort: Option<&str>| {
        let entry = providers
            .iter()
            .find(|entry| entry["id"].as_str() == Some(&provider.id));
        let metadata = entry
            .and_then(|entry| entry["modelMetadata"].as_array())
            .and_then(|models| {
                models
                    .iter()
                    .find(|item| item["id"].as_str() == Some(model.trim()))
            });
        gateway_target(
            provider,
            provider_name(entry, &provider.id),
            model,
            metadata,
            Some(role),
            effort,
        )
    };
    for (index, role) in ROLES.into_iter().enumerate() {
        projection.roles[index] = if let Some(assignment) = assignments.get(role) {
            let provider = assignment_provider(config, assignment).ok()?;
            Some(target(
                &provider,
                role,
                &assignment.model,
                assignment.effort.as_deref(),
            )?)
        } else {
            source
                .model_mapping
                .get(role)
                .and_then(|model| target(source, role, model, None))
        };
    }
    if let Some(index) = ROLES.iter().position(|role| Some(*role) == startup_role)
        && projection.roles[index].is_none()
    {
        return None;
    }
    projection.has_role_assignments = true;
    projection.startup_role = startup_role;
    Some(projection)
}

fn api_projection(source: &Provider) -> ClaudeGatewayProjection {
    ClaudeGatewayProjection {
        source: gateway_connection(source),
        roles: [None, None, None],
        web_search: None,
        compaction: None,
        unknown_compaction_context: false,
        source_provider_id: source.id.clone(),
        source_models: ROLES.map(|role| source.model_mapping.get(role).cloned()),
        source_efforts: ROLES.map(|role| source.effort_mapping.get(role).cloned()),
        source_effort: source.reasoning_effort.clone(),
        has_role_assignments: false,
        startup_role: None,
    }
}

fn gateway_target(
    provider: &Provider,
    provider_name: &str,
    model: &str,
    metadata: Option<&Value>,
    role: Option<&str>,
    effort: Option<&str>,
) -> Option<ClaudeGatewayRole> {
    let connection = gateway_connection(provider)?;
    let catalog_model = model.trim();
    if catalog_model.is_empty() {
        return None;
    }
    let model = metadata
        .and_then(|item| item["resolvedModel"].as_str())
        .filter(|model| !model.is_empty())
        .unwrap_or(catalog_model)
        .to_owned();
    let effort = effort
        .or_else(|| role.and_then(|role| provider.effort_mapping.get(role).map(String::as_str)))
        .or(provider.reasoning_effort.as_deref())
        .filter(|level| EFFORTS.contains(level) && *level != "follow")
        .map(str::to_owned);
    Some(ClaudeGatewayRole {
        connection,
        model,
        catalog_model: catalog_model.into(),
        provider_id: provider.id.clone(),
        provider_name: provider_name.into(),
        effort,
        supported_efforts: metadata
            .and_then(|item| item["supportedEfforts"].as_array())
            .map(|levels| {
                levels
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            }),
        max_output_tokens: metadata
            .and_then(|item| item["maxOutputTokens"].as_u64())
            .filter(|tokens| *tokens > 0),
        context_window: context_tokens(metadata, "contextWindow"),
        max_context_window: context_tokens(metadata, "maxContextWindow"),
    })
}

fn provider_name<'a>(entry: Option<&'a Value>, id: &'a str) -> &'a str {
    entry
        .and_then(|entry| entry["name"].as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(id)
}

fn has_role_snapshot(preferences: &ClaudeRoleLaunchPreferences) -> bool {
    !preferences.assignments.is_empty() || preferences.web_search.is_some()
}

fn has_independent_target(preferences: &ClaudeRoleLaunchPreferences) -> bool {
    preferences.web_search.is_some() || preferences.compaction["mode"] == "custom"
}

fn independent_gateway_projection(
    config: &Value,
    source: &Provider,
    preferences: &ClaudeRoleLaunchPreferences,
    startup_role: Option<&'static str>,
    data_root: &Path,
    environment: &BTreeMap<OsString, OsString>,
    load_native_catalog: &dyn Fn() -> Vec<Value>,
) -> Result<(ClaudeGatewayProjection, Option<u64>)> {
    let mut projection = api_projection(source);
    if !has_role_snapshot(preferences) {
        ensure!(
            projection.source.is_some() && projection.supports_environment(environment),
            "Original supplier source connection was not bound at startup; choose a configured role"
        );
        projection.compaction = Some(ClaudeGatewayCompaction::OnFirstRequest(
            ClaudeLazyCompaction {
                data_root: data_root.to_owned(),
                loopback_only: gateway_loopback_only(environment),
            },
        ));
        return Ok((projection, None));
    }

    let native_catalog = std::cell::OnceCell::new();
    let catalog = || native_catalog.get_or_init(load_native_catalog).as_slice();
    if let Some(assignment) = &preferences.web_search {
        let name = provider_name(
            provider_entry(config, &assignment.provider_id),
            &assignment.provider_id,
        );
        let unavailable = || format!("网页搜索 · {name}: model target is unavailable");
        let (provider, metadata) = exact_api_provider(config, assignment, &catalog)
            .map_err(|_| anyhow::anyhow!(unavailable()))?;
        projection.web_search = Some(
            gateway_target(
                &provider,
                name,
                &assignment.model,
                metadata,
                None,
                assignment.effort.as_deref(),
            )
            .with_context(unavailable)?,
        );
    }
    if preferences.compaction["mode"] == "custom" {
        let target = compaction_target(config, &preferences.compaction, &catalog)?;
        projection.compaction = Some(ClaudeGatewayCompaction::Snapshot {
            target,
            inherit_role_effort: preferences.compaction["effort"] == "follow",
        });
    }
    projection.roles = snapshot_api_roles(config, source, &preferences.assignments, &catalog)?;
    projection.has_role_assignments = true;
    projection.startup_role = startup_role;
    if gateway_loopback_only(environment) {
        if let Some(target) = &projection.web_search {
            ensure!(
                projection.source.iter().all(loopback_connection)
                    && projection
                        .roles
                        .iter()
                        .flatten()
                        .all(|role| loopback_connection(&role.connection))
                    && loopback_connection(&target.connection),
                "网页搜索 · {}: model target is unavailable",
                target.provider_name
            );
        }
        if let Some(ClaudeGatewayCompaction::Snapshot { target, .. }) = &projection.compaction {
            ensure!(
                projection.supports_environment(environment),
                "compaction · {}: model target is unavailable",
                target.provider_name
            );
        }
    }
    let compaction = match &projection.compaction {
        Some(ClaudeGatewayCompaction::Snapshot { target, .. }) => {
            ClaudeBudgetCompaction::Custom(target.budget_target())
        }
        _ => ClaudeBudgetCompaction::Follow,
    };
    let ClaudeRoleBudget {
        context_window,
        unknown_compaction_context,
        ..
    } = claude_role_budget(
        std::array::from_fn(|index| projection.roles[index].as_ref().unwrap().budget_target()),
        compaction,
        &context_windows(config, preferences),
    )?;
    projection.unknown_compaction_context = unknown_compaction_context;
    Ok((projection, context_window))
}

fn compaction_target<'a>(
    config: &'a Value,
    compaction: &Value,
    native_catalog: &impl Fn() -> &'a [Value],
) -> Result<ClaudeGatewayRole> {
    let mut assignment = parse_assignment(compaction)?;
    let name = provider_name(
        provider_entry(config, &assignment.provider_id),
        &assignment.provider_id,
    );
    let unavailable = || format!("compaction · {name}: model target is unavailable");
    let (provider, metadata) = exact_api_provider(config, &assignment, native_catalog)
        .map_err(|_| anyhow::anyhow!(unavailable()))?;
    if assignment.effort.as_deref() == Some("follow") {
        assignment.effort = None;
    }
    validate_assignment_effort(metadata, &assignment).context(format!(
        "compaction · {name}: model target or reasoning effort is unavailable"
    ))?;
    gateway_target(
        &provider,
        name,
        &assignment.model,
        metadata,
        None,
        assignment.effort.as_deref(),
    )
    .with_context(unavailable)
}

fn provider_entry<'a>(config: &'a Value, id: &str) -> Option<&'a Value> {
    provider_entries(config)?
        .iter()
        .find(|entry| entry["id"].as_str() == Some(id))
}

fn context_windows(
    config: &Value,
    preferences: &ClaudeRoleLaunchPreferences,
) -> Map<String, Value> {
    let mut windows = preferences.context_windows.clone();
    for provider in provider_entries(config).unwrap_or_default() {
        if let Some(id) = provider["id"].as_str()
            && let Some(tokens) = custom_context_window(&provider["contextWindow"])
        {
            windows.insert(id.to_owned(), json!(tokens));
        }
    }
    windows
}

fn uses_native_catalog(entry: &Value) -> bool {
    entry["builtIn"] == true
        && entry.get("modelMetadata").is_none()
        && entry["endpoint"].as_str().is_some_and(|endpoint| {
            endpoint.strip_suffix('/').unwrap_or(endpoint) == OFFICIAL_ENDPOINT
        })
}

fn exact_api_provider<'a>(
    config: &'a Value,
    assignment: &ClaudeRoleAssignment,
    native_catalog: &impl Fn() -> &'a [Value],
) -> Result<(Provider, Option<&'a Value>)> {
    if assignment.provider_id == OFFICIAL_ID && provider_entries(config).is_none() {
        // The built-in default exists without a provider file, with no Key.
        bail!("API Key 未配置");
    }
    let entry = provider_entry(config, &assignment.provider_id)
        .filter(|entry| entry["cli"].as_str().is_none_or(|cli| cli == "claude-code"))
        .context("供应商已不存在")?;
    ensure!(
        entry["kind"] != "subscription" && entry["subscription"].is_null(),
        "供应商已不存在"
    );
    ensure!(
        entry["apiKey"]
            .as_str()
            .is_some_and(|key| !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_graphic())),
        "API Key 未配置"
    );
    let provider: Provider =
        serde_json::from_value(entry.clone()).map_err(|_| anyhow::anyhow!("供应商已不存在"))?;
    let metadata = exact_model_metadata(entry, &assignment.model, native_catalog)?;
    Ok((provider, metadata))
}

fn exact_model_metadata<'a>(
    entry: &'a Value,
    model: &str,
    native_catalog: &impl Fn() -> &'a [Value],
) -> Result<Option<&'a Value>> {
    let models = if uses_native_catalog(entry) {
        native_catalog()
    } else if let Some(catalog) = entry.get("modelMetadata") {
        catalog.as_array().context("模型目录暂不可用")?
    } else {
        return Ok(None);
    };
    ensure!(!models.is_empty(), "模型目录暂不可用");
    let metadata = models
        .iter()
        .find(|item| item["id"].as_str() == Some(model))
        .context("模型已不可用")?;
    Ok(Some(metadata))
}

fn snapshot_api_roles<'a>(
    config: &'a Value,
    source: &Provider,
    assignments: &ClaudeRoleAssignments,
    native_catalog: &impl Fn() -> &'a [Value],
) -> Result<[Option<ClaudeGatewayRole>; 3]> {
    let mut targets = [None, None, None];
    for (index, role) in ROLES.into_iter().enumerate() {
        let assignment = assignments.get(role);
        let id = assignment.map_or(source.id.as_str(), |target| target.provider_id.as_str());
        let entry = provider_entry(config, id)
            .with_context(|| format!("{role} · {id}: model target is unavailable"))?;
        let name = provider_name(Some(entry), id);
        let provider;
        let (provider, model) = if let Some(assignment) = assignment {
            provider = serde_json::from_value::<Provider>(entry.clone())
                .map_err(|_| anyhow::anyhow!("{role} · {name}: model target is unavailable"))?;
            (&provider, assignment.model.as_str())
        } else {
            let model = source.model_mapping.get(role).map(String::as_str)
                .filter(|model| !model.is_empty())
                .or_else(|| {
                    let prefix = format!("claude-{role}-");
                    let models = native_catalog();
                    let native = models.iter().find(|item| item["id"] == role).or_else(|| {
                        models.iter().find(|item| item["id"].as_str().is_some_and(|id| id.starts_with(&prefix)))
                    })?;
                    if uses_native_catalog(entry) {
                        native["id"].as_str()
                    } else {
                        native["resolvedModel"].as_str().or_else(|| native["id"].as_str())
                    }
                })
                .with_context(|| format!("{role} · {name}: native role model is unavailable; configure this role's model"))?;
            (source, model)
        };
        let metadata = exact_model_metadata(entry, model, native_catalog)
            .with_context(|| format!("{role} · {name}"))?;
        if let Some(assignment) = assignment {
            validate_assignment_effort(metadata, assignment)
                .with_context(|| format!("{role} · {name}: 当前模型不支持所选推理强度"))?;
        }
        targets[index] = Some(
            gateway_target(
                provider,
                name,
                model,
                metadata,
                Some(role),
                assignment.and_then(|target| target.effort.as_deref()),
            )
            .with_context(|| format!("{role} · {name}: model target is unavailable"))?,
        );
    }
    Ok(targets)
}

fn gateway_connection(provider: &Provider) -> Option<ClaudeGatewayConnection> {
    let api_key = provider
        .api_key
        .as_deref()
        .filter(|key| !key.trim().is_empty() && key.bytes().all(|byte| byte.is_ascii_graphic()))?;
    let endpoint = url::Url::parse(&provider.endpoint).ok()?;
    if provider.kind.as_deref() == Some("subscription")
        || provider.subscription.is_some()
        || !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        return None;
    }
    Some(ClaudeGatewayConnection {
        endpoint,
        api_key: api_key.into(),
    })
}

pub fn validate_cwd(cwd: &Path) -> Result<()> {
    ensure!(
        !cwd.as_os_str().is_empty(),
        "Claude Code working directory is missing (stored cwd is empty)"
    );
    let metadata = fs::metadata(cwd)
        .with_context(|| format!("open Claude Code working directory {}", cwd.display()))?;
    ensure!(
        metadata.is_dir(),
        "Claude Code working directory {} is not a directory",
        cwd.display()
    );
    #[cfg(unix)]
    check_execute_access(cwd)
        .with_context(|| format!("access Claude Code working directory {}", cwd.display()))?;
    Ok(())
}

fn read_provider_config(data_root: &Path) -> Result<Value> {
    let path = data_root.join("provider-config.json");
    let config = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(|error| {
            anyhow::anyhow!(
                "read {}: invalid JSON at line {}, column {}",
                path.display(),
                error.line(),
                error.column()
            )
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    ensure!(
        !config.is_null(),
        "read {}: invalid provider configuration root",
        path.display()
    );
    Ok(config)
}

fn provider_entries(config: &Value) -> Option<&[Value]> {
    // Frozen V1 normalization seeds an empty list and repairs a stale active ID
    // to the first entry. A binding is explicit and must still resolve exactly.
    (config["version"].as_u64() == Some(1)
        && !config["providers"].is_null()
        && !config["activeProviderIds"].is_null())
    .then(|| config["providers"]["claude-code"].as_array())
    .flatten()
    .map(Vec::as_slice)
    .filter(|providers| !providers.is_empty())
}

fn official_provider() -> Provider {
    Provider {
        id: OFFICIAL_ID.into(),
        endpoint: OFFICIAL_ENDPOINT.into(),
        ..Provider::default()
    }
}

fn read_provider(data_root: &Path, config: &Value, binding: &Value) -> Result<Provider> {
    let bound_id = binding["providerConfigId"]
        .as_str()
        .filter(|id| !id.trim().is_empty());
    let missing = "会话绑定的供应商配置不存在";
    let Some(providers) = provider_entries(config) else {
        ensure!(bound_id.is_none_or(|id| id == OFFICIAL_ID), "{missing}");
        return Ok(official_provider());
    };
    let active_id = config["activeProviderIds"]["claude-code"].as_str();
    let find = |id| {
        providers
            .iter()
            .find(|provider| provider["id"].as_str() == Some(id))
    };
    let selected = match bound_id {
        Some(id) => find(id).context(missing)?,
        None => active_id.and_then(find).unwrap_or(&providers[0]),
    };
    // serde's error can include the offending value, including a credential.
    serde_json::from_value(selected.clone()).map_err(|_| {
        anyhow::anyhow!(
            "read {}: invalid Claude Code provider entry",
            data_root.join("provider-config.json").display()
        )
    })
}

fn assignment_provider(config: &Value, assignment: &ClaudeRoleAssignment) -> Result<Provider> {
    // Without a built-in model catalog, native skips the frozen runtime's
    // official target membership check when the provider has no stored catalog.
    let Some(providers) = provider_entries(config) else {
        ensure!(
            assignment.provider_id == OFFICIAL_ID,
            "Assignment provider is unavailable"
        );
        return Ok(official_provider());
    };
    let entry = providers
        .iter()
        .find(|row| row["id"].as_str() == Some(&assignment.provider_id))
        .context("Assignment provider is unavailable")?;
    let mut provider: Provider = serde_json::from_value(entry.clone())
        .map_err(|_| anyhow::anyhow!("Invalid assignment provider entry"))?;
    // Subscription targets are deliberately rejected until native can start
    // the subscription service, including when saving an assignment.
    ensure!(
        provider.kind.as_deref() != Some("subscription"),
        "Subscription launch service is unavailable"
    );
    provider.api_key = provider.api_key.filter(|key| !key.trim().is_empty());
    let catalog = entry.get("modelMetadata");
    let endpoint = provider
        .endpoint
        .strip_suffix('/')
        .unwrap_or(&provider.endpoint);
    let uses_native_catalog =
        entry["builtIn"] == true && catalog.is_none() && endpoint == OFFICIAL_ENDPOINT;
    ensure!(
        uses_native_catalog || provider.api_key.is_some(),
        "Assignment API key is unavailable"
    );
    if let Some(catalog) = catalog {
        validate_assignment_model(catalog, &provider, assignment)?;
    }
    Ok(provider)
}

fn validate_assignment_model(
    catalog: &Value,
    provider: &Provider,
    assignment: &ClaudeRoleAssignment,
) -> Result<()> {
    let models = catalog
        .as_array()
        .filter(|models| !models.is_empty())
        .with_context(|| {
            format!(
                "Assignment model catalog is unavailable for provider {}",
                provider.id
            )
        })?;
    let model = models
        .iter()
        .find(|model| model["id"].as_str() == Some(&assignment.model))
        .context("Assignment model is unavailable")?;
    validate_assignment_effort(Some(model), assignment)
}

fn validate_assignment_effort(
    metadata: Option<&Value>,
    assignment: &ClaudeRoleAssignment,
) -> Result<()> {
    if let Some(effort) = assignment
        .effort
        .as_deref()
        .filter(|effort| *effort != "follow")
        && let Some(levels) = metadata
            .and_then(|model| model.get("supportedEfforts"))
            .filter(|levels| !levels.is_null())
    {
        ensure!(
            levels.as_array().is_some_and(|levels| {
                levels.iter().any(|level| level.as_str() == Some(effort))
            }),
            "Unsupported reasoning effort for assignment model"
        );
    }
    Ok(())
}

fn startup_role(source: &Provider) -> Option<&'static str> {
    model_role(source, &source.model)
}

fn model_role(source: &Provider, model: &str) -> Option<&'static str> {
    let model = without_context_suffix(model);
    if matches!(model, "" | "default" | CLI_DEFAULT_MODEL) {
        // With the local defaults and no user-level model override, Claude Code
        // 2.1.328 reported claude-opus-5-5 in SessionStart without --model (2026-11).
        // Treat CLI-default models as opus. A user override can make this inaccurate
        // until the request gateway resolves the default as the frozen runtime does.
        return Some("opus");
    }
    if let Some(role) = startup_role_alias(model) {
        return Some(role);
    }
    ROLES.into_iter().find(|role| {
        source
            .model_mapping
            .get(*role)
            .is_some_and(|mapped| without_context_suffix(mapped) == model)
    })
}

fn startup_role_alias(model: &str) -> Option<&'static str> {
    if matches!(
        model,
        "matou-role/opus-fallback" | "claude-fable-5" | "claude-fable-5-2" | "claude-fable-5-5"
    ) {
        return Some("opus");
    }
    let role_model = model
        .strip_prefix("matou-role/")
        .or_else(|| model.strip_prefix("matou-api/"))
        .unwrap_or(model);
    match role_model {
        "opus" | "fable" => return Some("opus"),
        "sonnet" => return Some("sonnet"),
        "haiku" => return Some("haiku"),
        _ => {}
    }
    let subscription_model = model.strip_prefix("matou-antigravity/").unwrap_or(model);
    match model
        .strip_prefix("matou-chatgpt/")
        .unwrap_or(subscription_model)
    {
        "matou-opus" | "matou-opus-fallback" => return Some("opus"),
        "matou-sonnet" => return Some("sonnet"),
        "matou-haiku" => return Some("haiku"),
        _ => {}
    }
    match subscription_model {
        "claude-opus-5" | "claude-opus-5-5" => Some("opus"),
        "claude-sonnet-5" | "claude-sonnet-5-5" => Some("sonnet"),
        "claude-haiku-5-5" => Some("haiku"),
        _ => None,
    }
}

fn resolve_startup_assignment<'a>(
    config: &Value,
    source: &Provider,
    assignments: &'a ClaudeRoleAssignments,
) -> Result<Option<(&'static str, &'a ClaudeRoleAssignment, Provider)>> {
    let Some(role) = startup_role(source) else {
        return Ok(None);
    };
    let Some(assignment) = assignments.get(role) else {
        // Native has no built-in model catalog. Unassigned roles keep the source
        // launch without the frozen runtime's fallback target validation.
        return Ok(None);
    };
    let provider =
        assignment_provider(config, assignment).with_context(|| format!("{role} assignment"))?;
    Ok(Some((role, assignment, provider)))
}

fn new_assigned_context_window(
    config: &Value,
    source: &Provider,
    preferences: &ClaudeRoleLaunchPreferences,
) -> Result<Option<u64>> {
    let providers = provider_entries(config).unwrap_or_default();
    let roles = ROLES.map(|role| {
        if let Some(assignment) = preferences.assignments.get(role) {
            return budget_target(providers, &assignment.provider_id, Some(&assignment.model));
        }
        // Only a role mapping identifies a stored model for an unassigned tier.
        // Native has no live catalog fallback. The startup model belongs to no
        // other tier just because it was selected for this invocation.
        let model = source.model_mapping.get(role).map(String::as_str);
        budget_target(providers, &source.id, model)
    });
    Ok(claude_role_budget(
        roles,
        ClaudeBudgetCompaction::Follow,
        &context_windows(config, preferences),
    )?
    .context_window)
}

fn project_context_window(environment: &mut BTreeMap<OsString, OsString>, window: Option<u64>) {
    if let Some(window) = window {
        for (key, value) in [
            ("CLAUDE_CODE_MAX_CONTEXT_TOKENS", window.to_string()),
            ("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", "90".to_owned()),
            (
                "CLAUDE_CODE_DISABLE_1M_CONTEXT",
                u8::from(window < MAXIMUM_CONTEXT).to_string(),
            ),
        ] {
            set_var(environment, key.into(), value.into());
        }
    }
}

fn context_tokens(metadata: Option<&Value>, field: &str) -> Option<u64> {
    metadata
        .and_then(|metadata| metadata[field].as_f64())
        .filter(|count| *count > 0.0 && *count <= 9_007_199_254_740_991.0 && count.fract() == 0.0)
        .map(|count| count as u64)
}

fn budget_target<'a>(
    providers: &'a [Value],
    provider_id: &'a str,
    model: Option<&str>,
) -> ClaudeBudgetTarget<'a> {
    let provider = providers
        .iter()
        .find(|provider| provider["id"].as_str() == Some(provider_id));
    let metadata = provider
        .and_then(|provider| {
            provider["subscription"]["modelMetadata"]
                .as_array()
                .or_else(|| provider["modelMetadata"].as_array())
        })
        .and_then(|models| {
            let model = model.filter(|model| !model.is_empty())?;
            models
                .iter()
                .find(|item| item["id"].as_str() == Some(model))
        });
    ClaudeBudgetTarget {
        provider_id,
        provider_name: provider
            .and_then(|provider| provider["name"].as_str())
            .unwrap_or(provider_id),
        context_window: context_tokens(metadata, "contextWindow"),
        max_context_window: context_tokens(metadata, "maxContextWindow"),
    }
}

fn claude_argv(
    model: &str,
    effort: Option<&str>,
    target: Option<ClaudeTarget<'_>>,
    permission: Option<&str>,
) -> Vec<String> {
    let mut argv = Vec::new();
    if !model.trim().is_empty() {
        argv.extend(["--model".into(), model.trim().into()]);
    }
    if let Some(effort) = effort {
        argv.extend(["--effort".into(), effort.into()]);
    }
    if let Some(ClaudeTarget::Resume(id) | ClaudeTarget::Fork(id)) = target {
        argv.extend(["--resume".into(), id.into()]);
        if matches!(target, Some(ClaudeTarget::Fork(_))) {
            argv.push("--fork-session".into());
        }
    }
    match permission {
        Some("bypassPermissions") => argv.push("--dangerously-skip-permissions".into()),
        Some(mode @ ("auto" | "acceptEdits" | "plan")) => {
            argv.extend(["--permission-mode".into(), mode.into()]);
        }
        _ => {}
    }
    argv
}

fn without_context_suffix(model: &str) -> &str {
    model
        .split_at_checked(model.len().saturating_sub(4))
        .filter(|(_, suffix)| suffix.eq_ignore_ascii_case("[1m]"))
        .map_or(model, |(base, _)| base)
}

fn reasoning_effort<'a>(provider: &'a Provider, model: &str) -> Option<&'a str> {
    role_with_effort(model, |role| {
        provider.model_mapping.get(role).map(String::as_str)
    })
    .and_then(|index| provider.effort_mapping.get(ROLES[index]))
    .map(String::as_str)
    .or(provider.reasoning_effort.as_deref())
}

fn api_role_index(model: &str) -> Option<usize> {
    let role = model.strip_prefix("matou-api/")?;
    ROLES.iter().position(|candidate| *candidate == role)
}

fn role_with_effort<'a>(model: &str, mapping: impl Fn(&str) -> Option<&'a str>) -> Option<usize> {
    let model = if model.is_empty() { "default" } else { model };
    let mapped = ROLES.iter().position(|role| {
        mapping(role)
            .is_some_and(|mapped| without_context_suffix(mapped) == without_context_suffix(model))
    });
    let lower_model = model.to_ascii_lowercase();
    let inferred = || {
        ROLES.iter().position(|role| {
            ["matou-", "claude-"].into_iter().any(|prefix| {
                let pattern = format!("{prefix}{role}");
                lower_model.match_indices(&pattern).any(|(index, _)| {
                    let suffix = &lower_model[index + pattern.len()..];
                    suffix.is_empty() || suffix.starts_with(['-', '/', '['])
                })
            })
        })
    };
    mapped.or_else(inferred)
}

fn clear_role_model_environment(environment: &mut BTreeMap<OsString, OsString>) {
    environment.retain(|key, _| {
        #[cfg(windows)]
        let key = key.to_ascii_uppercase();
        let key = key.as_encoded_bytes();
        !(key.starts_with(b"ANTHROPIC_DEFAULT_")
            || matches!(key, b"ANTHROPIC_MODEL" | b"ANTHROPIC_SMALL_FAST_MODEL")
            || (key.starts_with(b"CLAUDE_CODE_")
                && key.ends_with(b"MODEL")
                && key
                    .windows(b"SUBAGENT".len())
                    .any(|part| part == b"SUBAGENT")))
    });
}

fn clear_assigned_environment(provider: &Provider, environment: &mut BTreeMap<OsString, OsString>) {
    clear_role_model_environment(environment);
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_DISABLE_1M_CONTEXT",
        "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
        "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
        "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
    ] {
        remove_var(environment, OsStr::new(key));
    }
    if provider.api_key.is_some() {
        remove_var(environment, OsStr::new("CLAUDE_CODE_OAUTH_TOKEN"));
    }
}

fn project_assigned_role(
    provider: &Provider,
    role: &str,
    model: &str,
    environment: &mut BTreeMap<OsString, OsString>,
) {
    let key = format!("ANTHROPIC_DEFAULT_{}_MODEL", role.to_ascii_uppercase());
    set_var(environment, format!("{key}_NAME").into(), model.into());
    set_var(environment, key.into(), model.into());

    let opus = if role == "opus" {
        Some(model)
    } else {
        provider
            .model_mapping
            .get("opus")
            .map(String::as_str)
            .filter(|model| !model.is_empty())
    };
    if let Some(opus) = opus {
        for key in [
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
        ] {
            set_var(environment, key.into(), opus.into());
        }
    }
}

fn project_environment(provider: &Provider, environment: &mut BTreeMap<OsString, OsString>) {
    if let Some(key) = provider.api_key.as_deref().filter(|key| !key.is_empty()) {
        set_var(environment, "ANTHROPIC_API_KEY".into(), key.into());
        set_var(environment, "ANTHROPIC_AUTH_TOKEN".into(), key.into());
    }
    let endpoint = provider
        .endpoint
        .strip_suffix('/')
        .unwrap_or(&provider.endpoint);
    if !endpoint.is_empty() {
        set_var(environment, "ANTHROPIC_BASE_URL".into(), endpoint.into());
    }
    for (role, model) in &provider.model_mapping {
        if !model.is_empty() {
            set_var(
                environment,
                format!("ANTHROPIC_DEFAULT_{}_MODEL", role.to_uppercase()).into(),
                model.into(),
            );
        }
    }
    remove_var(environment, OsStr::new("NO_COLOR"));
    for (key, value) in [
        ("TERM", "xterm-256color"),
        ("COLORTERM", "truecolor"),
        ("FORCE_COLOR", "1"),
    ] {
        set_var(environment, key.into(), value.into());
    }
    #[cfg(target_os = "macos")]
    if get_var(environment, "mimalloc_purge_holes_min_interval").is_none()
        && get_var(environment, "MIMALLOC_PURGE_HOLES_MIN_INTERVAL").is_none()
    {
        set_var(
            environment,
            "MIMALLOC_PURGE_HOLES_MIN_INTERVAL".into(),
            "1000".into(),
        );
    }
}

/// Resolve the Claude executable and recover its PATH using the regular launch rules.
pub fn resolve_claude_executable(
    cwd: &Path,
    environment: &mut BTreeMap<OsString, OsString>,
) -> Result<PathBuf> {
    let command = get_var(environment, "MATOU_CLAUDE_COMMAND")
        .filter(|command| !command.is_empty())
        .unwrap_or(OsStr::new("claude"))
        .to_owned();
    let command = Path::new(&command);
    let cwd = std::path::absolute(cwd).context("resolve Claude Code working directory")?;
    let resolved = search_command(command, &cwd, environment);
    #[cfg(unix)]
    let resolved = if resolved.is_err()
        && command.file_name() == Some(command.as_os_str())
        && get_var(environment, "MATOU_DISABLE_LOGIN_PATH_PROBE") != Some(OsStr::new("1"))
        && let Some(path) = login_shell_path(environment)
    {
        set_var(environment, "PATH".into(), path.into());
        search_command(command, &cwd, environment).or(resolved)
    } else {
        resolved
    };

    match resolved {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let command = command.display();
            bail!(
                "未检测到 Claude Code（命令：{command}）。请先安装并确保 `{command}` 在 PATH 中，再重试。"
            );
        }
        result => result.with_context(|| format!("start Claude Code {}", command.display())),
    }
}

fn search_command(
    command: &Path,
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> io::Result<PathBuf> {
    let mut failure = io::Error::from(io::ErrorKind::NotFound);
    let mut check = |path: PathBuf| {
        let result = fs::metadata(&path).and_then(|metadata| {
            if metadata.is_dir() {
                return Err(io::Error::from(io::ErrorKind::IsADirectory));
            }
            #[cfg(unix)]
            check_execute_access(&path)?;
            Ok(())
        });
        match result {
            Ok(()) => Some(path),
            Err(error) => {
                if failure.kind() == io::ErrorKind::NotFound {
                    failure = error;
                }
                None
            }
        }
    };
    #[cfg(windows)]
    let mut check = |path: PathBuf| {
        check(path.clone()).or_else(|| {
            if path.extension().is_some() {
                return None;
            }
            let extensions = get_var(environment, "PATHEXT").unwrap_or(OsStr::new(".EXE"));
            std::env::split_paths(extensions)
                .filter_map(|ext| {
                    ext.to_str()
                        .and_then(|ext| ext.strip_prefix('.'))
                        .map(str::to_owned)
                })
                .find_map(|ext| check(path.with_extension(ext)))
        })
    };

    let found = if command.file_name() != Some(command.as_os_str()) {
        check(cwd.join(command))
    } else {
        get_var(environment, "PATH").and_then(|path| {
            std::env::split_paths(path)
                .filter(|dir| !dir.as_os_str().is_empty())
                .find_map(|dir| check(cwd.join(dir).join(command)))
        })
    };
    found.ok_or(failure)
}

#[cfg(unix)]
fn check_execute_access(path: &Path) -> io::Result<()> {
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
    if unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn login_shell_path(environment: &BTreeMap<OsString, OsString>) -> Option<String> {
    use std::{
        io::Read,
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    const MARKER: &str = "__MATOU_LOGIN_PATH__=";
    const LIMIT: usize = 256 * 1024;
    let shell = get_var(environment, "SHELL").unwrap_or(OsStr::new("/bin/zsh"));
    let mut child = Command::new(shell)
        .args(["-ilc", "printf '__MATOU_LOGIN_PATH__=%s\\n' \"$PATH\""])
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take().expect("piped login-shell output");
    let fd = stdout.as_raw_fd();
    let mut data = Vec::new();
    let mut eof = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } == 0 {
        loop {
            if !eof {
                let remaining = (LIMIT + 1 - data.len()) as u64;
                match stdout.by_ref().take(remaining).read_to_end(&mut data) {
                    Ok(_) => eof = true,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(_) => break,
                }
            }
            if data.len() > LIMIT {
                break;
            }
            if eof && let Some(status) = child.try_wait().ok().flatten() {
                return status
                    .success()
                    .then(|| {
                        String::from_utf8_lossy(&data)
                            .lines()
                            .rev()
                            .find_map(|line| line.strip_prefix(MARKER))
                            .map(str::trim)
                            .filter(|path| !path.is_empty())
                            .map(str::to_owned)
                    })
                    .flatten();
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    // The child still owns this process group ID until waited on.
    unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    let _ = child.wait();
    None
}
