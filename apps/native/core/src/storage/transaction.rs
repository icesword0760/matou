use std::{error::Error, fmt};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandMetadata {
    pub command_id: String,
    pub command_type: String,
    pub request_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainEventInput {
    pub event_id: String,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_terminal_sequence: Option<i64>,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainCommit {
    pub result: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_event_sequence: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_sequence: Option<i64>,
    pub replayed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CommandReplayConflictError {
    pub command_id: String,
}

impl CommandReplayConflictError {
    pub const CODE: &str = "COMMAND_REPLAY_CONFLICT";
}

impl fmt::Display for CommandReplayConflictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "command id {} was already used for a different request",
            self.command_id
        )
    }
}

impl Error for CommandReplayConflictError {}

/// Commit the mutation, its ordered events, and its replay receipt together.
/// The caller owns serialization of writers for this database.
pub fn execute(
    connection: &mut Connection,
    command: &CommandMetadata,
    committed_at: i64,
    mutation: impl FnOnce(&Transaction<'_>, &mut Vec<DomainEventInput>) -> Result<Value>,
) -> Result<DomainCommit> {
    let stored = connection
        .query_row(
            "SELECT request_hash, response_json, first_event_seq, last_event_seq
             FROM command_deduplication WHERE command_id = ?",
            [&command.command_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            },
        )
        .optional()?;
    if let Some((request_hash, response, first_event_sequence, last_event_sequence)) = stored {
        if request_hash != command.request_hash {
            return Err(CommandReplayConflictError {
                command_id: command.command_id.clone(),
            }
            .into());
        }
        return Ok(DomainCommit {
            result: js_object_order(serde_json::from_str(&response)?),
            first_event_sequence,
            last_event_sequence,
            replayed: true,
        });
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut events = Vec::new();
    let result = js_object_order(mutation(&transaction, &mut events)?);

    let mut first_event_sequence = None;
    let mut last_event_sequence = None;
    if !events.is_empty() {
        let mut insertion = transaction.prepare(
            "INSERT INTO domain_events (
               event_id, event_type, aggregate_type, aggregate_id,
               workspace_id, task_id, session_id, payload_json, schema_version,
               required_terminal_sequence, command_id, causation_id, correlation_id, occurred_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )?;
        for event in events {
            let sequence = insertion.insert(params![
                event.event_id,
                event.event_type,
                event.aggregate_type,
                event.aggregate_id,
                event.workspace_id,
                event.task_id,
                event.session_id,
                serde_json::to_string(&js_object_order(event.payload))?,
                event.schema_version.unwrap_or(1),
                event.required_terminal_sequence,
                command.command_id,
                command.causation_id,
                command.correlation_id,
                event.occurred_at,
            ])?;
            first_event_sequence.get_or_insert(sequence);
            last_event_sequence = Some(sequence);
        }
    }

    transaction.execute(
        "INSERT INTO command_deduplication (
           command_id, command_type, request_hash, response_json,
           first_event_seq, last_event_seq, committed_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![
            command.command_id,
            command.command_type,
            command.request_hash,
            serde_json::to_string(&result)?,
            first_event_sequence,
            last_event_sequence,
            committed_at,
        ],
    )?;
    transaction.commit()?;

    Ok(DomainCommit {
        result,
        first_event_sequence,
        last_event_sequence,
        replayed: false,
    })
}

fn js_object_order(value: Value) -> Value {
    match value {
        Value::Object(mut object) => {
            if object.keys().any(|key| array_index(key).is_some()) {
                let mut entries: Vec<_> = object.into_iter().collect();
                // The stable sort keeps all non-index keys in their original order.
                entries.sort_by_cached_key(|(key, _)| array_index(key).unwrap_or(u32::MAX));
                object = entries.into_iter().collect();
            }
            for value in object.values_mut() {
                *value = js_object_order(value.take());
            }
            Value::Object(object)
        }
        Value::Array(array) => Value::Array(array.into_iter().map(js_object_order).collect()),
        scalar => scalar,
    }
}

fn array_index(key: &str) -> Option<u32> {
    if !matches!(key.as_bytes(), [b'0'] | [b'1'..=b'9', ..]) {
        return None;
    }
    key.parse::<u32>().ok().filter(|&index| index < u32::MAX)
}
