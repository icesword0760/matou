use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use matou_native_core::{
    claude_role_budget::CLAUDE_ROLES, provider_launch::resolve_claude_executable,
};
use serde_json::{Value, json};

use crate::protocol::{ClaudeCatalogModel, write_message};

const PROBE_TIMEOUT: Duration = Duration::from_secs(6);
const CACHE_TTL: Duration = Duration::from_secs(60);
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

struct CachedCatalog {
    started: Instant,
    models: Vec<ClaudeCatalogModel>,
}

impl CachedCatalog {
    fn is_fresh_at(&self, now: Instant) -> bool {
        now.duration_since(self.started) < CACHE_TTL
    }
}

static CACHE: Mutex<Option<CachedCatalog>> = Mutex::new(None);

pub(crate) fn models() -> Vec<ClaudeCatalogModel> {
    // Hold the cache lock across the probe so separate hosts share the in-flight result.
    let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
    let now = Instant::now();
    if let Some(cached) = cache.as_ref().filter(|cached| cached.is_fresh_at(now)) {
        return cached.models.clone();
    }
    let models = probe().unwrap_or_default();
    *cache = Some(CachedCatalog {
        started: now,
        models: models.clone(),
    });
    models
}

fn probe() -> Result<Vec<ClaudeCatalogModel>> {
    let root = tempfile::Builder::new()
        .prefix("matou-model-catalog-")
        .tempdir()?;
    let mut environment = std::env::vars_os().collect();
    let executable = resolve_claude_executable(&std::env::current_dir()?, &mut environment)?;
    query_cli(&executable, root.path(), environment)
}

fn query_cli(
    executable: &Path,
    root: &Path,
    mut environment: BTreeMap<OsString, OsString>,
) -> Result<Vec<ClaudeCatalogModel>> {
    environment.retain(|key, _| {
        let name = key.to_string_lossy();
        #[cfg(windows)]
        let name = name.to_ascii_uppercase();
        !["ANTHROPIC_", "CLAUDE_", "MATOU_"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
    });
    let mut command = Command::new(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--setting-sources",
            "user",
            "--strict-mcp-config",
            "--mcp-config",
            r#"{"mcpServers":{}}"#,
            "--tools",
            "",
        ])
        .current_dir(root)
        .env_clear()
        .envs(environment)
        .env("CLAUDE_CONFIG_DIR", root)
        .env("ANTHROPIC_API_KEY", "LOCAL_METADATA_ONLY")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let result = (|| -> Result<_> {
        let stdin = child.stdin.take().context("missing CLI stdin")?;
        let stdout = child.stdout.take().context("missing CLI stdout")?;
        let (send, receive) = mpsc::channel();
        // Pipe I/O can block in either direction. The supervisor retains the child handle.
        thread::Builder::new()
            .name("claude-catalog-stream".into())
            .spawn(move || {
                let mut stream = ControlStream {
                    stdin,
                    stdout: BufReader::new(stdout),
                    serial: 0,
                };
                let result = collect_models(&mut stream);
                // Keep stdin open until the supervisor checks whether the CLI exited.
                let _ = send.send((result, stream));
            })?;
        let (result, _stream) =
            receive.recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
        if child.try_wait()?.is_some() {
            bail!("Claude CLI exited during model catalog probe");
        }
        result
    })();
    let _ = child.kill();
    child.wait()?;
    result
}

struct ControlStream<W, R> {
    stdin: W,
    stdout: R,
    serial: u32,
}

impl<W: Write, R: BufRead> ControlStream<W, R> {
    fn request(&mut self, request: Value) -> Result<Option<Value>> {
        self.serial += 1;
        let request_id = self.serial.to_string();
        write_message(
            &mut self.stdin,
            &json!({"type": "control_request", "request_id": request_id, "request": request}),
        )?;
        let mut line = Vec::new();
        while self.stdout.read_until(b'\n', &mut line)? != 0 {
            if let Ok(row) = serde_json::from_slice::<Value>(&line)
                && row["type"] == "control_response"
                && row["response"]["request_id"] == request_id
            {
                let response = &row["response"];
                if response["subtype"] != "success" {
                    return Ok(None);
                }
                return Ok(response
                    .get("response")
                    .filter(|value| !matches!(value, Value::Null | Value::Bool(false)))
                    .cloned());
            }
            line.clear();
        }
        bail!("Claude CLI closed model catalog stream");
    }
}

fn collect_models(
    stream: &mut ControlStream<impl Write, impl BufRead>,
) -> Result<Vec<ClaudeCatalogModel>> {
    let Some(init) = stream.request(json!({"subtype": "initialize"}))? else {
        return Ok(Vec::new());
    };
    let mut models = Vec::<ClaudeCatalogModel>::new();
    for entry in init["models"].as_array().into_iter().flatten().take(12) {
        let (Some(alias), Some(id)) = (entry["value"].as_str(), entry["resolvedModel"].as_str())
        else {
            continue;
        };
        if (alias != "default" && !CLAUDE_ROLES.contains(&alias)) || id.is_empty() {
            continue;
        }
        let Some(base) = context_usage(stream, alias)? else {
            continue;
        };
        if !same_model(&base, id) {
            continue;
        }
        let expanded = context_usage(stream, &format!("{alias}[1m]"))?;
        let model = ClaudeCatalogModel {
            id: id.to_owned(),
            resolved_model: None,
            display_name: entry["displayName"].as_str().map(str::to_owned),
            context_window: positive_tokens(&base["rawMaxTokens"]),
            max_context_window: expanded
                .filter(|usage| same_model(usage, id))
                .and_then(|usage| positive_tokens(&usage["rawMaxTokens"])),
            supported_efforts: entry["supportedEffortLevels"].as_array().map(|levels| {
                levels
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|level| EFFORTS.contains(level))
                    .map(str::to_owned)
                    .collect()
            }),
        };
        match models
            .iter_mut()
            .find(|row| row.resolved_model.is_none() && row.id == id)
        {
            Some(existing) => {
                if model.max_context_window.is_some() {
                    existing.max_context_window = model.max_context_window;
                }
            }
            None => models.push(model.clone()),
        }
        models.push(ClaudeCatalogModel {
            id: alias.to_owned(),
            resolved_model: Some(id.to_owned()),
            ..model
        });
    }
    Ok(models
        .iter()
        .cloned()
        .map(|mut model| {
            if let Some(id) = &model.resolved_model
                && let Some(canonical) = models
                    .iter()
                    .find(|row| row.resolved_model.is_none() && &row.id == id)
            {
                model.context_window = model.context_window.or(canonical.context_window);
                model.max_context_window =
                    model.max_context_window.or(canonical.max_context_window);
                model.supported_efforts = model
                    .supported_efforts
                    .or_else(|| canonical.supported_efforts.clone());
            }
            model
        })
        .collect())
}

fn context_usage(
    stream: &mut ControlStream<impl Write, impl BufRead>,
    model: &str,
) -> Result<Option<Value>> {
    if stream
        .request(json!({"subtype": "set_model", "model": model}))?
        .is_none()
    {
        return Ok(None);
    }
    stream.request(json!({"subtype": "get_context_usage", "detail": "summary"}))
}

fn same_model(usage: &Value, id: &str) -> bool {
    fn without_suffix(id: &str) -> &str {
        id.strip_suffix("[1m]")
            .or_else(|| id.strip_suffix("[1M]"))
            .unwrap_or(id)
    }
    usage["model"]
        .as_str()
        .is_some_and(|model| without_suffix(model) == without_suffix(id))
}

fn positive_tokens(value: &Value) -> Option<u64> {
    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
    value
        .as_f64()
        .filter(|&number| number > 0.0 && number <= MAX_SAFE_INTEGER && number.fract() == 0.0)
        .map(|number| number as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_model_catalog_cache_expires_at_sixty_seconds() {
        let started = Instant::now();
        let cached = CachedCatalog {
            started,
            models: Vec::new(),
        };
        assert!(cached.is_fresh_at(started + Duration::from_millis(59_999)));
        assert!(!cached.is_fresh_at(started + Duration::from_secs(60)));
    }
}
