use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

use serde_json::{Map, Value, json};

use super::ObservableMedia;

const MAX_KEYS: usize = 256;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_SSE_LINE: usize = 256 * 1024;
const MAX_TEXT: usize = 1024;
const MAX_SIGNATURE: usize = 128;
const DEFAULT_WINDOW: u64 = 1_048_576;
const MAX_RESERVED_OUTPUT: u64 = 32_000;

#[derive(Default)]
pub(super) struct Tracker {
    keys: VecDeque<(Key, u64)>,
    compacted: bool,
}

#[derive(PartialEq, Eq)]
enum Key {
    Tool(String),
    Signature(Vec<u16>),
    Text([u8; 16]),
}

#[derive(Default)]
pub(super) struct ContextUsage {
    pub session_window: Option<u64>,
    pub hud_tokens: Option<u64>,
    pub hud_window: Option<u64>,
    pub target_window: Option<u64>,
}

impl Tracker {
    pub(super) fn note_compaction(&mut self) {
        self.compacted = true;
    }

    pub(super) fn attach(&self, message: &mut Map<String, Value>, context: ContextUsage) -> bool {
        let Some(messages) = message.get("messages").and_then(Value::as_array) else {
            return false;
        };
        let Some((last, history)) = messages.split_last() else {
            return false;
        };
        if last["role"] != "user" || has_reminder(&last["content"]) {
            return false;
        }
        let Some(assistant) = history
            .iter()
            .rposition(|entry| entry["role"] == "assistant")
        else {
            return false;
        };

        let mut sample = Vec::new();
        let mut keys = Vec::new();
        if let Some(content) = messages[assistant]["content"].as_str() {
            append_sample(&mut sample, content, MAX_TEXT);
        } else if let Some(blocks) = messages[assistant]["content"].as_array() {
            for block in blocks {
                match block["type"].as_str() {
                    Some("tool_use") => keys.extend(tool_key(&block["id"])),
                    Some("thinking") => keys.extend(signature_key(&block["signature"])),
                    Some("text") => append_text(&mut sample, &block["text"]),
                    _ => {}
                }
            }
        }
        for entry in &messages[assistant + 1..] {
            if entry["role"] == "user"
                && let Some(blocks) = entry["content"].as_array()
            {
                keys.extend(
                    blocks
                        .iter()
                        .filter(|block| block["type"] == "tool_result")
                        .filter_map(|block| tool_key(&block["tool_use_id"])),
                );
            }
        }
        keys.extend(text_key(&sample));

        let used = keys.iter().find_map(|key| {
            self.keys
                .iter()
                .find_map(|(observed, tokens)| (observed == key).then_some(*tokens))
        });
        let Some(used) = used.or_else(|| {
            context
                .hud_tokens
                .filter(|&tokens| tokens > 0 && !self.compacted && has_client_tools(message))
        }) else {
            return false;
        };
        let window = [
            context.session_window,
            context.hud_window,
            context.target_window,
        ]
        .into_iter()
        .flatten()
        .find(|&tokens| tokens > 0)
        .unwrap_or(DEFAULT_WINDOW);
        let reserved = message
            .get("max_tokens")
            .and_then(nonnegative)
            .unwrap_or_default()
            .min(MAX_RESERVED_OUTPUT);
        let total = if window > reserved {
            window - reserved
        } else {
            window
        };
        let remaining = total.saturating_sub(used);
        let reminder = json!({
            "type": "text",
            "text": format!("<system-reminder>\nToken usage: {used}/{total}; {remaining} remaining\n</system-reminder>"),
        });
        let content = &mut message["messages"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap()["content"];
        match content {
            Value::String(text) => {
                let mut blocks = Vec::new();
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                blocks.push(reminder);
                *content = Value::Array(blocks);
            }
            Value::Array(blocks) => blocks.push(reminder),
            _ => return false,
        }
        true
    }

    fn remember(&mut self, keys: impl IntoIterator<Item = Key>, used: u64) {
        for key in keys {
            if let Some(index) = self.keys.iter().position(|(old, _)| old == &key) {
                self.keys.remove(index);
            }
            self.keys.push_back((key, used));
            if self.keys.len() > MAX_KEYS {
                self.keys.pop_front();
            }
        }
    }
}

fn nonnegative(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|number| *number >= 0.0)
            .map(|number| number as u64)
    })
}

fn tool_key(value: &Value) -> Option<Key> {
    Some(Key::Tool(
        value.as_str().filter(|text| !text.is_empty())?.to_owned(),
    ))
}

fn signature_key(value: &Value) -> Option<Key> {
    let signature = value.as_str().filter(|text| !text.is_empty())?;
    Some(Key::Signature(
        signature.encode_utf16().take(MAX_SIGNATURE).collect(),
    ))
}

fn append_sample(sample: &mut Vec<u16>, fragment: &str, limit: usize) {
    sample.extend(fragment.encode_utf16().take(limit - sample.len()));
}

fn append_text(sample: &mut Vec<u16>, value: &Value) {
    if let Some(text) = value.as_str() {
        append_sample(sample, text, MAX_TEXT);
    }
}

fn text_key(sample: &[u16]) -> Option<Key> {
    let text = String::from_utf16_lossy(sample);
    (!text.trim().is_empty()).then(|| Key::Text(text_digest(&text)))
}

#[cfg(unix)]
fn text_digest(text: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())[..16].try_into().unwrap()
}

#[cfg(not(unix))]
fn text_digest(text: &str) -> [u8; 16] {
    // Keys never leave this tracker. Avoid retaining samples on platforms where
    // the runtime does not currently link the SHA-256 crate.
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut digest = [0; 16];
    for (index, half) in digest.chunks_exact_mut(8).enumerate() {
        let mut hash = DefaultHasher::new();
        (index, text).hash(&mut hash);
        half.copy_from_slice(&hash.finish().to_ne_bytes());
    }
    digest
}

fn has_reminder(content: &Value) -> bool {
    static REMINDER: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = REMINDER.get_or_init(|| {
        regex::Regex::new(r"<system-reminder>\s*Token usage:\s*[0-9]+/[0-9]+;").unwrap()
    });
    content.as_str().is_some_and(|text| pattern.is_match(text))
        || content.as_array().is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block["type"] == "text"
                    && block["text"]
                        .as_str()
                        .is_some_and(|text| pattern.is_match(text))
            })
        })
}

fn has_client_tools(message: &Map<String, Value>) -> bool {
    message
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool["name"].is_string()
                    && !tool["type"]
                        .as_str()
                        .is_some_and(|kind| kind.starts_with("web_search_"))
            })
        })
}

pub(super) struct Observer {
    tracker: Arc<Mutex<Tracker>>,
    sse: bool,
    received: usize,
    pending: Vec<u8>,
    usage: [u64; 4],
    tool_ids: Vec<Key>,
    signatures: BTreeMap<u64, Vec<u16>>,
    sample: Vec<u16>,
}

impl Observer {
    pub(super) fn new(media: ObservableMedia, tracker: Arc<Mutex<Tracker>>) -> Self {
        Self {
            tracker,
            sse: media == ObservableMedia::Sse,
            received: 0,
            pending: Vec::new(),
            usage: [0; 4],
            tool_ids: Vec::new(),
            signatures: BTreeMap::new(),
            sample: Vec::new(),
        }
    }

    // Return false to retire observation while continuing the response unchanged.
    pub(super) fn push(&mut self, chunk: &[u8]) -> bool {
        self.received += chunk.len();
        if self.received > MAX_BYTES {
            return false;
        }
        if !self.sse {
            self.pending.extend_from_slice(chunk);
            return true;
        }

        let mut lines = chunk.split(|byte| *byte == b'\n').peekable();
        while let Some(line) = lines.next() {
            self.pending.extend_from_slice(line);
            if lines.peek().is_none() {
                return self.pending.len() <= MAX_SSE_LINE
                    || chunk.contains(&b'\n')
                    || String::from_utf8_lossy(&self.pending)
                        .encode_utf16()
                        .count()
                        <= MAX_SSE_LINE;
            }
            let mut complete = std::mem::take(&mut self.pending);
            self.ingest_line(&complete);
            complete.clear();
            self.pending = complete;
        }
        true
    }

    fn ingest_line(&mut self, line: &[u8]) {
        let line = String::from_utf8_lossy(line);
        let Some(payload) = line.strip_prefix("data:") else {
            return;
        };
        let Ok(event) = serde_json::from_str::<Value>(payload.trim_start()) else {
            return;
        };
        match event["type"].as_str() {
            Some("message_start") => self.apply_usage(&event["message"]["usage"]),
            Some("message_delta") => {
                let usage = if event["usage"].is_object() {
                    &event["usage"]
                } else {
                    &event["delta"]["usage"]
                };
                self.apply_usage(usage);
            }
            Some("content_block_start") => {
                self.ingest_block(&event["content_block"], nonnegative(&event["index"]));
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                match delta["type"].as_str() {
                    Some("signature_delta") => {
                        if let (Some(index), Some(signature)) =
                            (nonnegative(&event["index"]), delta["signature"].as_str())
                        {
                            append_sample(
                                self.signatures.entry(index).or_default(),
                                signature,
                                MAX_SIGNATURE,
                            );
                        }
                    }
                    Some("text_delta") => append_text(&mut self.sample, &delta["text"]),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn apply_usage(&mut self, usage: &Value) {
        for (key, stored) in [
            "input_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
            "output_tokens",
        ]
        .into_iter()
        .zip(&mut self.usage)
        {
            if let Some(tokens) = nonnegative(&usage[key]) {
                *stored = tokens;
            }
        }
    }

    fn ingest_block(&mut self, block: &Value, index: Option<u64>) {
        match block["type"].as_str() {
            Some("tool_use") if self.tool_ids.len() < 16 => {
                self.tool_ids.extend(tool_key(&block["id"]))
            }
            Some("thinking") => {
                if let (Some(index), Some(Key::Signature(signature))) =
                    (index, signature_key(&block["signature"]))
                {
                    self.signatures.insert(index, signature);
                }
            }
            Some("text") => append_text(&mut self.sample, &block["text"]),
            _ => {}
        }
    }

    pub(super) fn finish(mut self) {
        let pending = std::mem::take(&mut self.pending);
        if self.sse {
            self.ingest_line(&pending);
        } else {
            let text = String::from_utf8_lossy(&pending);
            let Ok(message) = serde_json::from_str::<Value>(&text) else {
                return;
            };
            self.apply_usage(&message["usage"]);
            if let Some(text) = message["content"].as_str() {
                append_sample(&mut self.sample, text, MAX_TEXT);
            } else if let Some(blocks) = message["content"].as_array() {
                for (index, block) in blocks.iter().enumerate() {
                    self.ingest_block(block, Some(index as u64));
                }
            }
        }
        let used = self.usage.into_iter().fold(0, u64::saturating_add);
        if used > 0 {
            let keys = self
                .tool_ids
                .into_iter()
                .chain(
                    self.signatures
                        .into_values()
                        .filter(|signature| !signature.is_empty())
                        .map(Key::Signature),
                )
                .chain(text_key(&self.sample));
            self.tracker.lock().unwrap().remember(keys, used);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn continuation(content: Value) -> Map<String, Value> {
        json!({
            "max_tokens": 90_000,
            "tools": [{"name":"Bash"}],
            "messages": [
                {"role":"assistant","content":"previous answer"},
                {"role":"user","content":content}
            ]
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn hud_fallback_obeys_window_precedence_deduplication_and_compaction() {
        let mut tracker = Tracker::default();
        let context = || ContextUsage {
            session_window: Some(872_000),
            hud_tokens: Some(417_776),
            hud_window: Some(512_000),
            target_window: Some(1_048_576),
        };
        let mut request = continuation(json!("继续"));
        assert!(tracker.attach(&mut request, context()));
        assert_eq!(
            request["messages"][1]["content"],
            json!([
                {"type":"text","text":"继续"},
                {"type":"text","text":"<system-reminder>\nToken usage: 417776/840000; 422224 remaining\n</system-reminder>"}
            ])
        );
        assert!(!tracker.attach(&mut request, context()));
        let mut utility = continuation(json!("title"));
        utility.insert(
            "messages".into(),
            json!([{"role":"user","content":"title"}]),
        );
        assert!(!tracker.attach(&mut utility, context()));
        let mut search = continuation(json!("search"));
        search.insert(
            "tools".into(),
            json!([{"type":"web_search_20250305","name":"web_search"}]),
        );
        assert!(!tracker.attach(&mut search, context()));

        tracker.note_compaction();
        let mut request = continuation(json!(""));
        assert!(!tracker.attach(&mut request, context()));
        tracker.remember(
            text_key(&"previous answer".encode_utf16().collect::<Vec<_>>()),
            950,
        );
        assert!(tracker.attach(
            &mut request,
            ContextUsage {
                target_window: Some(800),
                ..ContextUsage::default()
            }
        ));
        assert_eq!(
            request["messages"][1]["content"],
            json!([
                {"type":"text","text":"<system-reminder>\nToken usage: 950/800; 0 remaining\n</system-reminder>"}
            ])
        );
    }

    #[test]
    fn observed_keys_are_bounded_and_updated_keys_are_retained() {
        let mut tracker = Tracker::default();
        for index in 0..MAX_KEYS {
            tracker.remember([Key::Tool(index.to_string())], 1);
        }
        tracker.remember([Key::Tool("0".into()), Key::Tool("new".into())], 7);
        assert_eq!(tracker.keys.len(), MAX_KEYS);
        assert!(
            tracker
                .keys
                .iter()
                .any(|(key, tokens)| key == &Key::Tool("0".into()) && *tokens == 7)
        );
        assert!(
            !tracker
                .keys
                .iter()
                .any(|(key, _)| key == &Key::Tool("1".into()))
        );
    }

    #[test]
    fn bounds_observation_without_parsing_an_oversized_body_or_unterminated_line() {
        let tracker = Arc::new(Mutex::new(Tracker::default()));
        let mut observer = Observer::new(ObservableMedia::Sse, Arc::clone(&tracker));
        assert!(observer.push(&vec![b'x'; MAX_SSE_LINE]));
        assert!(!observer.push(b"x"));
        let mut observer = Observer::new(ObservableMedia::Json, tracker);
        assert!(!observer.push(&vec![b'x'; MAX_BYTES + 1]));
    }
}
