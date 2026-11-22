//! Codex restoration keeps watching for the bound identity after its queue slot is released.
use std::{collections::BTreeMap, ffi::OsString, fs, io, path::Path, sync::LazyLock};

use anyhow::{Context, Result, ensure};
use matou_native_core::{provider_launch::EFFORTS, sessions::ProviderBinding};
use regex::Regex;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::codex_launch::{self, CodexLaunch};

pub(crate) const MISSING: &str = "provider session not found";

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").unwrap());

pub(crate) fn identity_mismatch() -> &'static str {
    if english() {
        crate::provider_resume::IDENTITY_MISMATCH
    } else {
        "AI 会话返回的上下文与待恢复会话不一致，请重试恢复"
    }
}

pub(crate) fn failure_title() -> &'static str {
    if english() {
        "Codex recovery failed"
    } else {
        "Codex 恢复失败"
    }
}

pub(crate) fn action_error(fresh: bool) -> String {
    match (english(), fresh) {
        (true, false) => "Only a Codex session whose restore failed needs a retry",
        (true, true) => "Only a Codex session whose restore failed can start a fresh conversation",
        (false, false) => "只有恢复失败的 Codex 会话需要重试",
        (false, true) => "只有恢复失败的 Codex 会话可新开对话",
    }
    .to_owned()
}

fn english() -> bool {
    std::env::var("MATOU_LOCALE").as_deref() == Ok("en")
}

pub(crate) struct Resume {
    pub binding_id: String,
    pub provider_session_id: String,
    pub identity: Uuid,
    pub permission_respawn: bool,
    recent: String,
}

impl Resume {
    pub fn from_binding(binding: &ProviderBinding) -> Result<Self> {
        let identity = Uuid::parse_str(&binding.provider_session_id)
            .ok()
            .filter(|_| binding.resumable)
            .context(MISSING)?;
        Ok(Self {
            binding_id: binding.id.clone(),
            provider_session_id: binding.provider_session_id.clone(),
            identity,
            permission_respawn: false,
            recent: String::new(),
        })
    }

    pub fn has_visible_output(&self) -> bool {
        ANSI.replace_all(&self.recent, "")
            .chars()
            .any(|ch| !ch.is_control() && !ch.is_whitespace())
    }

    pub fn ingest(&mut self, data: &str) -> Option<&'static str> {
        static FAILURE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"nosession.{0,96}found|session.{0,96}notfound|noconversation.{0,96}found|invalidsession|failedtoresume").unwrap()
        });
        self.recent.push_str(data);
        let keep = self
            .recent
            .ceil_char_boundary(self.recent.len().saturating_sub(8192));
        self.recent.drain(..keep);
        // Terminal wrapping and cursor moves can split both the error text and the ID.
        let plain = ANSI.replace_all(&self.recent, "");
        let output = plain
            .split(|ch: char| ch.is_control() || ch.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        let identity = self.identity.to_string();
        if output.contains(&format!(
            "thatsessionisrunninginthebackground({}).",
            &identity[..8]
        )) {
            return Some("provider session running in background");
        }
        let at = output.rfind(&identity)?;
        let start = output.floor_char_boundary(at.saturating_sub(320));
        let end = output.ceil_char_boundary((at + identity.len() + 320).min(output.len()));
        FAILURE.is_match(&output[start..end]).then_some(MISSING)
    }
}

pub(crate) fn prepare(
    data_root: &Path,
    cwd: &Path,
    binding: Option<&ProviderBinding>,
    fresh: bool,
    environment: BTreeMap<OsString, OsString>,
    cols: u16,
    rows: u16,
) -> Result<(CodexLaunch, Option<Resume>)> {
    let resume = if fresh {
        None
    } else {
        Some(Resume::from_binding(binding.context(MISSING)?)?)
    };
    let inherited_api = ["OPENAI_API_KEY", "OPENAI_BASE_URL"].map(|key| {
        let key = OsString::from(key);
        let value = environment.get(&key).cloned();
        (key, value)
    });
    let mut plan = codex_launch::prepare(
        data_root,
        cwd,
        binding.and_then(|binding| binding.metadata.get("permissionMode")),
        environment,
        cols,
        rows,
    )?;
    if let Some(binding) = binding {
        if let Some(id) = binding.metadata["providerConfigId"]
            .as_str()
            .filter(|id| !id.trim().is_empty() && *id != plan.provider_config_id)
        {
            let provider = bound_provider(data_root, id)?;
            ensure!(
                provider["kind"] != "subscription",
                "Subscription launch service is unavailable"
            );
            let field = |key: &str| provider[key].as_str().unwrap_or_default().trim();
            plan.provider_config_id = id.to_owned();
            replace_model(&mut plan, field("model"));
            let argv = &mut plan.launch.argv;
            if let Some(index) = argv
                .windows(2)
                .position(|args| args[0] == "-c" && args[1].starts_with("model_reasoning_effort="))
            {
                argv.drain(index..index + 2);
            }
            let effort = field("reasoningEffort");
            if effort != "follow" && EFFORTS.contains(&effort) {
                argv.splice(
                    2..2,
                    [
                        "-c".into(),
                        format!("model_reasoning_effort={}", json!(effort)),
                    ],
                );
            }
            for ((key, inherited), value) in inherited_api
                .into_iter()
                .zip([field("apiKey"), field("endpoint").trim_end_matches('/')])
            {
                if !value.is_empty() {
                    plan.launch.env.insert(key, value.into());
                } else if let Some(inherited) = inherited {
                    plan.launch.env.insert(key, inherited);
                } else {
                    plan.launch.env.remove(&key);
                }
            }
        }
        if let Some(model) = binding.metadata["model"].as_str() {
            replace_model(&mut plan, model.trim());
        }
    }
    Ok((plan, resume))
}

fn bound_provider(data_root: &Path, id: &str) -> Result<Value> {
    let state: Value = match fs::read(data_root.join("provider-config.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid provider configuration JSON"))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(error).context("Read provider configuration"),
    };
    state["providers"]["codex"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|provider| provider["id"] == id)
        .cloned()
        .with_context(|| {
            if english() {
                "The provider configuration bound to this session does not exist"
            } else {
                "会话绑定的供应商配置不存在"
            }
        })
}

fn replace_model(plan: &mut CodexLaunch, model: &str) {
    if plan.model == model {
        return;
    }
    let argv = &mut plan.launch.argv;
    if let Some(index) = argv.iter().position(|arg| arg == "--model") {
        argv.drain(index..index + 2);
    }
    if !model.is_empty() {
        let index = argv
            .iter()
            .position(|arg| {
                matches!(
                    arg.as_str(),
                    "--ask-for-approval" | "--dangerously-bypass-approvals-and-sandbox"
                )
            })
            .unwrap_or(argv.len());
        argv.splice(index..index, ["--model".into(), model.into()]);
    }
    plan.model = model.to_owned();
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ID: &str = "01a1ddc1-88fd-7fc1-8c66-ab8d38719da8";

    fn resume() -> Resume {
        Resume::from_binding(&ProviderBinding {
            id: "binding".into(),
            provider_session_id: SESSION_ID.to_ascii_uppercase(),
            metadata: Value::Null,
            resumable: true,
            restore_state: "restoring".into(),
        })
        .unwrap()
    }

    #[test]
    fn missing_session_is_recognized_with_a_wrapped_uuid() {
        for separator in ["\n", "\x1b[2G"] {
            for at in 1..SESSION_ID.len() {
                let (left, right) = SESSION_ID.split_at(at);
                let mut resume = resume();
                assert_eq!(
                    resume.ingest(&format!(
                        "No conver{separator}sation found with session ID: {left}"
                    )),
                    None
                );
                assert_eq!(
                    resume.ingest(&format!("{separator}{right}")),
                    Some(MISSING),
                    "UUID split at {at} by {separator:?}"
                );
            }
        }
    }

    #[test]
    fn background_session_is_recognized_with_wrapped_output() {
        let message = "That session is running in the background (01a1ddc1).";
        for separator in ["\n", "\x1b[2G"] {
            for at in 1..message.len() {
                let (left, right) = message.split_at(at);
                assert_eq!(
                    resume().ingest(&format!("{left}{separator}{right}")),
                    Some("provider session running in background"),
                    "Background message split at {at} by {separator:?}"
                );
            }
        }
    }

    #[test]
    fn startup_failures_require_a_matching_identity_and_nearby_reason() {
        for output in [
            format!("Resuming session {SESSION_ID}"),
            "No conversation found".into(),
            "No conver\nsation found with session ID: 01a1ddc1-88fd-7fc1-8c66-\x1b[2Gab8d38719da9"
                .into(),
            "That session is running in the back\nground (01a1\x1b[2Gddc2).".into(),
            "That session is running in the back\nground (01a1\x1b[2Gddc1ff).".into(),
            format!("Session not found {} {SESSION_ID}", "界".repeat(330)),
        ] {
            assert_eq!(resume().ingest(&output), None, "{output:?}");
        }
    }
}
