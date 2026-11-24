//! Subscription account presentation helpers.
//!
//! These functions never talk to the runtime: the settings shell supplies the
//! redacted account/quota snapshot and the saved provider row. Keeping the
//! filtering, four-tier defaults and quota wording pure makes the card, the
//! sign-in flow and the assignment rows share one interpretation.
use matou_native_core::claude_role_budget::CLAUDE_ROLES as ROLES;
use matou_native_core::sessions::InterfaceLocale;
use serde_json::Value;

use crate::provider_settings::ProviderDraft;

/// A provider row owns subscription details only when the runtime stored the
/// nested object. `kind` alone is not enough for the card to read an account.
pub(crate) fn subscription(provider: &Value) -> bool {
    provider["subscription"].is_object()
}

pub(crate) fn subscription_kind(provider: &Value) -> Option<&str> {
    subscription(provider)
        .then(|| provider["subscription"]["provider"].as_str())
        .flatten()
}

pub(crate) fn subscription_status(provider: &Value) -> &str {
    provider["subscription"]["account"]["status"]
        .as_str()
        .unwrap_or_default()
}

/// Role assignments and web search only accept an authenticated account. A
/// missing status is not treated as ready: the account has to be re-read and,
/// when needed, signed in again before it can back a Claude Code target.
pub(crate) fn subscription_ready(provider: &Value) -> bool {
    subscription(provider) && subscription_status(provider) == "ready"
}

/// Image/video generators and the non-conversational review helper can appear
/// in an account's raw list but cannot back a Claude Code role.
pub(crate) fn is_usable_model(id: &str) -> bool {
    !["image", "video", "codex-auto-review"]
        .iter()
        .any(|part| id.to_ascii_lowercase().contains(part))
}

pub(crate) fn usable_models(models: &[Value]) -> Vec<&Value> {
    models
        .iter()
        .filter(|model| {
            model["id"]
                .as_str()
                .is_some_and(|id| !id.is_empty() && is_usable_model(id))
        })
        .collect()
}

/// `subscriptionDefaults` from the frozen renderer: a saved mapping wins while
/// its model still exists, otherwise the provider-specific preferred model is
/// used and the first usable model is the final fallback. Fable never stays
/// empty: it keeps a native Fable model when present and otherwise follows
/// Opus, matching the runtime's historical three-tier inheritance.
pub(crate) fn subscription_defaults(
    kind: &str,
    models: &[Value],
    saved: &[String; ROLES.len()],
) -> [String; ROLES.len()] {
    let usable = usable_models(models);
    let first = usable
        .first()
        .and_then(|model| model["id"].as_str())
        .unwrap_or_default();
    let choose = |index: usize| -> String {
        let role = ROLES[index];
        let saved = saved[index].trim();
        if !saved.is_empty() && usable.iter().any(|model| model["id"] == saved) {
            return saved.to_owned();
        }
        let preferred = usable.iter().find(|model| {
            let id = model["id"].as_str().unwrap_or_default();
            let label = format!("{id} {}", model["displayName"].as_str().unwrap_or_default())
                .to_ascii_lowercase();
            if kind == "codex" {
                id == match role {
                    "opus" => "gpt-6-astra",
                    "sonnet" => "gpt-6.1-sol",
                    "haiku" => "gpt-6-luna",
                    _ => "",
                }
            } else {
                match role {
                    "opus" => {
                        label.contains("claude")
                            && label.contains("opus")
                            && label.contains("thinking")
                    }
                    "sonnet" => label.contains("claude") && label.contains("sonnet"),
                    "haiku" => label.contains("gemini") && label.contains("flash"),
                    _ => false,
                }
            }
        });
        if let Some(model) = preferred {
            return model["id"].as_str().unwrap_or_default().to_owned();
        }
        if kind == "antigravity" && role == "opus" {
            if let Some(model) = usable.iter().find(|model| {
                let label = format!(
                    "{} {}",
                    model["id"].as_str().unwrap_or_default(),
                    model["displayName"].as_str().unwrap_or_default()
                )
                .to_ascii_lowercase();
                label.contains("claude") && label.contains("opus")
            }) {
                return model["id"].as_str().unwrap_or_default().to_owned();
            }
        }
        first.to_owned()
    };
    let opus = choose(0);
    let fable = {
        let saved = saved[1].trim();
        if !saved.is_empty() && usable.iter().any(|model| model["id"] == saved) {
            saved.to_owned()
        } else {
            let native = (kind == "antigravity")
                .then(|| {
                    usable.iter().find(|model| {
                        let label = format!(
                            "{} {}",
                            model["id"].as_str().unwrap_or_default(),
                            model["displayName"].as_str().unwrap_or_default()
                        )
                        .to_ascii_lowercase();
                        label.contains("claude") && label.contains("fable")
                    })
                })
                .flatten()
                .and_then(|model| model["id"].as_str());
            native.map(str::to_owned).unwrap_or_else(|| opus.clone())
        }
    };
    [opus, fable, choose(2), choose(3)]
}

/// Fill the four tiers from the account directory before the editor renders
/// it. Saved mappings that still exist are kept; an empty or stale mapping is
/// replaced so the models phase never opens with blank role selects.
pub(crate) fn normalize_subscription_draft(draft: &mut ProviderDraft) {
    let Some(kind) = draft.subscription_kind.clone() else {
        return;
    };
    let models: Vec<Value> = draft
        .available_models()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    if !models.is_empty() {
        draft.model_mapping = subscription_defaults(&kind, &models, &draft.model_mapping);
    }
    for effort in &mut draft.effort_mapping {
        if effort.is_empty() {
            *effort = "follow".into();
        }
    }
    if draft.reasoning_effort.is_empty() {
        draft.reasoning_effort = "follow".into();
    }
}

fn quota_rows(details: Option<&Value>) -> &[Value] {
    let Some(details) = details else {
        return &[];
    };
    details["quota"]
        .as_array()
        .or_else(|| details["quota"]["windows"].as_array())
        .or_else(|| details["quota"]["items"].as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn used_percent(window: &Value) -> Option<f64> {
    window["usedPercent"]
        .as_f64()
        .filter(|percent| percent.is_finite())
}

fn percent(value: f64) -> i64 {
    value.round().clamp(0., 100.) as i64
}

fn window_name(window: &Value, locale: InterfaceLocale) -> String {
    let zh = locale == InterfaceLocale::Chinese;
    match window["name"].as_str().unwrap_or_default() {
        "five_hour" => {
            if zh {
                "5 小时".into()
            } else {
                "5h".into()
            }
        }
        "weekly" => {
            if zh {
                "每周".into()
            } else {
                "Weekly".into()
            }
        }
        name if name.is_empty() => String::new(),
        name => name.to_owned(),
    }
}

/// One-line quota summary for the subscription card. Codex keeps the 5-hour and
/// weekly windows; Antigravity groups windows by account family and reports the
/// remaining budget of the most constrained window in each group.
pub(crate) fn quota_text(kind: &str, details: Option<&Value>, locale: InterfaceLocale) -> String {
    let quota = quota_rows(details);
    let zh = locale == InterfaceLocale::Chinese;
    if kind == "codex" {
        let pick = |name: &str, seconds: u64| {
            quota.iter().find(|window| {
                window["name"] == name || window["windowSeconds"].as_u64() == Some(seconds)
            })
        };
        return [
            (pick("five_hour", 18_000), if zh { "5 小时" } else { "5h" }),
            (pick("weekly", 604_800), if zh { "每周" } else { "Weekly" }),
        ]
        .into_iter()
        .filter_map(|(window, label)| Some(format!("{label} {}%", percent(used_percent(window?)?))))
        .collect::<Vec<_>>()
        .join(" · ");
    }
    let mut groups: Vec<(String, f64)> = Vec::new();
    for window in quota {
        let Some(used) = used_percent(window) else {
            continue;
        };
        let raw = window["group"]
            .as_str()
            .or_else(|| window["name"].as_str())
            .unwrap_or_default();
        let label = if raw.to_ascii_lowercase().contains("claude") {
            "Claude"
        } else if raw == "gemini" {
            "Gemini"
        } else if raw == "gemini-argon" {
            "Gemini Argon"
        } else {
            raw
        };
        if label.is_empty() {
            continue;
        }
        match groups.iter_mut().find(|(name, _)| name == label) {
            Some((_, current)) if *current < used => *current = used,
            Some(_) => {}
            None => groups.push((label.to_owned(), used)),
        }
    }
    groups.sort_by_key(|(label, _)| if label == "Claude" { 0 } else { 1 });
    groups
        .into_iter()
        .map(|(label, used)| {
            let left = percent(100. - used);
            if zh {
                format!("{label} 剩余 {left}%")
            } else {
                format!("{label} {left}% left")
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Expanded quota wording for the card's secondary line. Native GPUI has no
/// hover surface for the frozen `title` attribute, so the shell can expose the
/// same information through the probe and a reason line when needed.
pub(crate) fn quota_title(details: Option<&Value>, locale: InterfaceLocale) -> String {
    let zh = locale == InterfaceLocale::Chinese;
    quota_rows(details)
        .iter()
        .filter_map(|window| {
            let used = used_percent(window)?;
            let name = window_name(window, locale);
            let left = percent(100. - used);
            let remaining = if zh {
                format!("剩余 {left}%")
            } else {
                format!("{left}% left")
            };
            let reset = window["resetsAt"]
                .as_str()
                .filter(|value| !value.is_empty())
                .map(|value| {
                    if zh {
                        format!("重置：{value}")
                    } else {
                        format!("Resets: {value}")
                    }
                })
                .unwrap_or_default();
            Some(
                [name, remaining, reset]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Frozen `SubscriptionProviderCard` account line.
pub(crate) fn account_line(provider: &Value, locale: InterfaceLocale) -> String {
    let account = &provider["subscription"]["account"];
    [
        account["email"].as_str().map(str::to_owned),
        account["plan"].as_str().map(str::to_owned),
        subscription(provider).then(|| {
            if subscription_status(provider) == "ready" {
                locale.text("已登录", "Signed in").to_owned()
            } else {
                locale.text("需要重新登录", "Sign in required").to_owned()
            }
        }),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn models() -> Vec<Value> {
        vec![
            json!({"id": "gpt-6.1-sol", "displayName": "Sol"}),
            json!({"id": "gpt-6-astra", "displayName": "Astra"}),
            json!({"id": "gpt-6-luna", "displayName": "Luna"}),
            json!({"id": "image-only"}),
            json!({"id": "video-lite"}),
            json!({"id": "codex-auto-review"}),
        ]
    }

    #[test]
    fn defaults_choose_provider_models_and_keep_fable_on_opus() {
        let defaults = subscription_defaults("codex", &models(), &Default::default());
        assert_eq!(
            defaults,
            [
                "gpt-6-astra".to_owned(),
                "gpt-6-astra".to_owned(),
                "gpt-6.1-sol".to_owned(),
                "gpt-6-luna".to_owned()
            ]
        );
        // A saved Fable mapping that still exists wins; a stale one falls back.
        let saved = [
            "gpt-6-astra".into(),
            "gpt-6-luna".into(),
            "gpt-6.1-sol".into(),
            "gpt-6-luna".into(),
        ];
        assert_eq!(
            subscription_defaults("codex", &models(), &saved)[1],
            "gpt-6-luna"
        );
        let stale = [
            "gone".into(),
            "also-gone".into(),
            String::new(),
            String::new(),
        ];
        let fallback = vec![json!({"id": "only-model"})];
        let defaults = subscription_defaults("codex", &fallback, &stale);
        assert_eq!(defaults[0], "only-model"); // first usable model
        assert_eq!(defaults[1], "only-model");
    }

    #[test]
    fn antigravity_preferences_and_native_fable_follow_the_frozen_rules() {
        let rows = vec![
            json!({"id": "claude-opus-4", "displayName": "Claude Opus"}),
            json!({"id": "claude-opus-5-thinking", "displayName": "Claude Opus Thinking"}),
            json!({"id": "claude-fable-5", "displayName": "Claude Fable"}),
            json!({"id": "claude-sonnet-4", "displayName": "Claude Sonnet"}),
            json!({"id": "gemini-3-flash", "displayName": "Gemini Flash"}),
        ];
        let defaults = subscription_defaults("antigravity", &rows, &Default::default());
        assert_eq!(defaults[0], "claude-opus-5-thinking");
        assert_eq!(defaults[1], "claude-fable-5");
        assert_eq!(defaults[2], "claude-sonnet-4");
        assert_eq!(defaults[3], "gemini-3-flash");
    }

    #[test]
    fn codex_quota_reads_both_windows_and_ignores_incomplete_rows() {
        let details = json!({"quota": [
            {"name": "five_hour", "usedPercent": 12.4},
            {"name": "weekly", "usedPercent": 67.6},
            {"name": "unknown", "usedPercent": 99},
            {"name": "broken"}
        ]});
        assert_eq!(
            quota_text("codex", Some(&details), InterfaceLocale::Chinese),
            "5 小时 12% · 每周 68%"
        );
        assert_eq!(
            quota_text("codex", Some(&details), InterfaceLocale::English),
            "5h 12% · Weekly 68%"
        );
        assert_eq!(quota_text("codex", None, InterfaceLocale::Chinese), "");
    }

    #[test]
    fn antigravity_quota_groups_windows_and_shows_remaining_budget() {
        let details = json!({"quota": [
            {"name": "gemini", "group": "gemini", "usedPercent": 40},
            {"name": "claude-a", "group": "claude", "usedPercent": 10},
            {"name": "claude-b", "group": "claude", "usedPercent": 25},
            {"name": "argon", "group": "gemini-argon", "usedPercent": 5}
        ]});
        assert_eq!(
            quota_text("antigravity", Some(&details), InterfaceLocale::Chinese),
            "Claude 剩余 75% · Gemini 剩余 60% · Gemini Argon 剩余 95%"
        );
        assert_eq!(
            quota_text("antigravity", Some(&details), InterfaceLocale::English),
            "Claude 75% left · Gemini 60% left · Gemini Argon 95% left"
        );
        let title = quota_title(Some(&details), InterfaceLocale::Chinese);
        assert!(title.contains("claude-a · 剩余 90%"));
    }

    #[test]
    fn account_line_and_ready_gate_distinguish_missing_and_signed_out_accounts() {
        let ready = json!({"subscription": {"provider": "codex", "account": {
            "status": "ready", "email": "a@example.com", "plan": "Pro"
        }}});
        assert!(subscription_ready(&ready));
        assert_eq!(
            account_line(&ready, InterfaceLocale::Chinese),
            "a@example.com · Pro · 已登录"
        );
        let needs_sign_in = json!({"subscription": {"provider": "codex", "account": {
            "status": "needs-sign-in", "email": "a@example.com"
        }}});
        assert!(!subscription_ready(&needs_sign_in));
        assert_eq!(
            account_line(&needs_sign_in, InterfaceLocale::Chinese),
            "a@example.com · 需要重新登录"
        );
        assert!(!subscription_ready(&json!({
            "subscription": {"provider": "codex"}
        })));
        assert!(!subscription_ready(&json!({"kind": "api"})));
    }
}
