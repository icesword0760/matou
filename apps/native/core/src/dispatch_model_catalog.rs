//! Local dispatch model discovery, with independent last-known catalogs per provider.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read},
    path::Path,
    process::{Command, Stdio},
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    dispatch_roles::{
        AGY_EFFORTS, CODEX_EFFORTS, CODEX_MODELS, DSH_EFFORTS, agy_effort_suffix, effort_is_fixed,
        write_dispatch_json,
    },
    provider_launch::search_command,
    pty::{get_var, set_var},
};

pub const CATALOG_FILENAME: &str = "dispatch-model-catalog.json";
const CODEX_DYNAMIC: &str = "系列动态解析（自动取本机最新）";
const CODEX_INSTALLED: &str = "本机已安装具体型号";
const AGY_ORCHESTRATORS: &str = "Orchestrator 编排器（可自派子代理 · 无独立推理档位）";
const AGY_DYNAMIC: &str = "Gemini 系列动态解析（随档位自动取最新）";
const AGY_FIXED: &str = "跨生态与固定型号";
const AGY_SERIES: [&str; 5] = ["neon", "carbon", "argon", "hydrogen", "flash"];
const CODEX_SERIES: [&str; 4] = ["astra", "sol", "terra", "luna"];
const CLI_TIMEOUT: Duration = Duration::from_secs(6);
static PROBE: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchModel {
    pub group: String,
    pub value: String,
    pub label: String,
    pub efforts: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    resolved_models: BTreeMap<String, String>,
}

impl DispatchModel {
    pub fn label_for_effort(&self, effort: &str) -> Cow<'_, str> {
        if let Some(model) = self.resolved_models.get(effort) {
            Cow::Owned(format!("{}（当前 {model}）", self.label))
        } else {
            Cow::Borrowed(&self.label)
        }
    }
}

fn model(group: &str, value: &str, label: &str, efforts: &[&str]) -> DispatchModel {
    DispatchModel {
        resolved_models: BTreeMap::new(),
        group: group.into(),
        value: value.into(),
        label: label.into(),
        efforts: efforts.iter().map(|effort| (*effort).into()).collect(),
    }
}

const CODEX_ALIASES: [&str; 5] = [
    "主力系列最新",
    "跨系列最高",
    "前沿系列最新",
    "均衡系列最新",
    "快速系列最新",
];

fn codex_alias(index: usize) -> DispatchModel {
    let value = CODEX_MODELS[index];
    model(
        CODEX_DYNAMIC,
        value,
        &format!("{value} · {}", CODEX_ALIASES[index]),
        &CODEX_EFFORTS,
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchModelCatalog {
    pub refreshed_at: u64,
    codex: Vec<DispatchModel>,
    agy: Vec<DispatchModel>,
    #[serde(skip, default = "dsh_models")]
    dsh: Vec<DispatchModel>,
}

fn dsh_models() -> Vec<DispatchModel> {
    [
        (
            "deepseek-flash",
            "DeepSeek-V4.1-Flash（deepseek-flash · 默认）",
        ),
        ("deepseek-v4-pro", "DeepSeek-V4-Pro（deepseek-v4-pro）"),
    ]
    .into_iter()
    .map(|(id, label)| model("", id, label, &DSH_EFFORTS))
    .collect()
}

fn orchestrator_label(id: &str) -> &str {
    match id {
        "gemini-pro-orchestrator" => "Orchestrator (Carbon) · 默认",
        "gemini-pro-orchestrator-neon" => "Orchestrator (Neon)",
        "gemini-pro-orchestrator-argon" => "Orchestrator (Argon)",
        _ => id,
    }
}

fn agy_label(id: &str) -> &str {
    match id {
        "claude-opus-5-5" => "Claude Opus 5.5",
        "claude-sonnet-5-5" => "Claude Sonnet 5.5",
        "gpt-oss-120b-medium" => "GPT-OSS 120B",
        _ => id,
    }
}

impl Default for DispatchModelCatalog {
    fn default() -> Self {
        let mut codex = (0..CODEX_MODELS.len()).map(codex_alias).collect::<Vec<_>>();
        codex.extend(
            [
                "gpt-6.2-astra",
                "gpt-6-astra",
                "gpt-6.2-sol",
                "gpt-6.1-sol",
                "gpt-6-sol",
                "gpt-5.6-sol",
                "gpt-6.1-terra",
                "gpt-5.6-terra",
                "gpt-6-luna",
                "gpt-5.6-luna",
            ]
            .into_iter()
            .map(|id| model(CODEX_INSTALLED, id, id, &CODEX_EFFORTS)),
        );

        let mut agy = [
            "gemini-pro-orchestrator",
            "gemini-pro-orchestrator-neon",
            "gemini-pro-orchestrator-argon",
        ]
        .into_iter()
        .map(|id| model(AGY_ORCHESTRATORS, id, orchestrator_label(id), &[]))
        .collect::<Vec<_>>();
        agy.extend(AGY_SERIES.into_iter().map(|id| {
            let efforts = if ["neon", "hydrogen"].contains(&id) {
                &AGY_EFFORTS[..]
            } else {
                &AGY_EFFORTS[..3]
            };
            model(AGY_DYNAMIC, id, &format!("{id} · 系列最新"), efforts)
        }));
        agy.extend(
            [
                ("claude-opus-5-5", &AGY_EFFORTS[..3]),
                ("claude-sonnet-5-5", &AGY_EFFORTS[..3]),
                ("gpt-oss-120b-medium", &["medium"][..]),
            ]
            .into_iter()
            .map(|(id, efforts)| model(AGY_FIXED, id, agy_label(id), efforts)),
        );

        Self {
            refreshed_at: 0,
            codex,
            agy,
            dsh: dsh_models(),
        }
    }
}

impl DispatchModelCatalog {
    pub fn load(data_root: &Path) -> Self {
        fs::read(data_root.join(CATALOG_FILENAME))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn models(&self, agent: &str) -> &[DispatchModel] {
        match agent {
            "codex" => &self.codex,
            "dsh" => &self.dsh,
            "agy" => &self.agy,
            _ => &[],
        }
    }

    pub fn is_stale(&self) -> bool {
        epoch_seconds().saturating_sub(self.refreshed_at) >= 24 * 60 * 60
    }
}

#[derive(Debug)]
pub struct DispatchModelRefresh {
    pub catalog: DispatchModelCatalog,
    pub codex_updated: bool,
    pub agy_updated: bool,
    pub cache_error: Option<io::Error>,
}

/// Call on a background worker. Each provider has its own timeout and fallback.
pub fn probe_local_cli_models(data_root: &Path) -> DispatchModelRefresh {
    // Prevent overlapping windows from overwriting each other's partial results.
    let _probe = PROBE.lock().unwrap_or_else(|error| error.into_inner());
    let environment = probe_environment();
    let cwd = std::env::current_dir().unwrap_or_else(|_| data_root.to_path_buf());
    let (codex, agy) = thread::scope(|scope| {
        let codex = scope.spawn(|| {
            run_cli(
                "CODEX_BIN",
                "codex",
                &["debug", "models"],
                &cwd,
                &environment,
            )
            .ok()
            .and_then(|out| parse_codex(&out))
        });
        let agy = scope.spawn(|| {
            run_cli("AGY_BIN", "agy", &["models"], &cwd, &environment)
                .ok()
                .and_then(|out| parse_agy(&out))
        });
        (codex.join().ok().flatten(), agy.join().ok().flatten())
    });
    merge_probe(data_root, codex, agy)
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn merge_probe(
    data_root: &Path,
    codex: Option<Vec<DispatchModel>>,
    agy: Option<Vec<DispatchModel>>,
) -> DispatchModelRefresh {
    let mut catalog = DispatchModelCatalog::load(data_root);
    let codex_updated = codex.is_some();
    let agy_updated = agy.is_some();
    if let Some(codex) = codex {
        catalog.codex = codex;
    }
    if let Some(agy) = agy {
        catalog.agy = agy;
    }
    let cache_error = if codex_updated || agy_updated {
        catalog.refreshed_at = epoch_seconds();
        write_dispatch_json(&data_root.join(CATALOG_FILENAME), &catalog).err()
    } else {
        None
    };
    DispatchModelRefresh {
        catalog,
        codex_updated,
        agy_updated,
        cache_error,
    }
}

fn probe_environment() -> BTreeMap<OsString, OsString> {
    let environment = std::env::vars_os().collect();
    #[cfg(unix)]
    {
        let mut environment = environment;
        let entries = get_var(&environment, "PATH")
            .into_iter()
            .flat_map(std::env::split_paths)
            .collect::<Vec<_>>();
        let has_local = entries.iter().any(|path| path.ends_with(".local/bin"));
        let has_nvm = entries
            .iter()
            .any(|path| path.components().any(|part| part.as_os_str() == ".nvm"));
        if !has_local || !has_nvm {
            add_login_path(&mut environment);
        }
        environment
    }
    #[cfg(not(unix))]
    environment
}

#[cfg(unix)]
fn add_login_path(environment: &mut BTreeMap<OsString, OsString>) {
    use crate::provider_launch::login_shell_path;
    if get_var(environment, "MATOU_DISABLE_LOGIN_PATH_PROBE") == Some(OsStr::new("1")) {
        return;
    }
    if let Some(login) = login_shell_path(
        environment,
        OsStr::new("/bin/zsh"),
        Duration::from_millis(1500),
    ) {
        let mut seen = BTreeSet::new();
        let paths = std::env::split_paths(&login)
            .chain(
                get_var(environment, "PATH")
                    .into_iter()
                    .flat_map(std::env::split_paths),
            )
            .filter(|path| !path.as_os_str().is_empty() && seen.insert(path.clone()));
        if let Ok(path) = std::env::join_paths(paths) {
            set_var(environment, "PATH".into(), path);
        }
    }
}

fn run_cli(
    env_key: &str,
    fallback: &str,
    args: &[&str],
    cwd: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> io::Result<String> {
    let command = get_var(environment, env_key)
        .filter(|value| !value.is_empty())
        .unwrap_or(OsStr::new(fallback));
    let executable = search_command(Path::new(command), cwd, environment)?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("piped CLI output");
    let (send, receive) = mpsc::sync_channel(1);
    const LIMIT: u64 = 32 * 1024 * 1024;
    thread::spawn(move || {
        let mut output = String::new();
        let result = stdout
            .take(LIMIT + 1)
            .read_to_string(&mut output)
            .and_then(|read| {
                if read as u64 > LIMIT {
                    Err(io::Error::other("CLI model catalog is too large"))
                } else {
                    Ok(output)
                }
            });
        let _ = send.send(result);
    });

    let deadline = Instant::now() + CLI_TIMEOUT;
    // Read concurrently so a catalog larger than a pipe buffer cannot block the CLI.
    let output = receive.recv_timeout(CLI_TIMEOUT);
    let result = match output {
        Ok(Ok(output)) => loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(output),
                Ok(Some(status)) => {
                    return Err(io::Error::other(format!("{fallback} exited with {status}")));
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(15)),
                Ok(None) => break Err(io::Error::new(io::ErrorKind::TimedOut, fallback)),
                Err(error) => break Err(error),
            }
        },
        Ok(Err(error)) => Err(error),
        Err(error) => Err(io::Error::new(io::ErrorKind::TimedOut, error)),
    };
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL)
    };
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn version_and_series(slug: &str) -> Option<(Vec<u32>, &str)> {
    let (version, series) = slug.split_once('-')?;
    let mut version = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    while version.len() > 1 && version.last() == Some(&0) {
        version.pop();
    }
    (!series.is_empty()).then_some((version, series))
}

fn parse_codex(output: &str) -> Option<Vec<DispatchModel>> {
    let document: Value = serde_json::from_str(output).ok()?;
    let mut models = document["models"]
        .as_array()?
        .iter()
        .filter(|entry| entry["visibility"] == "list" && entry["supported_in_api"] == true)
        .filter_map(|entry| {
            let slug = entry["slug"].as_str()?;
            let efforts = entry["supported_reasoning_levels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|level| level.as_str().or_else(|| level["effort"].as_str()))
                .collect::<Vec<_>>();
            let (version, series) = slug
                .strip_prefix("gpt-")
                .and_then(version_and_series)
                .unwrap_or_default();
            let rank = CODEX_SERIES
                .iter()
                .position(|item| *item == series)
                .unwrap_or(CODEX_SERIES.len());
            let upgrade = entry["upgrade"]
                .as_str()
                .or_else(|| entry["upgrade"]["model"].as_str());
            Some((
                rank,
                version,
                series,
                upgrade,
                model(CODEX_INSTALLED, slug, slug, &efforts),
            ))
        })
        .collect::<Vec<_>>();
    if models.is_empty() {
        return None;
    }
    models.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.4.value.cmp(&b.4.value))
    });

    let mut aliases = Vec::new();
    for (index, keyword) in CODEX_MODELS.into_iter().enumerate() {
        let mut alias = codex_alias(index);
        let matches = |entry: &&(_, _, &str, _, DispatchModel)| {
            entry.0 < CODEX_SERIES.len() && (keyword == "best" || entry.2 == keyword)
        };
        for effort in &alias.efforts {
            let candidates = || {
                models
                    .iter()
                    .filter(matches)
                    .filter(|entry| entry.4.efforts.contains(effort))
            };
            if let Some(top) = candidates().next() {
                let selected = candidates()
                    .find(|entry| Some(entry.4.value.as_str()) == top.3)
                    .unwrap_or(top);
                alias
                    .resolved_models
                    .insert(effort.clone(), selected.4.value.clone());
            }
        }
        alias
            .efforts
            .retain(|effort| alias.resolved_models.contains_key(effort));
        aliases.push(alias);
    }
    aliases.extend(models.into_iter().map(|(_, _, _, _, entry)| entry));
    Some(aliases)
}

fn parse_agy(output: &str) -> Option<Vec<DispatchModel>> {
    let mut orchestrators = Vec::new();
    let mut gemini = BTreeMap::<&str, BTreeSet<&str>>::new();
    let mut families = BTreeMap::<&str, Vec<(&str, &str, &str)>>::new();
    let mut fixed = Vec::new();
    let mut seen = BTreeSet::new();
    for line in output.lines() {
        // Accept standalone IDs, tabular output, and list display labels.
        // Validate the whole leading token so progress and errors stay out.
        let line = line.trim().trim_start_matches(['-', '*', '•']).trim_start();
        let (slug, label) = line
            .split_once('\t')
            .or_else(|| line.split_once("  "))
            .or_else(|| line.split_once(" - "))
            .unwrap_or((line, ""));
        let slug = slug.trim_end();
        if !slug.contains('-')
            || !slug.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            || !slug.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'-' | b'_')
            })
            || !seen.insert(slug)
        {
            continue;
        }
        let label = label.trim();
        let label = if label.is_empty() {
            agy_label(slug)
        } else {
            label
        };
        if effort_is_fixed("agy", slug) {
            let known = orchestrator_label(slug);
            orchestrators.push(model(
                AGY_ORCHESTRATORS,
                slug,
                if known == slug { label } else { known },
                &[],
            ));
        } else if let Some(effort) = agy_effort_suffix(slug) {
            let base = &slug[..slug.len() - effort.len() - 1];
            if let Some((_, series)) = base.strip_prefix("gemini-").and_then(version_and_series) {
                gemini.entry(series).or_default().insert(effort);
            } else {
                families
                    .entry(base)
                    .or_default()
                    .push((slug, label, effort));
            }
        } else {
            fixed.push(model(AGY_FIXED, slug, label, &[]));
        }
    }
    for (base, variants) in families {
        if let [(slug, label, effort)] = variants.as_slice() {
            fixed.push(model(AGY_FIXED, slug, label, &[*effort]));
        } else {
            let efforts = AGY_EFFORTS
                .into_iter()
                .filter(|effort| variants.iter().any(|entry| entry.2 == *effort))
                .collect::<Vec<_>>();
            fixed.push(model(AGY_FIXED, base, agy_label(base), &efforts));
        }
    }
    for (series, efforts) in gemini {
        let efforts = AGY_EFFORTS
            .into_iter()
            .filter(|effort| efforts.contains(effort))
            .collect::<Vec<_>>();
        orchestrators.push(model(
            AGY_DYNAMIC,
            series,
            &format!("{series} · 系列最新"),
            &efforts,
        ));
    }
    orchestrators.extend(fixed);
    (!orchestrators.is_empty()).then_some(orchestrators)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry<'a>(models: &'a [DispatchModel], id: &str) -> &'a DispatchModel {
        models.iter().find(|model| model.value == id).expect(id)
    }

    #[test]
    fn builtin_catalog_contains_all_twenty_eight_models() {
        let catalog = DispatchModelCatalog::default();
        assert_eq!(catalog.models("codex").len(), 15);
        assert_eq!(catalog.models("dsh").len(), 2);
        assert_eq!(catalog.models("agy").len(), 11);
        assert_eq!(entry(catalog.models("agy"), "neon").efforts, AGY_EFFORTS);
    }

    #[test]
    fn codex_filters_eligibility_and_resolves_tier_version_and_upgrade() {
        let models = parse_codex(r#"{"models":[
          {"slug":"gpt-6.10-sol","visibility":"list","supported_in_api":true,"upgrade":{"model":"gpt-6.2-sol"},"supported_reasoning_levels":[{"effort":"high"}]},
          {"slug":"gpt-6.2-sol","visibility":"list","supported_in_api":true,"supported_reasoning_levels":["low","high"]},
          {"slug":"gpt-6.1-sol","visibility":"list","supported_in_api":true,"supported_reasoning_levels":["low"]},
          {"slug":"gpt-6.1-astra","visibility":"list","supported_in_api":true,"supported_reasoning_levels":["high","xhigh"]},
          {"slug":"gpt-100-sol","visibility":"hidden","supported_in_api":true,"supported_reasoning_levels":["medium"]},
          {"slug":"gpt-100-astra","visibility":"list","supported_in_api":false,"supported_reasoning_levels":["high"]}
        ]}"#).unwrap();
        assert_eq!(models.len(), 9);
        assert!(
            entry(&models, "sol")
                .label_for_effort("high")
                .ends_with("gpt-6.2-sol）")
        );
        assert!(
            entry(&models, "best")
                .label_for_effort("high")
                .ends_with("gpt-6.1-astra）")
        );
        assert!(
            entry(&models, "best")
                .label_for_effort("low")
                .ends_with("gpt-6.2-sol）")
        );
        assert_eq!(entry(&models, "sol").efforts, ["low", "high"]);
        assert_eq!(entry(&models, "gpt-6.1-sol").efforts, ["low"]);
    }

    #[test]
    fn agy_unions_efforts_across_versions_and_keeps_single_variants_fixed() {
        let models = parse_agy(
            "Fetching available models...\ngemini-pro-orchestrator-neon\n\
             gemini-4-neon-high\tGemini Neon\ngemini-3.8-neon-xhigh\n\
             gemini-4-carbon-low  Gemini Carbon\nclaude-opus-5-5-high\nclaude-opus-5-5-medium\n\
             gpt-oss-120b-medium\nError: This is not a model\n",
        )
        .unwrap();
        assert_eq!(models.len(), 5);
        assert_eq!(entry(&models, "neon").efforts, ["high", "xhigh"]);
        assert_eq!(
            entry(&models, "claude-opus-5-5").efforts,
            ["medium", "high"]
        );
        assert_eq!(entry(&models, "gpt-oss-120b-medium").efforts, ["medium"]);
        assert!(
            entry(&models, "gemini-pro-orchestrator-neon")
                .efforts
                .is_empty()
        );
        assert!(parse_agy("Fetching available models...\nPlease sign in first").is_none());
    }

    #[test]
    fn partial_probe_replaces_only_the_successful_provider() {
        let root = tempfile::tempdir().unwrap();
        let agy = parse_agy("gemini-4-neon-high\n").unwrap();
        let first = merge_probe(root.path(), None, Some(agy.clone()));
        assert_eq!(first.catalog.models("codex").len(), 15);
        let codex = vec![model(
            CODEX_INSTALLED,
            "fixture-model",
            "Fixture",
            &["high"],
        )];
        let second = merge_probe(root.path(), Some(codex.clone()), None);
        assert!(second.codex_updated && !second.agy_updated);
        assert!(second.cache_error.is_none());
        let cached = DispatchModelCatalog::load(root.path());
        assert_eq!(cached.models("agy"), agy);
        assert_eq!(cached.models("codex"), codex);
        assert!(!cached.is_stale());
        assert_eq!(cached.models("dsh").len(), 2);
    }
}
