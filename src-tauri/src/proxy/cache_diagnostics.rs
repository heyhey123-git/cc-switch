//! Bounded, content-free evidence for Claude prompt-prefix changes.
//! Never stores prompts, credentials, tool arguments, images, or thinking text.

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

type Hash = [u8; 32];

struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn hash(value: &Value) -> Hash {
    let mut writer = HashWriter(Sha256::new());
    // Object keys are normalized by serde_json::Map. Stream large image strings
    // directly into the digest rather than creating a second prompt buffer.
    value
        .serialize(&mut serde_json::Serializer::new(&mut writer))
        .expect("JSON hash");
    writer.0.finalize().into()
}

fn block_hash(role: &str, value: &Value) -> Hash {
    let mut writer = HashWriter(Sha256::new());
    writer.0.update(role.as_bytes());
    if let Some(object) = value.as_object() {
        // Only the block's own marker moves. A schema/argument field literally
        // named cache_control remains part of its hash.
        for (key, value) in object {
            if key == "cache_control" {
                continue;
            }
            key.serialize(&mut serde_json::Serializer::new(&mut writer))
                .expect("JSON key hash");
            value
                .serialize(&mut serde_json::Serializer::new(&mut writer))
                .expect("JSON value hash");
        }
    } else {
        value
            .serialize(&mut serde_json::Serializer::new(&mut writer))
            .expect("JSON block hash");
    }
    writer.0.finalize().into()
}

#[derive(Clone)]
struct Snapshot {
    at: Instant,
    tools: Vec<Hash>,
    system: Vec<Hash>,
    settings: Hash,
    messages: Vec<Hash>,
    positions: Vec<(usize, usize)>,
    breakpoints: Vec<usize>,
    total_blocks: usize,
    thinking_blocks: usize,
}

impl Snapshot {
    fn new(body: &Value) -> Self {
        let mut messages = Vec::new();
        let mut positions = Vec::new();
        let mut total_blocks = 0;
        let mut thinking_blocks = 0;
        if let Some(items) = body.get("messages").and_then(Value::as_array) {
            for (message_index, message) in items.iter().enumerate() {
                let role = message["role"].as_str().unwrap_or("");
                if let Some(blocks) = message["content"].as_array() {
                    for (block_index, block) in blocks.iter().enumerate() {
                        if matches!(
                            block["type"].as_str(),
                            Some("thinking" | "redacted_thinking")
                        ) {
                            thinking_blocks += 1;
                        }
                        total_blocks += 1;
                        if messages.len() < 8192 {
                            messages.push(block_hash(role, block));
                            positions.push((message_index, block_index));
                        }
                    }
                } else {
                    total_blocks += 1;
                    if messages.len() < 8192 {
                        messages.push(block_hash(role, &message["content"]));
                        positions.push((message_index, usize::MAX));
                    }
                }
            }
        }
        let blocks = |field: &str| match body.get(field) {
            Some(Value::Array(values)) => values.iter().map(|v| block_hash(field, v)).collect(),
            Some(value) => vec![block_hash(field, value)],
            None => Vec::new(),
        };
        Self {
            at: Instant::now(),
            tools: blocks("tools"),
            system: blocks("system"),
            settings: hash(&json!({"model":body["model"],"thinking":body["thinking"],
                "output_config":body["output_config"],"tool_choice":body["tool_choice"],"max_tokens":body["max_tokens"]})),
            messages,
            positions,
            breakpoints: Vec::new(),
            total_blocks,
            thinking_blocks,
        }
    }

    fn shared_blocks(&self, previous: &Self) -> usize {
        self.messages
            .iter()
            .zip(&previous.messages)
            .take_while(|(a, b)| a == b)
            .count()
    }

    fn previous_anchor(&self, previous: &Self) -> Option<(usize, usize)> {
        if self.tools != previous.tools
            || self.system != previous.system
            || self.settings != previous.settings
            || previous.at.elapsed().as_secs() >= 300
        {
            return None;
        }
        let shared = self.shared_blocks(previous);
        previous.breakpoints.iter().rev().find_map(|index| {
            (*index < shared)
                .then(|| self.positions.get(*index).copied())
                .flatten()
                .filter(|(_, block)| *block != usize::MAX)
        })
    }

    fn update_breakpoints(&mut self, body: &Value) {
        self.breakpoints.clear();
        for (index, (message, block)) in self.positions.iter().enumerate() {
            if body["messages"][*message]["content"]
                .get(*block)
                .and_then(|b| b.get("cache_control"))
                .is_some()
            {
                if self.breakpoints.len() == 16 {
                    self.breakpoints.remove(0);
                }
                self.breakpoints.push(index);
            }
        }
    }
}

pub(crate) fn inject_and_record(
    provider: &str,
    session: &str,
    body: &mut Value,
    config: &super::types::OptimizerConfig,
) {
    if session.is_empty() {
        super::cache_injector::inject(body, config);
        return;
    }
    static HISTORY: OnceLock<Mutex<HashMap<Hash, Snapshot>>> = OnceLock::new();
    let key = hash(&json!([provider, session]));
    let label = key[..6]
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    let mut current = Snapshot::new(body);
    let Ok(mut history) = HISTORY.get_or_init(|| Mutex::new(HashMap::new())).lock() else {
        super::cache_injector::inject(body, config);
        return;
    };
    let anchor = history
        .get(&key)
        .and_then(|previous| current.previous_anchor(previous));
    super::cache_injector::inject_with_anchor(body, config, anchor);
    current.update_breakpoints(body);
    if let Some(previous) = history.get(&key) {
        log::info!("[CACHE_PREFIX] session={label} tools_changed={} system_changed={} settings_changed={} shared_blocks={} previous_blocks={} current_blocks={} thinking_blocks={}->{} elapsed_ms={}",
            previous.tools!=current.tools,previous.system!=current.system,previous.settings!=current.settings,
            current.shared_blocks(previous),previous.total_blocks,current.total_blocks,
            previous.thinking_blocks,current.thinking_blocks,previous.at.elapsed().as_millis());
    } else {
        log::info!(
            "[CACHE_PREFIX] session={label} initial=true blocks={} thinking_blocks={}",
            current.total_blocks,
            current.thinking_blocks
        );
    }
    if history.len() >= 128 && !history.contains_key(&key) {
        if let Some(oldest) = history.iter().min_by_key(|(_, v)| v.at).map(|(k, _)| *k) {
            history.remove(&oldest);
        }
    }
    history.insert(key, current);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn optimizer() -> super::super::types::OptimizerConfig {
        super::super::types::OptimizerConfig {
            enabled: true,
            thinking_optimizer: true,
            cache_injection: true,
        }
    }

    #[test]
    fn preserves_prior_write_point_beyond_twenty_block_lookback() {
        let mut body = json!({"model":"claude-opus-5-5","tools":[{"name":"read"}],"system":[{"type":"text","text":"stable"}],
            "messages":[{"role":"user","content":[{"type":"text","text":"long stable history"}]}]});
        inject_and_record("fixture-anchor", "same-session", &mut body, &optimizer());
        let mut next = body.clone();
        next["messages"][0]["content"][0]
            .as_object_mut()
            .unwrap()
            .remove("cache_control");
        next["tools"][0]
            .as_object_mut()
            .unwrap()
            .remove("cache_control");
        next["system"][0]
            .as_object_mut()
            .unwrap()
            .remove("cache_control");
        for n in 0..45 {
            next["messages"][0]["content"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"text","text":format!("appended {n}")}));
        }
        inject_and_record("fixture-anchor", "same-session", &mut next, &optimizer());
        let blocks = next["messages"][0]["content"].as_array().unwrap();
        assert!(blocks[0].get("cache_control").is_some());
        assert!(blocks.last().unwrap().get("cache_control").is_some());
        assert_eq!(
            blocks
                .iter()
                .filter(|b| b.get("cache_control").is_some())
                .count(),
            2
        );
        assert_eq!(blocks[0]["text"], body["messages"][0]["content"][0]["text"]);
        let mut changed = Snapshot::new(&next);
        let mut previous = Snapshot::new(&body);
        previous.update_breakpoints(&body);
        assert_eq!(changed.previous_anchor(&previous), Some((0, 0)));
        changed.system.clear();
        assert_eq!(changed.previous_anchor(&previous), None);
        let mut unrelated = Snapshot::new(&next);
        unrelated.messages[0] = hash(&json!("changed old history"));
        assert_eq!(unrelated.previous_anchor(&previous), None);
    }

    #[test]
    fn never_reuses_cross_session_or_thinking_breakpoint() {
        let mut one = json!({"model":"claude-opus-5-5","messages":[{"role":"user","content":[{"type":"text","text":"base"}]}]});
        inject_and_record("fixture-isolation", "first-session", &mut one, &optimizer());
        let mut two = json!({"model":"claude-opus-5-5","messages":[{"role":"user","content":[{"type":"text","text":"base"},{"type":"text","text":"new"}]},
            {"role":"assistant","content":[{"type":"thinking","thinking":"private","signature":"private"}]}]});
        inject_and_record(
            "fixture-isolation",
            "second-session",
            &mut two,
            &optimizer(),
        );
        assert!(two["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(two["messages"][0]["content"][1]
            .get("cache_control")
            .is_some());
        assert!(two["messages"][1]["content"][0]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn distinguishes_tool_system_settings_history_and_append_without_content() {
        let base = json!({"model":"claude-opus-5-5","tools":[{"name":"read","input_schema":{"type":"object"}}],
            "system":[{"type":"text","text":"operator"}],"thinking":{"type":"adaptive"},
            "messages":[{"role":"user","content":[{"type":"text","text":"PRIVATE"}]}]});
        let first = Snapshot::new(&base);
        let mut next = base.clone();
        next["messages"][0]["content"][0]["cache_control"] = json!({"type":"ephemeral"});
        next["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"assistant","content":[{"type":"text","text":"reply"}]}));
        let appended = Snapshot::new(&next);
        assert_eq!(appended.shared_blocks(&first), 1);
        assert_eq!(appended.tools, first.tools);
        assert_eq!(appended.system, first.system);
        assert_eq!(appended.settings, first.settings);
        next["tools"][0]["input_schema"]["properties"] = json!({"cache_control":{"type":"string"}});
        assert_ne!(Snapshot::new(&next).tools, first.tools);
        next["system"][0]["text"] = json!("changed");
        assert_ne!(Snapshot::new(&next).system, first.system);
        next["thinking"]["type"] = json!("disabled");
        assert_ne!(Snapshot::new(&next).settings, first.settings);
        next["messages"][0]["content"][0]["text"] = json!("different");
        assert_eq!(Snapshot::new(&next).shared_blocks(&first), 0);
    }
}
