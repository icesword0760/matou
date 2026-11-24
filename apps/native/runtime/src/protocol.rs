//! Versioned messages for private native parent/child pipes, not the external CLI.
//!
//! This module encodes owned data only. The child and supervisor must check Hello
//! before accepting domain commands when their connection is integrated.
use std::io::{BufReader, Read, Write};

use anyhow::{Result, ensure};
use matou_native_core::provider_launch::{ClaudeRoleAssignment, ClaudeRoleAssignments};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::host::{Command, Event};

/// Read-only inputs to verify a moved worktree's Git registration.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRepairQuery {
    pub repository_root: std::path::PathBuf,
    pub isolation_root: std::path::PathBuf,
    pub branch: String,
    pub candidate: std::path::PathBuf,
}

pub const VERSION: u32 = 3;

#[derive(Serialize, Deserialize)]
pub enum ClientMessage {
    Hello { version: u32 },
    Command(Command),
}

#[derive(Serialize, Deserialize)]
pub enum ServerMessage {
    Hello { version: u32 },
    Event(Event),
}

/// Returned in the Graph event's `claudeRoleAssignmentsResult` field.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeRoleAssignmentsReply {
    pub request_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignments: Option<ClaudeRoleAssignments>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present"
    )]
    pub web_search: Option<Option<ClaudeRoleAssignment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaudeModelPreferencesReply {
    #[serde(flatten)]
    pub roles: ClaudeRoleAssignmentsReply,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeZoneSettingsSnapshot {
    pub time_zone: String,
    pub system_time_zone: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_time_zone: Option<String>,
    pub zones: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TimeZoneSettingsError {
    InvalidRequest,
    Storage,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeZoneSettingsReply {
    pub request_id: u64,
    pub result: Result<TimeZoneSettingsSnapshot, TimeZoneSettingsError>,
}

// A missing field uses Serde's default. A present null belongs to the payload.
pub(crate) fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCatalogModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_efforts: Option<Vec<String>>,
}

/// Returned in the Graph event's `claudeModelCatalogResult` field.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeModelCatalogReply {
    pub request_id: u64,
    #[serde(flatten)]
    pub result: ClaudeModelCatalogResult,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClaudeModelCatalogResult {
    Models(Vec<ClaudeCatalogModel>),
    Error(String),
}

/// Returned in the Graph event's `providerSettingsResult` field.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsReply {
    pub request_id: u64,
    #[serde(flatten)]
    pub result: ProviderSettingsResult,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderSettingsResult {
    Snapshot(serde_json::Value),
    Provider(serde_json::Value),
    Models(serde_json::Value),
    Accepted(bool),
    SubscriptionLogin(SubscriptionLoginView),
    SubscriptionSettings(SubscriptionSettingsView),
    SubscriptionModels(SubscriptionModelsView),
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubscriptionLoginStatus {
    Waiting,
    Exchanging,
    Ready,
    Failed,
    Expired,
    Cancelled,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionLoginView {
    pub attempt_id: String,
    pub provider: crate::subscription_launch::SubscriptionProvider,
    pub status: SubscriptionLoginStatus,
    pub can_reopen: bool,
    pub system_browser_available: bool,
    pub reason: Option<String>,
    pub draft: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionDetails {
    pub account: Value,
    pub models: Vec<Value>,
    pub quota: Vec<SubscriptionQuota>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionQuota {
    pub name: String,
    pub group: Option<String>,
    pub used_percent: f64,
    pub window_seconds: Option<u64>,
    pub resets_at: Option<String>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionSettingsView {
    pub snapshot: Value,
    pub accounts: std::collections::BTreeMap<String, SubscriptionDetails>,
    pub antigravity_risk_accepted: bool,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionModelsView {
    pub snapshot: Value,
    pub provider_id: String,
    pub details: SubscriptionDetails,
}

pub fn check_version(version: u32) -> Result<()> {
    ensure!(
        version == VERSION,
        "unsupported native runtime protocol version {version}; expected {VERSION}"
    );
    Ok(())
}

/// Stream one JSON value, append LF and flush so the peer can consume it now.
pub fn write_message(writer: &mut impl Write, message: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *writer, message)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

/// One persistent parser retains read-ahead and split UTF-8/JSON across values.
/// Clean EOF ends iteration; malformed or truncated data is returned as an error.
pub fn read_messages<T: DeserializeOwned>(
    reader: impl Read,
) -> impl Iterator<Item = serde_json::Result<T>> {
    serde_json::Deserializer::from_reader(BufReader::new(reader)).into_iter()
}
