use std::time::Duration;

use http_body_util::{BodyExt, Empty};
use hyper::{Request, StatusCode, body::Bytes};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use matou_native_core::provider_config::ProviderConfigStore;
use serde_json::{Value, json};
use url::Url;

const BODY_LIMIT: usize = 512_000;
const DEADLINE: Duration = Duration::from_secs(10);

pub(super) fn fetch(store: &ProviderConfigStore, input: &Value) -> Value {
    fetch_models(store, input).unwrap_or_else(|error| json!({ "error": error }))
}

fn fetch_models(store: &ProviderConfigStore, input: &Value) -> Result<Value, &'static str> {
    let cli = input["cli"].as_str().ok_or("invalid")?;
    if !matches!(cli, "claude-code" | "codex") {
        return Err("invalid");
    }
    let endpoint = input["endpoint"].as_str().ok_or("invalid")?.trim();
    let url = models_url(endpoint)?;
    let key = store
        .model_discovery_key(
            cli,
            input["providerId"].as_str(),
            endpoint,
            input["apiKey"].as_str().unwrap_or_default().trim(),
        )
        .map_err(|_| "network")?
        .filter(|key| !key.is_empty() && key.bytes().all(|byte| (0x21..=0x7e).contains(&byte)))
        .ok_or("key-required")?;

    let mut request = Request::get(url.as_str())
        .header("accept", "application/json")
        .header("authorization", format!("Bearer {key}"));
    if cli == "claude-code" {
        request = request
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01");
    }
    let request = request.body(Empty::<Bytes>::new()).map_err(|_| "invalid")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "network")?;
    runtime.block_on(async {
        tokio::time::timeout(DEADLINE, discover(request))
            .await
            .map_err(|_| "timeout")?
    })
}

fn models_url(endpoint: &str) -> Result<Url, &'static str> {
    let mut url = Url::parse(endpoint).map_err(|_| "invalid")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid");
    }
    if url.as_str().trim_end_matches('/') == "https://api.deepseek.com/anthropic" {
        url.set_path("/models");
    } else {
        let path = url.path().strip_suffix('/').unwrap_or(url.path());
        url.set_path(&format!("{path}/models"));
    }
    Ok(url)
}

async fn discover(request: Request<Empty<Bytes>>) -> Result<Value, &'static str> {
    // No redirect handler, so credentials are never sent to another URL.
    let response = if request.uri().scheme_str() == Some("http") {
        Client::builder(TokioExecutor::new())
            .build_http()
            .request(request)
            .await
    } else {
        let connector = HttpsConnectorBuilder::new()
            .with_native_roots()
            .map_err(|_| "network")?
            .https_only()
            .enable_http1()
            .build();
        Client::builder(TokioExecutor::new())
            .build(connector)
            .request(request)
            .await
    }
    .map_err(|_| "network")?;
    match response.status() {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err("auth"),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => return Err("unsupported"),
        status if !status.is_success() => return Err("network"),
        _ => {}
    }
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame.map_err(|_| "network")?.into_data() {
            if data.len() > BODY_LIMIT - bytes.len() {
                return Err("too-large");
            }
            bytes.extend_from_slice(&data);
        }
    }
    let body: Value =
        serde_json::from_str(&String::from_utf8_lossy(&bytes)).map_err(|_| "network")?;
    let rows = body["data"].as_array().ok_or("unsupported")?;
    let upstream: Vec<_> = rows
        .iter()
        .map(|row| {
            let name = row["name"].as_str().filter(|name| {
                !name.trim().is_empty()
                    && name.encode_utf16().count() <= 240
                    && !name.chars().any(char::is_control)
            });
            json!({
                "id": row["id"],
                "displayName": name.map_or_else(|| row["display_name"].clone(), |name| json!(name)),
                "contextWindow": row["context_window"],
                "maxContextWindow": row["max_context_window"],
                "maxOutputTokens": row["max_output_tokens"],
                "supportedEfforts": row["effort"]["supported_levels"],
            })
        })
        .collect();
    let metadata = ProviderConfigStore::normalize_model_metadata(&Value::Array(upstream))
        .expect("upstream models are an array");
    let rows = metadata.as_array().expect("normalized models are an array");
    if rows.is_empty() {
        return Err("empty");
    }
    let models: Vec<_> = rows.iter().map(|row| &row["id"]).collect();
    if rows
        .iter()
        .any(|row| row.as_object().is_some_and(|row| row.len() > 1))
    {
        Ok(json!({ "models": models, "modelMetadata": metadata }))
    } else {
        Ok(json!({ "models": models }))
    }
}
