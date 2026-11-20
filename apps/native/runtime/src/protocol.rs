//! Versioned messages for private native parent/child pipes, not the external CLI.
//!
//! This module encodes owned data only. The child and supervisor must check Hello
//! before accepting domain commands when their connection is integrated.
use std::io::{BufReader, Read, Write};

use anyhow::{Result, ensure};
use matou_native_core::provider_launch::ClaudeRoleAssignments;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::host::{Command, Event};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsReply {
    pub request_id: u64,
    #[serde(flatten)]
    pub result: ProviderSettingsResult,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderSettingsResult {
    Snapshot(serde_json::Value),
    Provider(serde_json::Value),
    Accepted(bool),
    Error(String),
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
