//! Prepare a Claude Code run from the frozen runtime's V1 settings.
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::pty::{
    CALLER_SESSION_MARKERS, LaunchProfile, ShellLaunch, get_var, remove_var, set_var,
};

const OFFICIAL_ID: &str = "anthropic-official";
const OFFICIAL_ENDPOINT: &str = "https://api.anthropic.com";
const CLI_DEFAULT_MODEL: &str = "CLI 默认";
const ROLES: [&str; 3] = ["opus", "sonnet", "haiku"];
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

pub struct ClaudeLaunch {
    pub launch: ShellLaunch,
    pub provider_config_id: String,
    pub model: String,
    pub permission_mode: String,
    pub assigned_role: Option<&'static str>,
    pub effort: Option<String>,
    pub clear_assignment_environment: bool,
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
        Some(ClaudeTarget::New(assignments)),
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
    New(Option<&'a ClaudeRoleAssignments>),
    PermissionRestart { remembered: bool },
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
        Some(ClaudeTarget::New(assignments)) => (&Value::Null, assignments),
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
    let clear_assignment_environment = assigned_role.is_some()
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
        // Cross-role switching and per-role subagent routing need the request
        // gateway. Native only aligns the role selected for this launch.
        project_assigned_role(provider, role, model, &mut environment);
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
    if let Some(effort) = assignment
        .effort
        .as_deref()
        .filter(|effort| *effort != "follow")
        && let Some(levels) = model
            .get("supportedEfforts")
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
    let model = without_context_suffix(&source.model);
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
    let model = if model.is_empty() { "default" } else { model };
    let mapped = ROLES.into_iter().find(|role| {
        provider
            .model_mapping
            .get(*role)
            .is_some_and(|mapped| without_context_suffix(mapped) == without_context_suffix(model))
    });
    let lower_model = model.to_ascii_lowercase();
    let inferred = || {
        ROLES.into_iter().find(|role| {
            ["matou-", "claude-"].into_iter().any(|prefix| {
                let pattern = format!("{prefix}{role}");
                lower_model.match_indices(&pattern).any(|(index, _)| {
                    let suffix = &lower_model[index + pattern.len()..];
                    suffix.is_empty() || suffix.starts_with(['-', '/', '['])
                })
            })
        })
    };
    mapped
        .or_else(inferred)
        .and_then(|role| provider.effort_mapping.get(role))
        .map(String::as_str)
        .or(provider.reasoning_effort.as_deref())
}

fn clear_assigned_environment(provider: &Provider, environment: &mut BTreeMap<OsString, OsString>) {
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

fn resolve_claude_executable(
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
