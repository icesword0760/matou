//! Context limits for the Claude tiers and their optional compaction target.

use anyhow::{Result, ensure};
use serde_json::{Map, Value};

const MINIMUM_CONTEXT: u64 = 100_000;
const FIXED_CONTEXT: u64 = 512_000;
pub const MAXIMUM_CONTEXT: u64 = 1_000_000;
pub const CLAUDE_ROLES: [&str; 4] = ["opus", "fable", "sonnet", "haiku"];
pub const CLAUDE_ROLE_LABELS: [&str; CLAUDE_ROLES.len()] = ["Opus", "Fable", "Sonnet", "Haiku"];

#[derive(Clone, Copy)]
pub struct ClaudeBudgetTarget<'a> {
    pub provider_id: &'a str,
    pub provider_name: &'a str,
    pub context_window: Option<u64>,
    pub max_context_window: Option<u64>,
}

pub enum ClaudeBudgetCompaction<'a> {
    Follow,
    Custom(ClaudeBudgetTarget<'a>),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ClaudeRoleBudget {
    pub context_window: Option<u64>,
    pub unknown_context_roles: Vec<&'static str>,
    pub unknown_compaction_context: bool,
}

/// `roles` follows `CLAUDE_ROLES` order, including for the first threshold error.
pub fn claude_role_budget(
    roles: [ClaudeBudgetTarget<'_>; CLAUDE_ROLES.len()],
    compaction: ClaudeBudgetCompaction<'_>,
    context_windows: &Map<String, Value>,
) -> Result<ClaudeRoleBudget> {
    let mut budget = ClaudeRoleBudget::default();
    for (role, target) in CLAUDE_ROLES.into_iter().zip(roles) {
        if let Some(window) = target_window(role, target, context_windows)? {
            include_window(&mut budget.context_window, window);
        } else {
            budget.unknown_context_roles.push(role);
        }
    }
    if let ClaudeBudgetCompaction::Custom(target) = compaction {
        if let Some(window) = target_window("compaction", target, context_windows)? {
            include_window(&mut budget.context_window, window);
        } else {
            budget.unknown_compaction_context = true;
        }
    }
    Ok(budget)
}

fn target_window(
    role: &str,
    target: ClaudeBudgetTarget<'_>,
    context_windows: &Map<String, Value>,
) -> Result<Option<u64>> {
    let preference = context_windows.get(target.provider_id);
    let mode = preference.and_then(Value::as_str);
    let cap = preference.and_then(Value::as_u64).or(match mode {
        Some("maximum") => target.max_context_window.or(target.context_window),
        _ => target.context_window,
    });
    let Some(cap) = cap else {
        return Ok(None);
    };
    ensure!(
        cap >= MINIMUM_CONTEXT,
        "{role} · {}: context window is under 100K",
        target.provider_name
    );
    Ok(Some(match mode {
        Some("512k") => cap.min(FIXED_CONTEXT),
        _ => cap,
    }))
}

fn include_window(budget: &mut Option<u64>, window: u64) {
    *budget = Some(budget.unwrap_or(MAXIMUM_CONTEXT).min(window));
}
