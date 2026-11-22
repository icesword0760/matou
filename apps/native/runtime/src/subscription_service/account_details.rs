use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Result, anyhow};
use matou_native_core::provider_config::iso_time;
use serde_json::{Value, json};

use super::needs_sign_in;
use crate::{
    model_router::{AccountProbe, ManagementClient},
    protocol::{SubscriptionDetails, SubscriptionQuota},
    subscription_launch::{SubscriptionProvider, project_catalog},
};

pub(super) fn read_details(
    client: &ManagementClient<'_>,
    provider: SubscriptionProvider,
    file: &Value,
    known_models: Option<&[Value]>,
) -> SubscriptionDetails {
    let mut details = SubscriptionDetails {
        account: account(
            file,
            if needs_sign_in(file) {
                "needs-sign-in"
            } else {
                "ready"
            },
        ),
        models: known_models.unwrap_or_default().to_vec(),
        quota: Vec::new(),
    };
    if needs_sign_in(file) {
        return details;
    }
    let probe = match provider {
        SubscriptionProvider::ChatGpt => AccountProbe::CodexUsage,
        SubscriptionProvider::Antigravity => AccountProbe::AntigravityAccount,
    };
    let mut project = file["project_id"]
        .as_str()
        .and_then(safe_label)
        .map(str::to_owned);
    match client.account_probe(probe, file, None) {
        Ok(None) => details.account["status"] = json!("needs-sign-in"),
        Ok(Some(body)) if provider == SubscriptionProvider::ChatGpt => {
            if let Some(plan) = codex_plan(&body["plan_type"]) {
                details.account["plan"] = json!(plan);
            }
            details.quota = codex_quota(&body);
        }
        Ok(Some(body)) => {
            project = project_id(&body).or(project);
            if let Some(plan) = antigravity_plan(&body) {
                details.account["plan"] = json!(plan);
            }
            if antigravity_ineligible(&body) {
                details.account["status"] = json!("account-unavailable");
            }
        }
        Err(_) => {}
    }
    if details.account["status"] == "needs-sign-in" {
        return details;
    }
    if provider == SubscriptionProvider::Antigravity
        && let Some(project) = project.as_deref()
    {
        match client.account_probe(AccountProbe::AntigravityQuota, file, Some(project)) {
            Ok(Some(body)) => details.quota = antigravity_quota(&body),
            Ok(None) => details.account["status"] = json!("needs-sign-in"),
            Err(_) => {}
        }
    }
    if known_models.is_none() && details.account["status"] != "needs-sign-in" {
        match read_models(client, provider, file, project.as_deref(), false) {
            Ok(models) => details.models = models,
            Err(error) if error.is::<SignInRequired>() => {
                details.account["status"] = json!("needs-sign-in");
            }
            Err(_) => {}
        }
    }
    details
}

pub(super) fn read_models(
    client: &ManagementClient<'_>,
    provider: SubscriptionProvider,
    file: &Value,
    project: Option<&str>,
    strict: bool,
) -> Result<Vec<Value>> {
    let probe = match provider {
        SubscriptionProvider::ChatGpt => AccountProbe::CodexModels,
        SubscriptionProvider::Antigravity => AccountProbe::AntigravityModels,
    };
    let project = project.or_else(|| file["project_id"].as_str().and_then(safe_label));
    let body = match client.account_probe(probe, file, project) {
        Ok(Some(body)) => Some(body),
        Ok(None) => return Err(SignInRequired.into()),
        Err(error) if strict => return Err(error),
        Err(_) => None,
    };
    let definitions = client.model_definitions(provider).unwrap_or_default();
    let catalog = project_catalog(provider, body.as_ref(), &definitions, strict)
        .map_err(|_| anyhow!("Model list could not be read"))?;
    catalog
        .into_iter()
        .map(|model| Ok(serde_json::to_value(model)?))
        .collect()
}

#[derive(Debug)]
pub(super) struct SignInRequired;

impl std::fmt::Display for SignInRequired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Subscription sign-in is required")
    }
}

impl std::error::Error for SignInRequired {}

fn account(file: &Value, status: &str) -> Value {
    let mut account = json!({"status": status});
    if let Some(email) = file["email"]
        .as_str()
        .and_then(safe_label)
        .filter(|email| email.contains('@') && !email.chars().any(char::is_whitespace))
    {
        account["email"] = json!(email);
    }
    let plan = file
        .get("plan_type")
        .or(file.get("plan"))
        .unwrap_or(&file["id_token"]["plan_type"]);
    let plan = if file["provider"] == "codex" {
        codex_plan(plan)
    } else {
        plan.as_str().and_then(safe_label).map(str::to_owned)
    };
    if let Some(plan) = plan {
        account["plan"] = json!(plan);
    }
    account
}

fn safe_label(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 150
        && !value.chars().any(char::is_control)
        && !value.contains(['<', '>', '?', '=', '/'])
        && !value.to_ascii_lowercase().contains("bearer ")
        && !value.starts_with("eyJ"))
    .then_some(value)
}

fn codex_plan(input: &Value) -> Option<String> {
    let label = input.as_str().and_then(safe_label)?.to_ascii_lowercase();
    let known = match label.as_str() {
        "unknown" => return None,
        "free" => "Free",
        "go" => "Go",
        "plus" => "Plus",
        "prolite" => "Pro 100",
        "pro" => "Pro 200",
        "promax" => "Pro 500",
        "team" | "self_serve_business_usage_based" => "Business",
        "self_serve_business_prolite" => "Business Premium",
        "business" | "ent26" => "Enterprise",
        "edu" => "Edu",
        "edu_plus" => "Edu Plus",
        "edu_pro" => "Edu Pro",
        _ if label.starts_with("enterprise") => "Enterprise",
        _ => {
            return Some(
                label
                    .split(['_', '-', ' '])
                    .filter_map(|word| {
                        let mut chars = word.chars();
                        Some(chars.next()?.to_uppercase().collect::<String>() + chars.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
    };
    Some(known.to_owned())
}

fn codex_quota(body: &Value) -> Vec<SubscriptionQuota> {
    let rate = &body["rate_limit"];
    let mut windows = ["primary_window", "secondary_window"]
        .into_iter()
        .filter_map(|key| {
            let window = &rate[key];
            let used = window["used_percent"]
                .as_f64()
                .filter(|used| *used >= 0.0)?;
            let seconds = window["limit_window_seconds"]
                .as_u64()
                .filter(|seconds| *seconds > 0);
            let name = match seconds {
                Some(18_000) => "five_hour",
                Some(604_800) => "weekly",
                _ => "unknown",
            };
            let resets_at = window["reset_at"]
                .as_u64()
                .filter(|_| used > 0.0)
                .and_then(|reset| UNIX_EPOCH.checked_add(Duration::from_secs(reset)))
                .and_then(|time| iso_time(time).ok());
            Some(SubscriptionQuota {
                name: name.into(),
                group: None,
                used_percent: used.min(100.0),
                window_seconds: seconds,
                resets_at,
            })
        })
        .collect::<Vec<_>>();
    windows.sort_by_key(|window| window.window_seconds.unwrap_or(u64::MAX));
    windows
}

fn project_id(body: &Value) -> Option<String> {
    let project = &body["cloudaicompanionProject"];
    project
        .as_str()
        .or_else(|| project["id"].as_str())
        .or_else(|| project["projectId"].as_str())
        .and_then(safe_label)
        .map(str::to_owned)
}

fn antigravity_plan(body: &Value) -> Option<String> {
    let current = &body["currentTier"];
    let paid = &body["paidTier"];
    let (label, tier) = if let Some(name) = paid["name"].as_str().and_then(safe_label) {
        (name, &paid["id"])
    } else {
        let name = match current["id"].as_str() {
            Some("free-tier") => "Free",
            Some("standard-tier") => "Paid",
            _ => current["name"].as_str().and_then(safe_label)?,
        };
        (name, &current["id"])
    };
    let variant = match tier.as_str() {
        Some("g1-ultra-lite-tier") => Some("5x"),
        Some("g1-ultra-tier") => Some("20x"),
        _ => None,
    };
    if let Some(variant) = variant.filter(|_| {
        label.to_ascii_lowercase().contains("ultra")
            && !label.contains("5x")
            && !label.contains("20x")
    }) {
        return Some(format!("{label} {variant}"));
    }
    Some(label.into())
}

fn antigravity_ineligible(body: &Value) -> bool {
    let has_current = body["currentTier"]["id"]
        .as_str()
        .is_some_and(|id| !id.is_empty());
    let has_free = body["currentTier"]["id"] == "free-tier"
        || body["allowedTiers"]
            .as_array()
            .is_some_and(|tiers| tiers.iter().any(|tier| tier["id"] == "free-tier"));
    body["ineligibleTiers"].as_array().is_some_and(|tiers| {
        tiers.iter().any(|tier| {
            tier["reasonMessage"]
                .as_str()
                .is_some_and(|reason| !reason.trim().is_empty())
                && (!has_current
                    || (!has_free && body["gcpManaged"] != true && tier["tierId"] == "free-tier"))
        })
    })
}

fn antigravity_quota(body: &Value) -> Vec<SubscriptionQuota> {
    let buckets = body["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| group["buckets"].as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    [
        ("gemini-5h", "agy_gemini_five_hour", "gemini"),
        ("gemini-weekly", "agy_gemini_weekly", "gemini"),
        (
            "gemini-argon-wtu-weekly",
            "agy_gemini_argon_weekly",
            "gemini-argon",
        ),
        ("3p-5h", "agy_third_party_five_hour", "claude-gpt"),
        ("3p-weekly", "agy_third_party_weekly", "claude-gpt"),
    ]
    .into_iter()
    .filter_map(|(id, name, group)| {
        let bucket = buckets.iter().find(|bucket| bucket["bucketId"] == id)?;
        let fraction = bucket["remainingFraction"].as_f64()?.clamp(0.0, 1.0);
        let resets_at = bucket["resetTime"]
            .as_str()
            .filter(|time| {
                time.len() <= 40
                    && time
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || b"-:TZ.+".contains(&byte))
            })
            .map(str::to_owned);
        Some(SubscriptionQuota {
            name: name.into(),
            group: Some(group.into()),
            used_percent: (1.0 - fraction) * 100.0,
            window_seconds: None,
            resets_at,
        })
    })
    .collect()
}
