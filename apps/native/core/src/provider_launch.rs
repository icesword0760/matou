//! Prepare a Claude Code run from the frozen runtime's V1 settings.
#[cfg(unix)]
use std::sync::OnceLock;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    claude_role_budget::{
        CLAUDE_ROLE_LABELS, CLAUDE_ROLES as ROLES, ClaudeBudgetCompaction, ClaudeBudgetTarget,
        ClaudeRoleBudget, MAXIMUM_CONTEXT, claude_role_budget,
    },
    provider_config::custom_context_window,
    pty::{CALLER_SESSION_MARKERS, LaunchProfile, ShellLaunch, get_var, remove_var, set_var},
    sessions::{ClaudeRoleLaunchPreferences, current_interface_locale, read_locale_preference},
};

mod official_oauth;
use official_oauth::OfficialClaudeEnvironment;
pub use official_oauth::OfficialClaudeOAuth;

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
        .or_else(platform_time_zone)
        .unwrap_or_else(|| "UTC".into())
}

#[cfg(not(windows))]
fn platform_time_zone() -> Option<String> {
    (|| {
        let zone = fs::canonicalize("/etc/localtime").ok()?;
        let name = zone.to_str()?.rsplit_once("/zoneinfo/")?.1;
        valid_claude_time_zone(name).then(|| name.to_owned())
    })()
    .or_else(|| {
        let zone = fs::read_to_string("/etc/timezone").ok()?;
        let name = zone.trim();
        valid_claude_time_zone(name).then(|| name.to_owned())
    })
}

#[cfg(windows)]
fn platform_time_zone() -> Option<String> {
    #[repr(C)]
    struct DynamicTimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: [u16; 8],
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: [u16; 8],
        daylight_bias: i32,
        key_name: [u16; 128],
        dynamic_daylight_time_disabled: u8,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDynamicTimeZoneInformation(information: *mut DynamicTimeZoneInformation) -> u32;
    }

    let mut information = std::mem::MaybeUninit::<DynamicTimeZoneInformation>::zeroed();
    // SAFETY: The writable buffer matches DYNAMIC_TIME_ZONE_INFORMATION.
    if unsafe { GetDynamicTimeZoneInformation(information.as_mut_ptr()) } == u32::MAX {
        return None;
    }
    let information = unsafe { information.assume_init() };
    let key = information.key_name.split(|unit| *unit == 0).next()?;
    // China Standard Time includes the Windows mainland and Hong Kong zones
    // covered by the agent's time zone policy.
    (String::from_utf16_lossy(key) == "China Standard Time").then(|| "Asia/Shanghai".to_owned())
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
pub const CLAUDE_PERMISSION_MODES: [&str; 7] = [
    "default",
    "manual",
    "auto",
    "acceptEdits",
    "plan",
    "dontAsk",
    "bypassPermissions",
];

#[derive(Clone, Default, Deserialize)]
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
        snapshot_api_roles(&config, &source, assignments, &catalog, None)?;
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
            gateway_connection(&provider).is_some() || keyless_official(&config, &provider),
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
    pub gateway_refresh: Option<ClaudeGatewayRefresh>,
    pub subscription_settings: Option<Value>,
    pub subscription_role: Option<&'static str>,
}

impl ClaudeLaunch {
    pub fn for_fork_rebuild(&self, parent: &str, child: &str) -> Result<Self> {
        validate_fork_rebuild_ids(parent, child)?;
        let original = &self.launch;
        let argv = claude_argv(
            &self.model,
            self.effort.as_deref(),
            Some(ClaudeTarget::RebuildFork { parent, child }),
            Some(&self.permission_mode),
        );
        Ok(Self {
            launch: ShellLaunch {
                profile: original.profile,
                executable: original.executable.clone(),
                argv,
                cwd: original.cwd.clone(),
                data_root: original.data_root.clone(),
                inherited_env: original.inherited_env.clone(),
                env: original.env.clone(),
                cols: original.cols,
                rows: original.rows,
            },
            provider_config_id: self.provider_config_id.clone(),
            model: self.model.clone(),
            permission_mode: self.permission_mode.clone(),
            assigned_role: self.assigned_role,
            effort: self.effort.clone(),
            clear_assignment_environment: self.clear_assignment_environment,
            gateway: self.gateway.clone(),
            gateway_refresh: self.gateway_refresh.clone(),
            subscription_settings: self.subscription_settings.clone(),
            subscription_role: self.subscription_role,
        })
    }

    pub fn project_time_zone(&mut self, setting: &str, system: &str) {
        self.project_time_zone_with_exit(setting, system, None);
    }

    pub fn project_time_zone_with_exit(
        &mut self,
        setting: &str,
        system: &str,
        codex_exit: Option<&str>,
    ) {
        let official = official_time_zone_endpoint(
            get_var(&self.launch.env, "ANTHROPIC_BASE_URL"),
            "api.anthropic.com",
        ) || self.gateway.as_ref().is_some_and(|gateway| {
            gateway.subscription_providers().next().is_some()
                || gateway
                    .connections()
                    .skip(usize::from(gateway.source.is_some()))
                    .any(|target| target.endpoint.host_str() == Some("api.anthropic.com"))
        });
        let exit = codex_exit.filter(|_| {
            self.gateway.as_ref().is_some_and(|gateway| {
                gateway
                    .subscription_providers()
                    .any(|provider| provider == "codex")
            })
        });
        if let Some(zone) = cli_time_zone("claude-code", setting, system, exit, official) {
            set_var(&mut self.launch.env, "TZ".into(), zone.into());
        }
    }
}

/// Private provider settings captured for rebuilding a running gateway's roles.
#[derive(Clone)]
pub struct ClaudeGatewayRefresh {
    config: Value,
    source: Provider,
    startup_role: Option<&'static str>,
    exact_roles: bool,
    official_environment: OfficialClaudeEnvironment,
}

impl ClaudeGatewayRefresh {
    pub fn refresh(&self, assignments: &ClaudeRoleAssignments) -> Option<ClaudeGatewayProjection> {
        if assignments.is_empty()
            || assignments.iter().any(|(role, assignment)| {
                !ROLES.contains(&role.as_str())
                    || assignment
                        .effort
                        .as_deref()
                        .is_some_and(|effort| !EFFORTS.contains(&effort))
            })
        {
            return None;
        }
        gateway_projection(
            &self.config,
            &self.source,
            Some(assignments),
            self.startup_role,
        )
    }

    pub fn refresh_roles(
        &self,
        current: &ClaudeGatewayProjection,
        preferences: &ClaudeRoleLaunchPreferences,
        load_native_catalog: &dyn Fn() -> Vec<Value>,
    ) -> Option<ClaudeGatewayProjection> {
        let assignments = &preferences.assignments;
        if assignments.is_empty() {
            return None;
        }
        // Exact startup routes can use the native catalog and official OAuth.
        let ordinary = (!self.exact_roles)
            .then(|| self.refresh(assignments))
            .flatten();
        let roles = match ordinary {
            Some(projection) => projection.roles,
            None => {
                let official = OfficialCapture::new(&self.config, self.official_environment.clone());
                let native_catalog = std::cell::OnceCell::new();
                let catalog = || native_catalog.get_or_init(load_native_catalog).as_slice();
                snapshot_api_roles(
                    &self.config,
                    &self.source,
                    assignments,
                    &catalog,
                    Some(&official),
                )
                .ok()?
            }
        };
        let mut projection = current.clone();
        projection.roles = roles;
        projection.has_role_assignments = true;
        projection.role_assignments = assignments.clone();
        if (0..ROLES.len()).any(|index| {
            current.role_target(index).is_some() && projection.role_target(index).is_none()
        }) {
            return None;
        }
        for role in projection.roles.iter_mut().flatten() {
            role.context_window = effective_context_window(
                role.context_window,
                role.max_context_window,
                preferences.context_preference(&role.provider_id),
            );
        }
        Some(projection)
    }
}

/// Private launch material for a CLI run.
/// Never serialize or format it as part of a launch diagnostic.
#[derive(Clone)]
pub struct ClaudeGatewayProjection {
    pub source: Option<ClaudeGatewayConnection>,
    pub roles: [Option<ClaudeGatewayRole>; ROLES.len()],
    pub web_search: Option<ClaudeGatewayRole>,
    pub compaction: Option<ClaudeGatewayCompaction>,
    pub unknown_compaction_context: bool,
    pub context_window: Option<u64>,
    pub stream_keep_alive: bool,
    source_model: String,
    source_context_windows: BTreeMap<String, u64>,
    source_provider_id: String,
    source_models: [Option<String>; ROLES.len()],
    source_efforts: [Option<String>; ROLES.len()],
    source_effort: Option<String>,
    has_role_assignments: bool,
    role_assignments: ClaudeRoleAssignments,
    startup_role: Option<&'static str>,
}

#[derive(Clone)]
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
    compaction: Value,
    context_windows: Map<String, Value>,
    official_environment: OfficialClaudeEnvironment,
    subscription_provider: Option<&'static str>,
}

impl ClaudeLazyCompaction {
    pub fn resolve(
        &self,
        load_native_catalog: &dyn Fn() -> Vec<Value>,
    ) -> Result<ClaudeGatewayRole> {
        let config = read_provider_config(&self.data_root)?;
        let native_catalog = std::cell::OnceCell::new();
        let catalog = || native_catalog.get_or_init(load_native_catalog).as_slice();
        let official = OfficialCapture::new(&config, self.official_environment.clone());
        let mut target = compaction_target(&config, &self.compaction, &catalog, &official)?;
        // The old API route has no directory output cap. A missing explicit
        // effort follows the working supplier, not the compaction supplier.
        if self.compaction["effort"] == "follow" {
            target.effort = None;
        }
        target.model = if let Some(subscription) = &target.connection.subscription {
            format!(
                "{}/{}",
                subscription.prefix(),
                without_context_suffix(&target.catalog_model)
            )
        } else if target.connection.endpoint.origin().ascii_serialization() == OFFICIAL_ENDPOINT {
            catalog()
                .iter()
                .find(|model| model["id"].as_str() == Some(&target.catalog_model))
                .and_then(|model| model["resolvedModel"].as_str())
                .unwrap_or(&target.catalog_model)
                .to_owned()
        } else {
            target.catalog_model.clone()
        };
        target.context_window = effective_context_window(
            target.context_window,
            target.max_context_window,
            self.context_windows.get(&target.provider_id),
        );
        target.stream_keep_alive |= is_gemini_model(&target.model);
        target.max_output_tokens = None;
        Ok(target)
    }
}

/// Decode a compressed gateway error within the caller's inspection budget.
pub fn decode_claude_gateway_error(encoding: &str, body: &[u8], limit: usize) -> Option<Vec<u8>> {
    use flate2::{Decompress, FlushDecompress, Status, read::MultiGzDecoder};

    let mut decoded = Vec::new();
    match encoding {
        "gzip" => {
            MultiGzDecoder::new(body)
                .take((limit + 1) as u64)
                .read_to_end(&mut decoded)
                .ok()?;
        }
        "deflate" => {
            let mut decoder = Decompress::new(true);
            decoded.resize(limit + 1, 0);
            if decoder
                .decompress(body, &mut decoded, FlushDecompress::Finish)
                .ok()?
                != Status::StreamEnd
            {
                return None;
            }
            decoded.truncate(decoder.total_out() as usize);
        }
        _ => return None,
    }
    (decoded.len() <= limit).then_some(decoded)
}

/// A route the runtime must bind before opening a connection.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PreparedSubscriptionTarget {
    pub provider_id: String,
    pub provider: &'static str,
    /// The saved raw model for an exact route, or none for the four role aliases.
    pub model: Option<String>,
}

#[derive(Clone)]
pub struct ClaudeGatewayConnection {
    pub endpoint: url::Url,
    pub api_key: String,
    pub official_oauth: Option<OfficialClaudeOAuth>,
    pub subscription: Option<PreparedSubscriptionTarget>,
}

/// Four-tier projection from the managed runtime. It contains only CLI-safe values.
pub struct PreparedSubscriptionLaunch {
    pub models: [String; ROLES.len()],
    pub environment: BTreeMap<String, String>,
    pub clear_environment: std::collections::BTreeSet<String>,
    pub settings: Value,
    pub main_model: String,
}

impl ClaudeLaunch {
    pub fn apply_subscription_launch(&mut self, projection: PreparedSubscriptionLaunch) {
        let gateway = self.gateway.as_mut().expect("managed subscription gateway");
        for (index, model) in projection.models.into_iter().enumerate() {
            if let Some(window) = gateway.source_models[index]
                .as_deref()
                .and_then(|raw| {
                    gateway
                        .source_context_windows
                        .get(without_context_suffix(raw))
                })
                .copied()
            {
                gateway
                    .source_context_windows
                    .insert(without_context_suffix(&model).to_owned(), window);
            }
            gateway.source_models[index] = Some(model);
        }
        for key in projection.clear_environment {
            remove_var(&mut self.launch.env, OsStr::new(&key));
        }
        for (key, value) in projection.environment {
            set_var(&mut self.launch.env, key.into(), value.into());
        }
        gateway.context_window = projection.settings["autoCompactWindow"].as_u64();
        self.model = projection.main_model;
        if let [flag, model, ..] = self.launch.argv.as_mut_slice()
            && flag == "--model"
        {
            *model = self.model.clone();
        } else {
            self.launch
                .argv
                .splice(..0, ["--model".into(), self.model.clone()]);
        }
        self.subscription_settings = Some(projection.settings);
    }
}

#[derive(Clone)]
pub struct ClaudeGatewayRole {
    pub connection: ClaudeGatewayConnection,
    pub model: String,
    catalog_model: String,
    provider_id: String,
    pub provider_name: String,
    pub effort: Option<String>,
    pub supported_efforts: Option<Vec<String>>,
    pub max_output_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub stream_keep_alive: bool,
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
    pub fn role_assignments(&self) -> &ClaudeRoleAssignments {
        &self.role_assignments
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

    pub fn subscription_providers(&self) -> impl Iterator<Item = &'static str> {
        let lazy = match &self.compaction {
            Some(ClaudeGatewayCompaction::OnFirstRequest(route)) => route.subscription_provider,
            _ => None,
        };
        self.connections()
            .filter_map(|connection| {
                connection
                    .subscription
                    .as_ref()
                    .map(|subscription| subscription.provider)
            })
            .chain(lazy)
    }

    pub fn request_role(&self, model: &str) -> Option<usize> {
        if !self.has_role_assignments {
            return None;
        }
        let model = without_context_suffix(model);
        if let Some(source) = self
            .source
            .as_ref()
            .and_then(|source| source.subscription.as_ref())
            && source.model.is_none()
            && model
                .strip_prefix(source.prefix())
                .is_some_and(|rest| rest.starts_with('/'))
        {
            return None;
        }
        if let Some(alias) = startup_role_alias(model) {
            return ROLES.iter().position(|role| *role == alias);
        }
        self.source_models.iter().position(|candidate| {
            candidate
                .as_deref()
                .is_some_and(|id| without_context_suffix(id) == model)
        })
    }

    pub fn role_target(&self, index: usize) -> Option<&ClaudeGatewayRole> {
        self.roles[index].as_ref().or_else(|| {
            (ROLES[index] == "fable")
                .then(|| self.roles[0].as_ref())
                .flatten()
        })
    }

    pub fn literal_effort(&self, model: &str) -> Option<&str> {
        literal_role_index(model)
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
        let index = if self
            .source
            .as_ref()
            .is_some_and(|source| source.subscription.is_some())
        {
            let plain = without_context_suffix(model);
            if plain.starts_with("matou-chatgpt/") || plain.starts_with("matou-antigravity/") {
                return None;
            }
            let role = startup_role_alias(plain)?;
            ROLES.iter().position(|candidate| *candidate == role)?
        } else {
            literal_role_index(model)?
        };
        self.source_models[index].as_deref()
    }

    fn direct_gateway_required(&self) -> bool {
        self.stream_keep_alive
            || self.source.as_ref().is_some_and(|source| {
                source.subscription.is_some()
                    || (source.endpoint.host_str() == Some("api.deepseek.com")
                        && source.endpoint.path().trim_end_matches('/') == "/anthropic")
            })
    }

    fn apply_context_preferences(
        &mut self,
        config: &Value,
        preferences: &ClaudeRoleLaunchPreferences,
    ) {
        self.source_context_windows = source_context_windows(
            config,
            &self.source_provider_id,
            preferences.context_windows.get(&self.source_provider_id),
        );
        for role in self
            .roles
            .iter_mut()
            .flatten()
            .chain(self.web_search.iter_mut())
        {
            role.context_window = effective_context_window(
                role.context_window,
                role.max_context_window,
                preferences.context_windows.get(&role.provider_id),
            );
        }
    }

    pub fn literal_context_window(&self, model: &str) -> Option<u64> {
        let model = self.literal_model_override(model).unwrap_or(model);
        self.source_context_windows
            .get(without_context_suffix(model))
            .copied()
    }

    pub fn literal_stream_keep_alive(&self, model: &str) -> bool {
        if !self.stream_keep_alive {
            return false;
        }
        let candidate = literal_role_index(model)
            .or_else(|| {
                role_with_effort(model, |role| {
                    let index = ROLES.iter().position(|candidate| *candidate == role)?;
                    self.source_models[index].as_deref()
                })
            })
            .and_then(|index| self.source_models[index].as_deref())
            .unwrap_or(&self.source_model);
        is_gemini_model(candidate)
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
        let env = |key| get_var(&launch.env, key).or_else(|| get_var(&launch.inherited_env, key));
        let no_proxy = env("no_proxy")
            .or_else(|| env("NO_PROXY"))
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
        for (index, role) in ROLES.into_iter().enumerate() {
            let (prefix, name) = if self.has_role_assignments {
                ("matou-role", self.display_model(index))
            } else if self
                .source
                .as_ref()
                .is_some_and(|source| source.subscription.is_some())
            {
                continue;
            } else if let Some(model) = self.source_models[index]
                .as_deref()
                .filter(|model| !model.is_empty())
            {
                ("matou-api", model)
            } else {
                continue;
            };
            let key = role_model_env_key(role);
            projected.insert(format!("{key}_NAME"), name.into());
            projected.insert(key, format!("{prefix}/{role}"));
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
            let label = CLAUDE_ROLE_LABELS[index];
            let model = self.display_model(index);
            let provider_id = self
                .role_target(index)
                .map_or(self.source_provider_id.as_str(), |target| {
                    &target.provider_id
                });
            serde_json::json!({
                "model": role,
                "label": format!("{label} · {model}"),
                "description": provider_id,
            })
        });
        Some(serde_json::json!({
            "replaceBuiltInOptions": true,
            "options": options,
        }))
    }

    fn display_model(&self, index: usize) -> &str {
        self.role_target(index)
            .map_or(ROLES[index], |target| &target.model)
    }
}

impl ClaudeLaunch {
    pub fn capture_restart_gateway(
        &mut self,
        preferences: &ClaudeRoleLaunchPreferences,
        load_native_catalog: &dyn Fn() -> Vec<Value>,
        launch_environment: &BTreeMap<OsString, OsString>,
    ) -> Result<()> {
        let data_root = &self.launch.data_root;
        let config = read_provider_config(data_root)?;
        // The bound launch determines the conversation and restored model.
        // The legacy compaction gateway only adds a route to that launch.
        let independent = has_independent_target(preferences);
        let role_snapshot = has_role_snapshot(preferences);
        let bound_subscription = self
            .gateway
            .as_ref()
            .and_then(|gateway| gateway.source.as_ref())
            .filter(|source| {
                source
                    .subscription
                    .as_ref()
                    .is_some_and(|target| target.provider_id == self.provider_config_id)
            })
            .cloned();
        let source_binding = if !role_snapshot || bound_subscription.is_some() {
            json!({"providerConfigId": self.provider_config_id})
        } else {
            Value::Null
        };
        let source = read_provider(data_root, &config, &source_binding)?;
        let role = self
            .assigned_role
            .or_else(|| model_role(&source, &self.model));
        let official =
            OfficialCapture::new(&config, OfficialClaudeEnvironment::new(launch_environment));
        let exact = independent || requires_role_snapshot(&config, &source, preferences, &official);
        self.gateway =
            if exact {
                let (mut projection, context_window) = independent_gateway_projection(
                    &config,
                    &source,
                    preferences,
                    role,
                    data_root,
                    &official,
                    load_native_catalog,
                )?;
                if role_snapshot {
                    project_context_window(&mut self.launch.env, context_window);
                }
                if let Some(source) = bound_subscription {
                    // An exact role or search route can coexist with the restored
                    // conversation's original subscription and model.
                    projection.source = Some(source);
                    projection.startup_role = None;
                }
                Some(projection)
            } else {
                gateway_projection(&config, &source, Some(&preferences.assignments), role)
            };
        if !independent
            && !role_snapshot
            && self.assigned_role.is_none()
            && !source.is_subscription()
        {
            let key = "CLAUDE_CODE_MAX_CONTEXT_TOKENS";
            let current = get_var(&self.launch.env, key);
            // Restart preferences become available after the initial plan has
            // already projected the catalog fallback for the restored model.
            if current.is_none()
                || (preferences.context_preference(&source.id).is_some()
                    && get_var(launch_environment, key).is_none()
                    && single_provider_context_window(&config, &source, &self.model, None)
                        .is_some_and(|window| current == Some(OsStr::new(&window.to_string()))))
            {
                let window = single_provider_context_window(
                    &config,
                    &source,
                    &self.model,
                    Some(preferences),
                );
                project_context_window(&mut self.launch.env, window);
            }
        }
        if let Some(projection) = &mut self.gateway {
            projection.context_window = get_var(&self.launch.env, "CLAUDE_CODE_MAX_CONTEXT_TOKENS")
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse().ok())
                .or(projection.context_window);
            projection.apply_context_preferences(&config, preferences);
        }
        let official_environment = official.environment;
        self.gateway_refresh = self.gateway.as_ref().map(|_| ClaudeGatewayRefresh {
            config,
            source,
            startup_role: role,
            exact_roles: exact && role_snapshot,
            official_environment,
        });
        Ok(())
    }
}

/// Put the private run settings before all provider selection flags.
pub fn attach_claude_settings(launch: &mut ShellLaunch, path: &Path) -> Result<()> {
    let path = path
        .to_str()
        .context("provider settings path is not UTF-8")?;
    launch.argv.splice(..0, ["--settings".into(), path.into()]);
    // Claude treats bearer and API-key authentication as mutually exclusive.
    if get_var(&launch.env, "ANTHROPIC_AUTH_TOKEN").is_some_and(|token| !token.is_empty()) {
        remove_var(&mut launch.env, OsStr::new("ANTHROPIC_API_KEY"));
    }
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
        false,
    )
}

pub fn prepare_claude_fork_rebuild(
    data_root: &Path,
    cwd: &Path,
    binding: &Value,
    (parent, child): (&str, &str),
    environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<ClaudeLaunch> {
    prepare_claude(
        data_root,
        cwd,
        binding,
        Some(ClaudeTarget::RebuildFork { parent, child }),
        environment,
        cols,
        rows,
        false,
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
        false,
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
        false,
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
        false,
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
        false,
    )
}

#[derive(Clone, Copy)]
pub enum ClaudeTarget<'a> {
    New {
        assignments: Option<&'a ClaudeRoleAssignments>,
        budget_preferences: Option<(&'a ClaudeRoleLaunchPreferences, &'a dyn Fn() -> Vec<Value>)>,
    },
    PermissionRestart {
        remembered: bool,
    },
    Resume(&'a str),
    Fork(&'a str),
    RebuildFork {
        parent: &'a str,
        child: &'a str,
    },
}

fn validate_fork_rebuild_ids(parent: &str, child: &str) -> Result<()> {
    let parent = parent.trim();
    let child = child.trim();
    ensure!(
        !parent.is_empty() && !child.is_empty(),
        "Fork rebuild requires both a parent and a child conversation ID"
    );
    ensure!(
        !parent.eq_ignore_ascii_case(child)
            && uuid::Uuid::parse_str(parent)
                .ok()
                .is_none_or(|parent| { uuid::Uuid::parse_str(child) != Ok(parent) }),
        "Fork rebuild requires distinct parent and child conversation IDs"
    );
    Ok(())
}

pub fn prepare_managed_claude_launch(
    data_root: &Path,
    cwd: &Path,
    binding: &Value,
    target: Option<ClaudeTarget<'_>>,
    environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<ClaudeLaunch> {
    if let Some(ClaudeTarget::Fork(source_id)) = target {
        ensure!(
            !source_id.trim().is_empty(),
            "Fork source conversation is missing"
        );
    }
    prepare_claude(
        data_root,
        cwd,
        binding,
        target,
        environment,
        cols,
        rows,
        true,
    )
}

fn prepare_claude(
    data_root: &Path,
    cwd: &Path,
    binding: &Value,
    target: Option<ClaudeTarget<'_>>,
    mut environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
    subscriptions: bool,
) -> Result<ClaudeLaunch> {
    if let Some(ClaudeTarget::RebuildFork { parent, child }) = target {
        validate_fork_rebuild_ids(parent, child)?;
    }
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
    let locale = || current_interface_locale(read_locale_preference(data_root));
    let service_unavailable = || {
        locale().text(
            "订阅服务未启动，请重新打开设置或重启码头",
            "The subscription service hasn't started. Reopen Settings or restart Matou.",
        )
    };
    if subscriptions && provider.is_subscription() {
        ensure!(
            gateway_connection(&provider).is_some(),
            "{}",
            subscription_account_unavailable(data_root)
        );
    }
    let bound_role = if matches!(
        target,
        Some(
            ClaudeTarget::Resume(_)
                | ClaudeTarget::RebuildFork { .. }
                | ClaudeTarget::PermissionRestart { .. }
        )
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
    if !subscriptions
        && provider.is_subscription()
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
            "{}",
            locale().text(
                "订阅账号的模型信息缺失，请刷新账号或重新登录",
                "Model information is missing for this subscription account. Refresh the account or sign in again.",
            )
        );
        bail!("{}", service_unavailable());
    }

    let assigned = match assignments {
        Some(assignments) => resolve_startup_assignment(&config, &provider, assignments)?,
        None => None,
    };
    let official = OfficialCapture::new(&config, OfficialClaudeEnvironment::new(&environment));
    let mut exact_roles = false;
    let (mut gateway, context_window) = match target {
        Some(ClaudeTarget::New {
            budget_preferences: Some((preferences, catalog)),
            ..
        }) if has_independent_target(preferences)
            || requires_role_snapshot(&config, &provider, preferences, &official) =>
        {
            exact_roles = has_role_snapshot(preferences);
            let (projection, context_window) = independent_gateway_projection(
                &config,
                &provider,
                preferences,
                startup_role(&provider),
                data_root,
                &official,
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
        _ => (
            gateway_projection(&config, &provider, None, None)
                .filter(|projection| projection.direct_gateway_required()),
            None,
        ),
    };
    let gateway_refresh = gateway.as_ref().map(|_| ClaudeGatewayRefresh {
        config: config.clone(),
        source: provider.clone(),
        startup_role: startup_role(&provider),
        exact_roles,
        official_environment: official.environment.clone(),
    });
    let stored_model = provider_binding["model"].as_str();
    let selected_model = match stored_model {
        Some(bound)
            if !provider.is_subscription()
                && (bound.starts_with("matou-chatgpt/")
                    || bound.starts_with("matou-antigravity/")) =>
        {
            &provider.model
        }
        Some(bound) => bound,
        None if provider.is_subscription() => "opus",
        None => &provider.model,
    };
    // Translate a model selected through the request gateway for direct launches.
    let selected_role = if provider.is_subscription() {
        model_role(&provider, selected_model)
    } else {
        literal_role_index(selected_model).map(|index| ROLES[index])
    };
    let model = match selected_role {
        Some(role) => effective_role_model(&provider, role).unwrap_or_else(|| {
            if selected_model.starts_with("matou-api/") {
                &provider.model
            } else {
                selected_model
            }
        }),
        None => selected_model,
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
            let restored_role = restored_model_role(
                target,
                stored_model.filter(|model| *model == selected_model),
            );
            let effort = if bound_role.is_some() || remembered {
                // Preserve omitted effort for remembered concrete models.
                provider_binding["effort"].as_str().or_else(|| {
                    let role = restored_role?;
                    reasoning_effort(&provider, model, selected_role.or(Some(role)))
                })
            } else {
                reasoning_effort(&provider, model, selected_role.or(restored_role))
            };
            (&provider, model, effort, bound_role)
        }
    };
    ensure!(
        subscriptions
            || gateway.as_ref().is_none_or(|plan| {
                plan.connections()
                    .all(|connection| connection.subscription.is_none())
            }) && !provider.is_subscription(),
        "{}",
        service_unavailable()
    );
    let subscription_role = if provider.is_subscription() {
        selected_role.or_else(|| model_role(provider, model))
    } else {
        None
    };
    let clear_assignment_environment = gateway
        .as_ref()
        .is_some_and(|projection| projection.web_search.is_some())
        || assigned_role.is_some()
        || assignments.is_some_and(|assignments| !assignments.is_empty())
        || (remembered && provider_binding["clearAssignmentEnvironment"] == true);
    clear_role_model_environment(&mut environment);
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
    let preferences = match target {
        Some(ClaudeTarget::New {
            budget_preferences, ..
        }) => budget_preferences.map(|(preferences, _)| preferences),
        _ => None,
    };
    let context_window = context_window.or_else(|| {
        if !provider.is_subscription()
            && assigned_role.is_none()
            && assignments.is_none_or(BTreeMap::is_empty)
            && preferences.is_none_or(|preferences| !has_independent_target(preferences))
        {
            single_provider_context_window(&config, provider, model, preferences)
        } else {
            provider_entry(&config, &provider.id)
                .and_then(|entry| custom_context_window(&entry["contextWindow"]))
        }
    });
    project_context_window(&mut environment, context_window);
    if let Some(projection) = &mut gateway {
        projection.context_window = context_window;
        if let Some(ClaudeTarget::New {
            budget_preferences: Some((preferences, _)),
            ..
        }) = target
        {
            projection.apply_context_preferences(&config, preferences);
        }
    }
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
        gateway_refresh,
        subscription_settings: None,
        subscription_role,
    })
}

fn gateway_projection(
    config: &Value,
    source: &Provider,
    assignments: Option<&ClaudeRoleAssignments>,
    startup_role: Option<&'static str>,
) -> Option<ClaudeGatewayProjection> {
    let mut projection = api_projection(config, source);
    let Some(assignments) = assignments.filter(|assignments| !assignments.is_empty()) else {
        return (projection.source.is_some()
            && (projection.direct_gateway_required()
                || projection
                    .source_efforts
                    .iter()
                    .chain([&projection.source_effort])
                    .any(|effort| {
                        effort
                            .as_deref()
                            .is_some_and(|level| EFFORTS.contains(&level) && level != "follow")
                    })))
        .then_some(projection);
    };
    // A keyless source needs API targets for every role. Fable can use Opus.
    if projection.source.is_none()
        && (startup_role.is_none() || roles_need_source(source, assignments))
    {
        return None;
    }
    let providers = provider_entries(config).unwrap_or_default();
    let target = |provider: &Provider, role: &str, model: &str, effort: Option<&str>| {
        let entry = providers
            .iter()
            .find(|entry| entry["id"].as_str() == Some(&provider.id));
        let metadata = entry
            .and_then(|entry| {
                if provider.is_subscription() {
                    entry["subscription"]["modelMetadata"].as_array()
                } else {
                    entry["modelMetadata"].as_array()
                }
            })
            .and_then(|models| {
                models
                    .iter()
                    .find(|item| item["id"].as_str() == Some(model.trim()))
            });
        gateway_target(
            gateway_connection(provider)?,
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
            mapped_role_model(source, role).and_then(|model| target(source, role, model, None))
        };
    }
    if let Some(index) = ROLES.iter().position(|role| Some(*role) == startup_role)
        && projection.role_target(index).is_none()
    {
        return None;
    }
    projection.has_role_assignments = true;
    projection.role_assignments = assignments.clone();
    projection.startup_role = startup_role;
    Some(projection)
}

pub fn is_gemini_model(model: &str) -> bool {
    model.trim().split('/').any(|part| {
        part.get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("gemini-"))
    })
}

fn api_projection(config: &Value, source: &Provider) -> ClaudeGatewayProjection {
    ClaudeGatewayProjection {
        source: gateway_connection(source),
        roles: [const { None }; ROLES.len()],
        web_search: None,
        compaction: None,
        unknown_compaction_context: false,
        context_window: None,
        stream_keep_alive: std::iter::once(&source.model)
            .chain(source.model_mapping.values())
            .any(|model| is_gemini_model(model)),
        source_model: source.model.clone(),
        source_context_windows: source_context_windows(config, &source.id, None),
        source_provider_id: source.id.clone(),
        source_models: ROLES.map(|role| effective_role_model(source, role).map(str::to_owned)),
        source_efforts: ROLES.map(|role| {
            source
                .effort_mapping
                .get(provider_mapping_role(source, role))
                .cloned()
        }),
        source_effort: source.reasoning_effort.clone(),
        has_role_assignments: false,
        role_assignments: ClaudeRoleAssignments::new(),
        startup_role: None,
    }
}

fn effective_context_window(
    default: Option<u64>,
    maximum: Option<u64>,
    preference: Option<&Value>,
) -> Option<u64> {
    if let Some(window) = preference.and_then(custom_context_window) {
        return Some(window);
    }
    match preference.and_then(Value::as_str) {
        Some("maximum") => maximum.or(default),
        Some("512k") => default.map(|window| window.min(512_000)),
        _ => default,
    }
}

fn source_context_windows(
    config: &Value,
    provider_id: &str,
    preference: Option<&Value>,
) -> BTreeMap<String, u64> {
    provider_entry(config, provider_id)
        .and_then(|provider| provider["modelMetadata"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|metadata| {
            let window = effective_context_window(
                context_tokens(Some(metadata), "contextWindow"),
                context_tokens(Some(metadata), "maxContextWindow"),
                preference,
            )?;
            Some((metadata["id"].as_str()?.to_owned(), window))
        })
        .collect()
}

fn gateway_target(
    mut connection: ClaudeGatewayConnection,
    provider: &Provider,
    provider_name: &str,
    model: &str,
    metadata: Option<&Value>,
    role: Option<&str>,
    effort: Option<&str>,
) -> Option<ClaudeGatewayRole> {
    if let Some(subscription) = &mut connection.subscription {
        subscription.model = Some(model.trim().to_owned());
    }
    let catalog_model = model.trim();
    if catalog_model.is_empty() {
        return None;
    }
    let model = if let Some(subscription) = &connection.subscription {
        format!("{}/{catalog_model}", subscription.prefix())
    } else {
        metadata
            .and_then(|item| item["resolvedModel"].as_str())
            .filter(|model| !model.is_empty())
            .unwrap_or(catalog_model)
            .to_owned()
    };
    let effort = effort
        .or_else(|| role.and_then(|role| provider.effort_mapping.get(role).map(String::as_str)))
        .or(provider.reasoning_effort.as_deref())
        .filter(|level| EFFORTS.contains(level) && *level != "follow")
        .map(str::to_owned);
    Some(ClaudeGatewayRole {
        stream_keep_alive: is_gemini_model(catalog_model) || is_gemini_model(&model),
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

fn requires_role_snapshot(
    config: &Value,
    source: &Provider,
    preferences: &ClaudeRoleLaunchPreferences,
    official: &OfficialCapture<'_>,
) -> bool {
    has_role_snapshot(preferences)
        && (source.is_subscription()
            || preferences.assignments.values().any(|target| {
                match provider_entry(config, &target.provider_id) {
                    Some(row) => {
                        row["kind"] == "subscription"
                            || (target.provider_id == OFFICIAL_ID
                                && uses_native_catalog(row)
                                && has_no_api_key(row["apiKey"].as_str())
                                // A partial snapshot still needs a working source.
                                // Keep legacy direct launches for an unbound source.
                                && (!roles_need_source(source, &preferences.assignments)
                                    || keyless_official(config, source)
                                    || gateway_connection(source).is_some()))
                    }
                    // Preserve direct CLI login for a role on the built-in
                    // default when the user hasn't configured an account yet.
                    None => {
                        target.provider_id == OFFICIAL_ID
                            && source.id == OFFICIAL_ID
                            && provider_entries(config).is_none()
                            && official.connection(source, OFFICIAL_ID).is_some()
                    }
                }
            }))
}

fn independent_gateway_projection(
    config: &Value,
    source: &Provider,
    preferences: &ClaudeRoleLaunchPreferences,
    startup_role: Option<&'static str>,
    data_root: &Path,
    official: &OfficialCapture<'_>,
    load_native_catalog: &dyn Fn() -> Vec<Value>,
) -> Result<(ClaudeGatewayProjection, Option<u64>)> {
    let mut projection = api_projection(config, source);
    if !has_role_snapshot(preferences) {
        if projection.source.is_none() {
            let name = provider_name(routing_provider_entry(config, &source.id), &source.id);
            projection.source = official.connection(source, name);
        }
        ensure!(
            projection.source.is_some(),
            "Original supplier source connection was not bound at startup; choose a configured role"
        );
        projection.compaction = Some(ClaudeGatewayCompaction::OnFirstRequest(
            ClaudeLazyCompaction {
                data_root: data_root.to_owned(),
                compaction: preferences.compaction.clone(),
                context_windows: preferences.context_windows.clone(),
                official_environment: official.environment.clone(),
                subscription_provider: preferences.compaction["providerId"]
                    .as_str()
                    .and_then(|id| provider_entry(config, id))
                    .and_then(subscription_kind),
            },
        ));
        return Ok((projection, None));
    }

    let native_catalog = std::cell::OnceCell::new();
    let catalog = || native_catalog.get_or_init(load_native_catalog).as_slice();
    if let Some(assignment) = &preferences.web_search {
        let name = provider_name(
            routing_provider_entry(config, &assignment.provider_id),
            &assignment.provider_id,
        );
        let unavailable = || format!("网页搜索 · {name}: model target is unavailable");
        let (provider, metadata) = exact_api_provider(config, assignment, &catalog)
            .map_err(|_| anyhow::anyhow!(unavailable()))?;
        let connection = official
            .connection(&provider, name)
            .with_context(unavailable)?;
        projection.web_search = Some(
            gateway_target(
                connection,
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
        let target = compaction_target(config, &preferences.compaction, &catalog, official)?;
        projection.compaction = Some(ClaudeGatewayCompaction::Snapshot {
            target,
            inherit_role_effort: preferences.compaction["effort"] == "follow",
        });
    }
    projection.roles = snapshot_api_roles(
        config,
        source,
        &preferences.assignments,
        &catalog,
        Some(official),
    )?;
    if source.is_subscription() || keyless_official(config, source) {
        let default_model = source
            .model_mapping
            .get("opus")
            .filter(|model| !model.is_empty())
            .unwrap_or(&source.model);
        // Reuse an already prepared connection. The original source does not
        // require its own subscription router or official account capture.
        projection.source = projection
            .roles
            .iter()
            .flatten()
            .chain(projection.web_search.iter())
            .chain(
                projection
                    .compaction
                    .iter()
                    .filter_map(|compaction| match compaction {
                        ClaudeGatewayCompaction::Snapshot { target, .. } => Some(target),
                        ClaudeGatewayCompaction::OnFirstRequest(_) => None,
                    }),
            )
            .find(|target| {
                target.provider_id == source.id
                    && (!source.is_subscription() || target.catalog_model == *default_model)
            })
            .map(|target| target.connection.clone());
    }
    projection.has_role_assignments = true;
    projection.role_assignments = preferences.assignments.clone();
    projection.startup_role = startup_role;
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
        std::array::from_fn(|index| {
            projection
                .role_target(index)
                .expect("complete Claude role snapshot")
                .budget_target()
        }),
        compaction,
        &context_windows(config, preferences),
    )?;
    projection.unknown_compaction_context = unknown_compaction_context;
    projection.context_window = context_window;
    Ok((projection, context_window))
}

fn compaction_target<'a>(
    config: &'a Value,
    compaction: &Value,
    native_catalog: &impl Fn() -> &'a [Value],
    official: &OfficialCapture<'_>,
) -> Result<ClaudeGatewayRole> {
    let mut assignment = parse_assignment(compaction)?;
    let name = provider_name(
        routing_provider_entry(config, &assignment.provider_id),
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
    let connection = official
        .connection(&provider, name)
        .with_context(unavailable)?;
    gateway_target(
        connection,
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

fn routing_provider_entry<'a>(config: &'a Value, id: &str) -> Option<&'a Value> {
    static OFFICIAL: std::sync::LazyLock<Value> = std::sync::LazyLock::new(
        || json!({"id": OFFICIAL_ID, "endpoint": OFFICIAL_ENDPOINT, "builtIn": true, "model": ""}),
    );
    provider_entry(config, id)
        .or_else(|| (id == OFFICIAL_ID && provider_entries(config).is_none()).then_some(&*OFFICIAL))
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
    let entry = routing_provider_entry(config, &assignment.provider_id)
        .filter(|entry| entry["cli"].as_str().is_none_or(|cli| cli == "claude-code"))
        .context("供应商已不存在")?;
    if entry["kind"] == "subscription" {
        ensure!(subscription_kind(entry).is_some(), "供应商已不存在");
    } else {
        let key = entry["apiKey"].as_str();
        ensure!(
            valid_api_key(key).is_some()
                || (assignment.provider_id == OFFICIAL_ID
                    && has_no_api_key(key)
                    && uses_native_catalog(entry)),
            "API Key 未配置"
        );
    }
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
    let models = if entry["kind"] == "subscription" {
        entry["subscription"]["modelMetadata"]
            .as_array()
            .context("模型目录暂不可用")?
    } else if uses_native_catalog(entry) {
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
    official: Option<&OfficialCapture<'_>>,
) -> Result<[Option<ClaudeGatewayRole>; ROLES.len()]> {
    let mut targets = [const { None }; ROLES.len()];
    for (index, role) in ROLES.into_iter().enumerate() {
        if fallback_role(source, assignments, role) != role {
            continue;
        }
        let assignment = assignments.get(role);
        let id = assignment.map_or(source.id.as_str(), |target| target.provider_id.as_str());
        let entry = routing_provider_entry(config, id)
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
        let unavailable = || format!("{role} · {name}: model target is unavailable");
        let Some(official) = official else {
            ensure!(
                gateway_connection(provider).is_some() || keyless_official(config, provider),
                "{}",
                unavailable()
            );
            continue;
        };
        let connection = official
            .connection(provider, name)
            .with_context(unavailable)?;
        targets[index] = Some(
            gateway_target(
                connection,
                provider,
                name,
                model,
                metadata,
                Some(role),
                assignment.and_then(|target| target.effort.as_deref()),
            )
            .with_context(unavailable)?,
        );
    }
    Ok(targets)
}

impl Provider {
    fn is_subscription(&self) -> bool {
        self.kind.as_deref() == Some("subscription")
    }
}

impl PreparedSubscriptionTarget {
    pub fn prefix(&self) -> &'static str {
        match self.provider {
            "codex" => "matou-chatgpt",
            _ => "matou-antigravity",
        }
    }
}

fn subscription_kind(entry: &Value) -> Option<&'static str> {
    if entry["kind"] != "subscription" {
        return None;
    }
    subscription_provider(&entry["subscription"])
}

fn subscription_provider(subscription: &Value) -> Option<&'static str> {
    match subscription["provider"].as_str()? {
        "codex" => Some("codex"),
        "antigravity" => Some("antigravity"),
        _ => None,
    }
}

fn has_no_api_key(key: Option<&str>) -> bool {
    key.is_none_or(|key| key.trim().is_empty())
}

fn valid_api_key(key: Option<&str>) -> Option<&str> {
    key.filter(|key| !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_graphic()))
}

fn keyless_official(config: &Value, provider: &Provider) -> bool {
    provider.id == OFFICIAL_ID
        && !provider.is_subscription()
        && provider.subscription.is_none()
        && has_no_api_key(provider.api_key.as_deref())
        && routing_provider_entry(config, &provider.id).is_some_and(uses_native_catalog)
}

struct OfficialCapture<'a> {
    config: &'a Value,
    environment: OfficialClaudeEnvironment,
    account: std::cell::OnceCell<Option<OfficialClaudeOAuth>>,
}

impl<'a> OfficialCapture<'a> {
    fn new(config: &'a Value, environment: OfficialClaudeEnvironment) -> Self {
        Self {
            config,
            environment,
            account: std::cell::OnceCell::new(),
        }
    }

    fn connection(&self, provider: &Provider, name: &str) -> Option<ClaudeGatewayConnection> {
        if !keyless_official(self.config, provider) {
            return gateway_connection(provider);
        }
        let account = self
            .account
            .get_or_init(|| self.environment.capture(name).ok());
        Some(ClaudeGatewayConnection {
            endpoint: url::Url::parse(OFFICIAL_ENDPOINT).expect("official HTTPS endpoint"),
            api_key: String::new(),
            official_oauth: Some(account.as_ref()?.clone()),
            subscription: None,
        })
    }
}

fn gateway_connection(provider: &Provider) -> Option<ClaudeGatewayConnection> {
    if provider.is_subscription() {
        let provider_kind = subscription_provider(provider.subscription.as_ref()?)?;
        return Some(ClaudeGatewayConnection {
            endpoint: url::Url::parse("http://127.0.0.1").expect("loopback endpoint"),
            api_key: String::new(),
            official_oauth: None,
            subscription: Some(PreparedSubscriptionTarget {
                provider_id: provider.id.clone(),
                provider: provider_kind,
                model: None,
            }),
        });
    }
    let api_key = valid_api_key(provider.api_key.as_deref())?;
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
        official_oauth: None,
        subscription: None,
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

fn subscription_account_unavailable(data_root: &Path) -> &'static str {
    current_interface_locale(read_locale_preference(data_root)).text(
        "还没有可用的订阅账号，请先在设置里添加并启用",
        "No subscription account is available. Add and enable one in Settings.",
    )
}

fn read_provider(data_root: &Path, config: &Value, binding: &Value) -> Result<Provider> {
    let bound_id = binding["providerConfigId"]
        .as_str()
        .filter(|id| !id.trim().is_empty());
    let missing = || {
        if binding["model"].as_str().is_some_and(|model| {
            model.starts_with("matou-chatgpt/") || model.starts_with("matou-antigravity/")
        }) {
            subscription_account_unavailable(data_root)
        } else {
            "会话绑定的供应商配置不存在"
        }
    };
    let Some(providers) = provider_entries(config) else {
        ensure!(bound_id.is_none_or(|id| id == OFFICIAL_ID), "{}", missing());
        return Ok(official_provider());
    };
    let active_id = config["activeProviderIds"]["claude-code"].as_str();
    let find = |id| {
        providers
            .iter()
            .find(|provider| provider["id"].as_str() == Some(id))
    };
    let selected = match bound_id {
        Some(id) => find(id).with_context(missing)?,
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
    if provider.is_subscription() {
        ensure!(
            subscription_kind(entry).is_some(),
            "Assignment provider is unavailable"
        );
        validate_assignment_model(
            &entry["subscription"]["modelMetadata"],
            &provider,
            assignment,
        )?;
        return Ok(provider);
    }
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
    if source.is_subscription() {
        return Some("opus");
    }
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

pub fn startup_role_alias(model: &str) -> Option<&'static str> {
    let model = without_context_suffix(model);
    if model == "matou-role/opus-fallback" {
        return Some("opus");
    }
    let role_model = model
        .strip_prefix("matou-role/")
        .or_else(|| model.strip_prefix("matou-api/"))
        .unwrap_or(model);
    if let Some(role) = ROLES.into_iter().find(|role| *role == role_model) {
        return Some(role);
    }
    let subscription_model = model.strip_prefix("matou-antigravity/").unwrap_or(model);
    match model
        .strip_prefix("matou-chatgpt/")
        .unwrap_or(subscription_model)
    {
        "matou-opus" | "matou-opus-fallback" => return Some("opus"),
        "matou-fable" => return Some("fable"),
        "matou-sonnet" => return Some("sonnet"),
        "matou-haiku" => return Some("haiku"),
        _ => {}
    }
    if subscription_model == "claude-fable-5" || subscription_model.starts_with("claude-fable-5-") {
        return Some("fable");
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
    let role = fallback_role(source, assignments, role);
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
        let role = fallback_role(source, &preferences.assignments, role);
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

fn is_builtin_claude_model(model: &str) -> bool {
    let model = without_context_suffix(model);
    [
        "",
        "default",
        CLI_DEFAULT_MODEL,
        "opus",
        "sonnet",
        "haiku",
        "fable",
    ]
    .iter()
    .any(|alias| model.eq_ignore_ascii_case(alias))
        || model
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("claude-"))
}

fn single_provider_context_window(
    config: &Value,
    provider: &Provider,
    model: &str,
    preferences: Option<&ClaudeRoleLaunchPreferences>,
) -> Option<u64> {
    let entry = provider_entry(config, &provider.id);
    let mode = preferences.and_then(|preferences| preferences.context_preference(&provider.id));
    if let Some(window) = entry
        .and_then(|entry| custom_context_window(&entry["contextWindow"]))
        .or_else(|| mode.and_then(custom_context_window))
    {
        return Some(window);
    }

    let metadata = |model: &str| {
        let model = without_context_suffix(model.trim());
        entry?["modelMetadata"]
            .as_array()?
            .iter()
            .find(|metadata| metadata["id"].as_str() == Some(model))
    };
    match mode.and_then(Value::as_str) {
        Some("512k") => Some(512_000),
        Some("maximum") => {
            let maximum = |model: &str| {
                let model = metadata(model);
                context_tokens(model, "maxContextWindow")
                    .or_else(|| context_tokens(model, "contextWindow"))
            };
            let main = maximum(model)?;
            provider
                .model_mapping
                .values()
                .filter(|model| !model.trim().is_empty())
                .try_fold(main, |window, model| Some(window.min(maximum(model)?)))
                .or(Some(main))
        }
        _ if provider.id != OFFICIAL_ID
            && provider.endpoint.trim_end_matches('/') != OFFICIAL_ENDPOINT
            && !is_builtin_claude_model(model.trim()) =>
        {
            context_tokens(metadata(model), "contextWindow")
        }
        _ => None,
    }
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
    match target {
        Some(ClaudeTarget::Resume(id)) => argv.extend(["--resume".into(), id.into()]),
        Some(ClaudeTarget::Fork(id)) => {
            argv.extend(["--resume".into(), id.into(), "--fork-session".into()]);
        }
        Some(ClaudeTarget::RebuildFork { parent, child }) => {
            argv.extend([
                "--resume".into(),
                parent.into(),
                "--fork-session".into(),
                "--session-id".into(),
                child.into(),
            ]);
        }
        _ => {}
    }
    match permission {
        Some("bypassPermissions") => argv.push("--dangerously-skip-permissions".into()),
        // The internal `default` name also works on versions that predate the Manual label.
        Some("manual") => argv.extend(["--permission-mode".into(), "default".into()]),
        Some(mode @ ("auto" | "acceptEdits" | "plan" | "dontAsk")) => {
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

fn mapped_role_model<'a>(provider: &'a Provider, role: &str) -> Option<&'a str> {
    provider
        .model_mapping
        .get(role)
        .map(String::as_str)
        .filter(|model| !model.trim().is_empty())
}

fn provider_mapping_role<'a>(provider: &Provider, role: &'a str) -> &'a str {
    if role == "fable" && mapped_role_model(provider, role).is_none() {
        "opus"
    } else {
        role
    }
}

fn effective_role_model<'a>(provider: &'a Provider, role: &str) -> Option<&'a str> {
    mapped_role_model(provider, provider_mapping_role(provider, role))
}

fn roles_need_source(source: &Provider, assignments: &ClaudeRoleAssignments) -> bool {
    ROLES
        .into_iter()
        .any(|role| !assignments.contains_key(fallback_role(source, assignments, role)))
}

fn fallback_role(
    source: &Provider,
    assignments: &ClaudeRoleAssignments,
    role: &'static str,
) -> &'static str {
    if assignments.contains_key(role) {
        role
    } else if role == "fable"
        && source.is_subscription()
        && mapped_role_model(source, role).is_some_and(|model| {
            source
                .subscription
                .as_ref()
                .and_then(|sub| sub["modelMetadata"].as_array())
                .is_none_or(|models| !models.iter().any(|item| item["id"] == model))
        })
    {
        "opus"
    } else {
        provider_mapping_role(source, role)
    }
}

fn restored_model_role(
    target: Option<ClaudeTarget<'_>>,
    stored_model: Option<&str>,
) -> Option<&'static str> {
    match target {
        Some(ClaudeTarget::Resume(_) | ClaudeTarget::PermissionRestart { .. }) => {
            stored_model.and_then(startup_role_alias)
        }
        _ => None,
    }
}

fn reasoning_effort<'a>(
    provider: &'a Provider,
    model: &str,
    role: Option<&str>,
) -> Option<&'a str> {
    role.or_else(|| {
        role_with_effort(model, |role| mapped_role_model(provider, role)).map(|index| ROLES[index])
    })
    .and_then(|role| {
        provider
            .effort_mapping
            .get(provider_mapping_role(provider, role))
    })
    .map(String::as_str)
    .or(provider.reasoning_effort.as_deref())
}

fn literal_role_index(model: &str) -> Option<usize> {
    let model = without_context_suffix(model);
    let role = model
        .strip_prefix("matou-api/")
        .or_else(|| startup_role_alias(model).filter(|role| *role == "fable"))?;
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

pub fn is_claude_model_environment(key: &OsStr) -> bool {
    #[cfg(windows)]
    let key = key.to_ascii_uppercase();
    let key = key.as_encoded_bytes();
    key.starts_with(b"ANTHROPIC_DEFAULT_")
        || matches!(
            key,
            b"ANTHROPIC_MODEL"
                | b"ANTHROPIC_SMALL_FAST_MODEL"
                | b"CLAUDE_CODE_GATEWAY_HINT_HEADERS"
                | b"CLAUDE_CODE_SUBAGENT_MODEL_FORCE"
        )
        || (key.starts_with(b"CLAUDE_CODE_")
            && key
                .windows(b"SUBAGENT".len())
                .position(|part| part == b"SUBAGENT")
                .is_some_and(|start| {
                    let suffix = &key[start + b"SUBAGENT".len()..];
                    [b"MODEL".as_slice(), b"MODEL_FORCE", b"MODEL_OVERRIDE"]
                        .into_iter()
                        .any(|ending| suffix.ends_with(ending))
                }))
}

fn clear_role_model_environment(environment: &mut BTreeMap<OsString, OsString>) {
    environment.retain(|key, _| !is_claude_model_environment(key));
}

fn clear_assigned_environment(provider: &Provider, environment: &mut BTreeMap<OsString, OsString>) {
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

pub fn role_model_env_key(role: &str) -> String {
    format!("ANTHROPIC_DEFAULT_{}_MODEL", role.to_ascii_uppercase())
}

fn project_role_model(environment: &mut BTreeMap<OsString, OsString>, role: &str, model: &str) {
    let key = role_model_env_key(role);
    set_var(environment, format!("{key}_NAME").into(), model.into());
    set_var(environment, key.into(), model.into());
}

fn project_assigned_role(
    provider: &Provider,
    role: &str,
    model: &str,
    environment: &mut BTreeMap<OsString, OsString>,
) {
    project_role_model(environment, role, model);
    if role == "opus" && mapped_role_model(provider, "fable").is_none() {
        project_role_model(environment, "fable", model);
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
    for role in ROLES {
        if let Some(model) = effective_role_model(provider, role) {
            project_role_model(environment, role, model);
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
        && let Some(path) = login_shell_path(
            environment,
            get_var(environment, "SHELL").unwrap_or(OsStr::new("/bin/zsh")),
            std::time::Duration::from_secs(10),
        ) {
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

/// Resolve a CLI using the launch PATH first, then the user's login-shell PATH.
#[cfg(unix)]
pub fn resolve_command_with_login_path(
    command: &Path,
    cwd: &Path,
    environment: &mut BTreeMap<OsString, OsString>,
) -> io::Result<PathBuf> {
    static LOGIN_PATH: OnceLock<Option<String>> = OnceLock::new();
    resolve_command_with_cached_login_path(command, cwd, environment, &LOGIN_PATH)
}

#[cfg(unix)]
fn resolve_command_with_cached_login_path(
    command: &Path,
    cwd: &Path,
    environment: &mut BTreeMap<OsString, OsString>,
    cache: &OnceLock<Option<String>>,
) -> io::Result<PathBuf> {
    let resolved =
        search_command_with_file_filter(command, cwd, environment, fs::Metadata::is_file);
    if resolved.is_ok()
        || command.file_name() != Some(command.as_os_str())
        || get_var(environment, "MATOU_DISABLE_LOGIN_PATH_PROBE") == Some(OsStr::new("1"))
    {
        return resolved;
    }

    // get_or_init waits for an in-flight probe and remembers failure as well.
    let Some(path) = cache.get_or_init(|| {
        login_shell_path(
            environment,
            get_var(environment, "SHELL").unwrap_or(OsStr::new("/bin/zsh")),
            std::time::Duration::from_secs(10),
        )
        .filter(|path| {
            !path.contains('\0')
                && std::env::split_paths(path).any(|entry| !entry.as_os_str().is_empty())
        })
    }) else {
        return resolved;
    };
    set_var(environment, "PATH".into(), path.into());
    search_command_with_file_filter(command, cwd, environment, fs::Metadata::is_file).or(resolved)
}

pub(crate) fn search_command(
    command: &Path,
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> io::Result<PathBuf> {
    search_command_with_file_filter(command, cwd, environment, |_| true)
}

fn search_command_with_file_filter(
    command: &Path,
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
    matches_file: impl Fn(&fs::Metadata) -> bool,
) -> io::Result<PathBuf> {
    let mut failure = io::Error::from(io::ErrorKind::NotFound);
    let mut check = |path: PathBuf| {
        let result = fs::metadata(&path).and_then(|metadata| {
            if metadata.is_dir() {
                return Err(io::Error::from(io::ErrorKind::IsADirectory));
            }
            if !matches_file(&metadata) {
                return Err(io::Error::from(io::ErrorKind::NotFound));
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
pub(crate) fn login_shell_path(
    environment: &BTreeMap<OsString, OsString>,
    shell: &OsStr,
    timeout: std::time::Duration,
) -> Option<String> {
    use std::{
        io::Read,
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    const MARKER: &str = "__MATOU_LOGIN_PATH__=";
    const LIMIT: usize = 256 * 1024;
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
    let deadline = Instant::now() + timeout;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    mod login_path {
        use super::*;
        use std::{os::unix::fs::PermissionsExt, sync::Barrier, thread};

        fn fixture() -> Result<(tempfile::TempDir, BTreeMap<OsString, OsString>)> {
            let root = tempfile::tempdir()?;
            let shell = root.path().join("login-shell");
            fs::write(
                &shell,
                "#!/bin/sh\nprintf 'probe\\n' >> \"$HOME/probes\"\nprintf '__MATOU_LOGIN_PATH__=%s\\n' \"$LOGIN_PATH\"\nexit \"$PROBE_STATUS\"\n",
            )?;
            fs::set_permissions(shell.as_path(), fs::Permissions::from_mode(0o700))?;
            let executable = root.path().join("agent");
            fs::write(&executable, "#!/bin/sh\nexit 0\n")?;
            fs::set_permissions(executable, fs::Permissions::from_mode(0o700))?;
            let environment = BTreeMap::from([
                ("SHELL".into(), shell.into_os_string()),
                ("HOME".into(), root.path().as_os_str().into()),
                ("LOGIN_PATH".into(), root.path().as_os_str().into()),
                ("PROBE_STATUS".into(), "0".into()),
                ("PATH".into(), "missing-bin".into()),
            ]);
            Ok((root, environment))
        }

        #[test]
        fn concurrent_launches_probe_once_and_receive_the_login_path() -> Result<()> {
            let (root, environment) = fixture()?;
            let cache = OnceLock::new();
            let barrier = Barrier::new(4);
            let cwd = root.path();
            thread::scope(|scope| {
                for _ in 0..4 {
                    scope.spawn(|| {
                        let mut environment = environment.clone();
                        barrier.wait();
                        let executable = resolve_command_with_cached_login_path(
                            Path::new("agent"),
                            cwd,
                            &mut environment,
                            &cache,
                        )
                        .unwrap();
                        assert_eq!(executable, cwd.join("agent"));
                        assert_eq!(get_var(&environment, "PATH"), Some(cwd.as_os_str()));
                    });
                }
            });
            let mut next = environment.clone();
            assert_eq!(
                resolve_command_with_cached_login_path(Path::new("agent"), cwd, &mut next, &cache)?,
                cwd.join("agent")
            );
            assert_eq!(get_var(&next, "PATH"), Some(cwd.as_os_str()));
            assert_eq!(
                get_var(&environment, "PATH"),
                Some(OsStr::new("missing-bin"))
            );
            assert_eq!(fs::read_to_string(cwd.join("probes"))?, "probe\n");
            Ok(())
        }

        #[test]
        fn failed_or_empty_probe_is_cached_without_replacing_path() -> Result<()> {
            for (login_path, exit_status) in [("::", "0"), ("", "0"), ("/bin", "1")] {
                let (root, mut environment) = fixture()?;
                let cache = OnceLock::new();
                let cwd = root.path();
                set_var(&mut environment, "LOGIN_PATH".into(), login_path.into());
                set_var(&mut environment, "PROBE_STATUS".into(), exit_status.into());
                let command = Path::new("agent");
                assert!(
                    resolve_command_with_cached_login_path(command, cwd, &mut environment, &cache)
                        .is_err()
                );
                assert_eq!(
                    get_var(&environment, "PATH"),
                    Some(OsStr::new("missing-bin"))
                );
                set_var(
                    &mut environment,
                    "LOGIN_PATH".into(),
                    cwd.as_os_str().into(),
                );
                set_var(&mut environment, "PROBE_STATUS".into(), "0".into());
                assert!(
                    resolve_command_with_cached_login_path(command, cwd, &mut environment, &cache)
                        .is_err()
                );
                assert_eq!(
                    get_var(&environment, "PATH"),
                    Some(OsStr::new("missing-bin"))
                );
                assert_eq!(fs::read_to_string(cwd.join("probes"))?, "probe\n");
            }
            Ok(())
        }

        #[test]
        fn existing_path_explicit_paths_and_opt_out_do_not_probe() -> Result<()> {
            let (root, mut environment) = fixture()?;
            let cwd = root.path();
            let cache = OnceLock::new();
            assert!(
                resolve_command_with_cached_login_path(
                    Path::new("./missing/agent"),
                    cwd,
                    &mut environment,
                    &cache,
                )
                .is_err()
            );
            let command = Path::new("agent");
            set_var(&mut environment, "PATH".into(), ".".into());
            assert_eq!(
                resolve_command_with_cached_login_path(command, cwd, &mut environment, &cache)?,
                cwd.join(".").join(command)
            );
            assert_eq!(get_var(&environment, "PATH"), Some(OsStr::new(".")));
            set_var(&mut environment, "PATH".into(), "missing-bin".into());
            set_var(
                &mut environment,
                "MATOU_DISABLE_LOGIN_PATH_PROBE".into(),
                "1".into(),
            );
            assert!(
                resolve_command_with_cached_login_path(command, cwd, &mut environment, &cache)
                    .is_err()
            );
            assert_eq!(
                get_var(&environment, "PATH"),
                Some(OsStr::new("missing-bin"))
            );
            assert!(cache.get().is_none());
            assert!(!cwd.join("probes").exists());
            Ok(())
        }
    }

    #[test]
    fn gateway_refresh_rebuilds_roles_but_retains_the_source_connection() -> Result<()> {
        let config = json!({
            "version": 1,
            "activeProviderIds": {"claude-code": "source"},
            "providers": {"claude-code": [
                {"id": "source", "endpoint": "https://api.deepseek.com/anthropic",
                 "apiKey": "source-key", "model": "deepseek-flash",
                 "modelMapping": {"opus": "deepseek-flash"}},
                {"id": "gemini", "name": "Antigravity", "kind": "subscription",
                 "endpoint": "", "model": "gemini-pro",
                 "subscription": {"provider": "antigravity", "modelMetadata": [
                     {"id": "gemini-pro", "supportedEfforts": ["high"], "maxOutputTokens": 4096}
                 ]}}
            ]}
        });
        let source = serde_json::from_value(config["providers"]["claude-code"][0].clone())?;
        let refresh = ClaudeGatewayRefresh {
            config,
            source,
            startup_role: Some("opus"),
            exact_roles: false,
            official_environment: OfficialClaudeEnvironment::new(&BTreeMap::new()),
        };
        let initial = parse_claude_role_assignments(&json!({
            "opus": {"providerId": "source", "model": "deepseek-flash", "effort": "low"}
        }))?;
        let previous = refresh
            .refresh(&initial)
            .context("missing original projection")?;
        assert_eq!(previous.role_target(0).unwrap().model, "deepseek-flash");
        assert_eq!(previous.role_target(0).unwrap().effort.as_deref(), Some("low"));
        let assignments = parse_claude_role_assignments(&json!({
            "opus": {"providerId": "gemini", "model": "gemini-pro", "effort": "high"}
        }))?;
        let projection = refresh
            .refresh(&assignments)
            .context("missing refreshed projection")?;
        let opus = projection.role_target(0).context("missing Opus target")?;
        assert_eq!(opus.provider_id, "gemini");
        assert_eq!(opus.model, "matou-antigravity/gemini-pro");
        assert_eq!(opus.effort.as_deref(), Some("high"));
        assert_eq!(opus.supported_efforts, Some(vec!["high".into()]));
        assert_eq!(opus.max_output_tokens, Some(4096));
        assert_eq!(
            opus.connection.subscription.as_ref().unwrap().provider,
            "antigravity"
        );
        let old_source = previous.source.as_ref().unwrap();
        let source = projection.source.as_ref().unwrap();
        assert_eq!(source.endpoint, old_source.endpoint);
        assert_eq!(source.api_key, old_source.api_key);

        assert!(refresh.refresh(&ClaudeRoleAssignments::new()).is_none());
        let mut invalid = assignments;
        invalid.get_mut("opus").unwrap().effort = Some("low".into());
        assert!(refresh.refresh(&invalid).is_none());
        invalid.get_mut("opus").unwrap().provider_id = "deleted-provider".into();
        assert!(refresh.refresh(&invalid).is_none());
        Ok(())
    }

    fn effort_provider() -> Result<tempfile::TempDir> {
        let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let config = json!({
            "version": 1,
            "activeProviderIds": {"claude-code": "fixture"},
            "providers": {"claude-code": [{
                "id": "fixture",
                "endpoint": "https://example.invalid",
                "apiKey": "fixture-key",
                "model": "opus",
                "effortMapping": {"opus": "high", "sonnet": "follow"}
            }]}
        });
        fs::write(root.path().join("provider-config.json"), config.to_string())?;
        Ok(root)
    }

    #[test]
    fn restored_claude_argv_uses_the_stored_model_role_for_effort() -> Result<()> {
        let root = effort_provider()?;
        let environment = BTreeMap::from([(
            "MATOU_CLAUDE_COMMAND".into(),
            std::env::current_exe()?.into_os_string(),
        )]);
        let resume = ClaudeTarget::Resume("original");
        let restart = ClaudeTarget::PermissionRestart { remembered: true };
        for (target, model, assigned_role, saved_effort, expected_effort) in [
            (resume, "matou-role/opus", None, None, Some("high")),
            (
                resume,
                "matou-role/opus-fallback",
                Some("opus"),
                None,
                Some("high"),
            ),
            (
                resume,
                "matou-role/sonnet",
                Some("sonnet"),
                Some("xhigh"),
                Some("xhigh"),
            ),
            (resume, "matou-role/haiku", None, None, None),
            (resume, "matou-role/sonnet", None, None, None),
            (resume, "matou-role/opus", Some("opus"), Some(""), None),
            (restart, "matou-role/opus", None, None, Some("high")),
            (restart, "matou-role/opus", None, Some("follow"), None),
        ] {
            let binding = json!({
                "providerConfigId": "fixture", "model": model,
                "assignedRole": assigned_role, "effort": saved_effort,
            });
            let launch = prepare_managed_claude_launch(
                root.path(),
                root.path(),
                &binding,
                Some(target),
                environment.clone(),
                80,
                24,
            )?;
            let mut expected = vec!["--model", model];
            if let Some(effort) = expected_effort {
                expected.extend(["--effort", effort]);
            }
            if let ClaudeTarget::Resume(id) = target {
                expected.extend(["--resume", id]);
            }
            assert_eq!(launch.launch.argv, expected, "{binding}");
        }
        Ok(())
    }

    #[test]
    fn new_claude_argv_uses_assigned_effort() -> Result<()> {
        let root = effort_provider()?;
        let assignments = BTreeMap::from([(
            "opus".into(),
            ClaudeRoleAssignment {
                provider_id: "fixture".into(),
                model: "matou-role/opus".into(),
                effort: Some("xhigh".into()),
            },
        )]);
        let launch = prepare_new_claude_launch(
            root.path(),
            root.path(),
            &json!({"model": "matou-role/sonnet", "effort": "low"}),
            Some(&assignments),
            BTreeMap::from([(
                "MATOU_CLAUDE_COMMAND".into(),
                std::env::current_exe()?.into_os_string(),
            )]),
            80,
            24,
        )?;

        assert_eq!(
            launch.launch.argv,
            ["--model", "matou-role/opus", "--effort", "xhigh"]
        );
        Ok(())
    }
}
