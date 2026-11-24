use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::{copy_present, copy_strings, copy_truthy, trim};
use crate::provider_launch::EFFORTS;

const MAX_MODELS: usize = 1500;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const TOKEN_FIELDS: [&str; 3] = ["contextWindow", "maxContextWindow", "maxOutputTokens"];

pub(super) fn normalize_metadata(value: &Value) -> Option<Value> {
    let mut by_id = HashMap::<&str, Map<String, Value>>::new();
    for raw in value.as_array()? {
        let Some(id) = model_text(&raw["id"]) else {
            continue;
        };
        let mut model = Map::new();
        model.insert("id".into(), json!(id));
        if let Some(name) = model_text(&raw["displayName"]) {
            model.insert("displayName".into(), json!(name));
        }
        for field in TOKEN_FIELDS {
            if let Some(count) = raw[field]
                .as_f64()
                .filter(|count| *count > 0.0 && *count <= MAX_SAFE_INTEGER && count.fract() == 0.0)
            {
                model.insert(field.into(), json!(count as u64));
            }
        }
        if let Some(values) = raw["supportedEfforts"].as_array() {
            let mut efforts = Vec::new();
            for level in values.iter().filter_map(Value::as_str) {
                if EFFORTS[1..].contains(&level) && !efforts.contains(&level) {
                    efforts.push(level);
                }
            }
            if !efforts.is_empty() {
                model.insert("supportedEfforts".into(), json!(efforts));
            }
        }
        let existing = by_id.entry(id).or_default();
        if existing.is_empty() || (existing.len() == 1 && model.len() > 1) {
            *existing = model;
        }
    }
    let mut models: Vec<_> = by_id.into_iter().collect();
    models.sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
    Some(Value::Array(
        models
            .into_iter()
            .take(MAX_MODELS)
            .map(|(_, model)| Value::Object(model))
            .collect(),
    ))
}

fn model_text(value: &Value) -> Option<&str> {
    let text = value.as_str()?;
    (!trim(text).is_empty()
        && text.encode_utf16().count() <= 240
        && !text.chars().any(char::is_control))
    .then_some(text)
}

pub(super) fn subscription_model(input: &Value) -> Value {
    let mut model = Map::new();
    copy_present(&mut model, input, &["id"]);
    copy_truthy(
        &mut model,
        input,
        &[
            "displayName",
            "ownedBy",
            "contextWindow",
            "maxContextWindow",
            "supportedEfforts",
            "maxOutputTokens",
        ],
    );
    Value::Object(model)
}

pub(super) fn public_subscription_model(input: &Value) -> Value {
    let mut model = Map::new();
    copy_strings(
        &mut model,
        input,
        &["id", "displayName", "ownedBy", "resolvedModel"],
    );
    for field in TOKEN_FIELDS {
        if input[field].is_number() {
            model.insert(field.into(), input[field].clone());
        }
    }
    if let Some(efforts) = input["supportedEfforts"].as_array() {
        model.insert(
            "supportedEfforts".into(),
            json!(efforts.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
        );
    }
    Value::Object(model)
}
