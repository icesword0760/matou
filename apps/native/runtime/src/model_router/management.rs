use std::{net::Ipv4Addr, time::Duration};

use anyhow::{Context, Result, anyhow, ensure};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request, StatusCode, body::Bytes, client::conn::http1, header};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpStream;

use super::{RouterConnection, VERSION, config::codex_user_agent};
use crate::subscription_launch::{ModelAlias, SubscriptionProvider};

const DEFAULT_LIMIT: usize = 1024 * 1024;
const MODEL_LIMIT: usize = 4 * DEFAULT_LIMIT;
const TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct ManagementClient<'a> {
    connection: &'a RouterConnection,
}

impl<'a> ManagementClient<'a> {
    pub fn new(connection: &'a RouterConnection) -> Self {
        Self { connection }
    }

    pub fn ready(&self, timeout: Duration) -> Result<bool> {
        let (status, pinned, _) =
            self.exchange(Method::GET, "/credentials", None, timeout, DEFAULT_LIMIT)?;
        ensure!(status.is_success(), "Model router authentication failed");
        Ok(pinned)
    }

    pub fn credentials(&self) -> Result<Vec<Value>> {
        let body = self.json(Method::GET, "/credentials", None, DEFAULT_LIMIT)?;
        body["files"]
            .as_array()
            .cloned()
            .context("Invalid model router credential list")
    }

    pub fn models(&self, name: &str) -> Result<Vec<Value>> {
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("name", name)
            .finish();
        self.model_list(&format!("/credentials/models?{query}"))
    }

    pub fn model_definitions(&self, provider: SubscriptionProvider) -> Result<Vec<Value>> {
        self.model_list(&format!(
            "/routing/model-definitions/{}",
            provider.router_provider()
        ))
    }

    pub fn start_oauth(&self, provider: SubscriptionProvider) -> Result<Value> {
        self.json(
            Method::GET,
            &query("/oauth/auth-url", "provider", provider.router_provider()),
            None,
            DEFAULT_LIMIT,
        )
    }

    pub fn oauth_status(&self, state: &str) -> Result<Value> {
        self.json(
            Method::GET,
            &query("/oauth/status", "state", state),
            None,
            DEFAULT_LIMIT,
        )
    }

    pub fn cancel_oauth(&self, state: &str) -> Result<bool> {
        let response = self.json(
            Method::DELETE,
            &query("/oauth/session", "state", state),
            None,
            DEFAULT_LIMIT,
        )?;
        Ok(response["cancelled"] == true)
    }

    pub fn oauth_callback(
        &self,
        provider: SubscriptionProvider,
        state: &str,
        parameter: &str,
        value: &str,
    ) -> Result<()> {
        let mut body = json!({"provider": provider.router_provider(), "state": state});
        body[parameter] = json!(value);
        self.json(Method::POST, "/oauth/callback", Some(body), DEFAULT_LIMIT)?;
        Ok(())
    }

    pub fn delete_credential(&self, name: &str) -> Result<()> {
        self.json(
            Method::DELETE,
            &query("/credentials", "name", name),
            None,
            DEFAULT_LIMIT,
        )?;
        Ok(())
    }

    pub fn set_headers(&self, name: &str, headers: Value) -> Result<()> {
        self.json(
            Method::PATCH,
            "/credentials/fields",
            Some(json!({"name": name, "headers": headers})),
            DEFAULT_LIMIT,
        )?;
        Ok(())
    }

    pub fn account_probe(
        &self,
        probe: AccountProbe,
        file: &Value,
        project: Option<&str>,
    ) -> Result<Option<Value>> {
        use AccountProbe::*;
        let (url, data) = match probe {
            CodexUsage => ("https://chatgpt.com/backend-api/wham/usage", None),
            CodexModels => (
                "https://chatgpt.com/backend-api/codex/models?client_version=0.162.1",
                None,
            ),
            AntigravityAccount => (
                "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
                Some(json!({"metadata": {"ideType": "ANTIGRAVITY"}})),
            ),
            AntigravityQuota => (
                "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
                Some(project.map_or_else(|| json!({}), |id| json!({"project": id}))),
            ),
            AntigravityModels => (
                "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
                Some(project.map_or_else(|| json!({}), |id| json!({"project": id}))),
            ),
        };
        let codex = matches!(probe, CodexUsage | CodexModels);
        ensure!(
            file["provider"] == if codex { "codex" } else { "antigravity" },
            "Subscription probe does not match account"
        );
        let index = file["auth_index"]
            .as_str()
            .filter(|id| !id.is_empty())
            .context("Subscription credential is unavailable")?;
        let mut headers = json!({
            "Authorization": "Bearer $TOKEN$",
            "Content-Type": "application/json",
            "User-Agent": if codex { codex_user_agent().to_owned() } else { antigravity_user_agent() },
        });
        if codex {
            headers["originator"] = json!("codex-tui");
            if let Some(account) = file["account_id"]
                .as_str()
                .or_else(|| file["id_token"]["chatgpt_account_id"].as_str())
            {
                headers["ChatGPT-Account-Id"] = json!(account);
            }
        }
        let mut input = json!({"url": url, "method": if data.is_some() { "POST" } else { "GET" }, "auth_index": index, "header": headers});
        if let Some(data) = data {
            input["data"] = json!(data.to_string());
        }
        let limit = if matches!(probe, CodexModels | AntigravityModels) {
            MODEL_LIMIT
        } else {
            DEFAULT_LIMIT
        };
        let envelope = self.json(
            Method::POST,
            "/requests/api-call",
            Some(input),
            limit * 2 + 64 * 1024,
        )?;
        if envelope["status_code"] == 401 {
            return Ok(None);
        }
        ensure!(
            envelope["status_code"]
                .as_u64()
                .is_some_and(|status| (200..300).contains(&status)),
            "Subscription account service is unavailable"
        );
        let body = envelope["body"]
            .as_str()
            .filter(|body| body.len() <= limit)
            .context("Invalid subscription account response")?;
        Ok(Some(serde_json::from_str(body).map_err(|_| {
            anyhow!("Invalid subscription account response")
        })?))
    }

    fn model_list(&self, path: &str) -> Result<Vec<Value>> {
        let body = self.json(Method::GET, path, None, MODEL_LIMIT)?;
        body["models"]
            .as_array()
            .cloned()
            .context("Invalid model router model list")
    }

    pub fn set_disabled(&self, name: &str, disabled: bool) -> Result<()> {
        self.json(
            Method::PATCH,
            "/credentials/status",
            Some(json!({"name": name, "disabled": disabled})),
            DEFAULT_LIMIT,
        )?;
        Ok(())
    }

    pub fn set_prefix(&self, name: &str, prefix: &str) -> Result<()> {
        self.json(
            Method::PATCH,
            "/credentials/fields",
            Some(json!({"name": name, "prefix": prefix})),
            DEFAULT_LIMIT,
        )?;
        Ok(())
    }

    pub fn set_extra_models(&self, name: &str, prefix: &str, models: Vec<Value>) -> Result<()> {
        self.json(
            Method::PATCH,
            "/credentials/fields",
            Some(json!({"name": name, "prefix": prefix, "extra_models": models})),
            DEFAULT_LIMIT,
        )?;
        Ok(())
    }

    pub fn aliases(&self, provider: SubscriptionProvider) -> Result<Vec<ModelAlias>> {
        let (status, pinned, bytes) = self.exchange(
            Method::GET,
            &format!("/config/oauth/model-alias/{}", provider.router_provider()),
            None,
            TIMEOUT,
            DEFAULT_LIMIT,
        )?;
        if status == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        check_status(status, pinned)?;
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid model router aliases"))
    }

    pub fn set_aliases(
        &self,
        provider: SubscriptionProvider,
        aliases: &[ModelAlias],
    ) -> Result<()> {
        self.json(
            Method::PUT,
            &format!("/config/oauth/model-alias/{}", provider.router_provider()),
            Some(json!(aliases)),
            DEFAULT_LIMIT,
        )?;
        Ok(())
    }

    fn json(&self, method: Method, path: &str, body: Option<Value>, limit: usize) -> Result<Value> {
        let (status, pinned, bytes) = self.exchange(method, path, body, TIMEOUT, limit)?;
        check_status(status, pinned)?;
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid model router response"))
    }

    fn exchange(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        timeout: Duration,
        limit: usize,
    ) -> Result<(StatusCode, bool, Bytes)> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let result = runtime.block_on(async {
            tokio::time::timeout(timeout, self.request(method, path, body, limit))
                .await
                .map_err(|_| anyhow!("Model router management request timed out"))?
        });
        runtime.shutdown_background();
        result
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        limit: usize,
    ) -> Result<(StatusCode, bool, Bytes)> {
        let port = self.connection.port;
        let socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|_| anyhow!("Model router management connection failed"))?;
        let (mut sender, connection) = http1::handshake(TokioIo::new(socket))
            .await
            .map_err(|_| anyhow!("Model router management connection failed"))?;
        tokio::spawn(connection);
        let mut bearer =
            header::HeaderValue::from_str(&format!("Bearer {}", self.connection.management_key))
                .map_err(|_| anyhow!("Invalid model router management key"))?;
        bearer.set_sensitive(true);
        let data = body.map_or_else(Vec::new, |json| json.to_string().into_bytes());
        let request = Request::builder()
            .method(method)
            .uri(format!("/v8/management{path}"))
            .header(header::HOST, format!("127.0.0.1:{port}"))
            .header(header::AUTHORIZATION, bearer)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONNECTION, "close")
            .body(Full::new(Bytes::from(data)))?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| anyhow!("Model router management connection failed"))?;
        let status = response.status();
        let pinned = response
            .headers()
            .get("x-cpa-version")
            .is_some_and(|value| value == VERSION);
        if !status.is_success() || !pinned {
            return Ok((status, pinned, Bytes::new()));
        }
        let bytes = Limited::new(response.into_body(), limit)
            .collect()
            .await
            .map_err(|_| anyhow!("Invalid or oversized model router response"))?
            .to_bytes();
        Ok((status, pinned, bytes))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum AccountProbe {
    CodexUsage,
    CodexModels,
    AntigravityAccount,
    AntigravityQuota,
    AntigravityModels,
}

pub(crate) fn antigravity_user_agent() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        value => value,
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        std::env::consts::ARCH
    };
    format!("antigravity/hub/2.29.0 (aidev_client; os_type={os}; arch={arch}; cl=1013060670)")
}

fn query(path: &str, key: &str, value: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair(key, value)
        .finish();
    format!("{path}?{query}")
}

fn check_status(status: StatusCode, pinned: bool) -> Result<()> {
    ensure!(pinned, "Model router version does not match {VERSION}");
    ensure!(
        status.is_success(),
        "Model router management request failed ({status})"
    );
    Ok(())
}
