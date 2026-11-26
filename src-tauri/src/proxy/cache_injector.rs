//! Cache 断点注入器
//!
//! 在请求转发前自动注入 cache_control 标记，启用 Bedrock Prompt Caching

use super::types::{CacheTtlMode, OptimizerConfig};
use serde_json::{json, Value};

/// 在请求体关键位置注入 cache_control 断点
pub fn inject(body: &mut Value, config: &OptimizerConfig) {
    inject_with_anchor(body, config, None);
}

/// Positions are (message index, content block index).
#[derive(Clone, Copy, Default)]
pub(crate) struct CacheAnchors {
    /// Newest breakpoint of the previous accepted request (5m tail).
    pub previous: Option<(usize, usize)>,
    /// 1h long-history breakpoint of the previous accepted request (mixed mode).
    pub long: Option<(usize, usize)>,
}

pub(crate) fn inject_with_anchor(
    body: &mut Value,
    config: &OptimizerConfig,
    previous_anchor: Option<(usize, usize)>,
) {
    inject_with_anchors(
        body,
        config,
        CacheAnchors {
            previous: previous_anchor,
            long: None,
        },
    );
}

pub(crate) fn inject_with_anchors(
    body: &mut Value,
    config: &OptimizerConfig,
    anchors: CacheAnchors,
) {
    let previous_anchor = anchors.previous;
    if !config.enabled || !config.cache_injection {
        return;
    }

    let existing = count_existing(body);

    if existing > 4 {
        // Existing markers are caller-owned. Do not silently delete or reorder
        // them; surface the invalid/unsupported total and leave validation to
        // the upstream provider.
        log::warn!(
            "[OPT] cache: existing breakpoint count {existing} exceeds the supported total of 4; preserving caller input"
        );
    }

    if config.cache_ttl_mode == CacheTtlMode::Mixed && existing == 0 {
        if inject_mixed(body, config, anchors) {
            return;
        }
        // Context too small for a 1h anchor: behave exactly like 5m mode.
    }
    let ttl = match config.cache_ttl_mode {
        CacheTtlMode::OneHour => Some("1h"),
        _ => None,
    };

    let mut budget = 4_usize.saturating_sub(existing);
    if budget == 0 {
        log::info!("[OPT] cache: no-op(existing={existing})");
        return;
    }

    let mut injected = Vec::new();

    // (a) tools 末尾
    if budget > 0 {
        if let Some(tools) = body.get_mut("tools").and_then(|t| t.as_array_mut()) {
            if let Some(last) = tools.last_mut() {
                if last.get("cache_control").is_none() {
                    if let Some(o) = last.as_object_mut() {
                        o.insert("cache_control".to_string(), make_cache_control(ttl));
                    }
                    budget -= 1;
                    injected.push("tools");
                }
            }
        }
    }

    // (b) system 末尾
    if budget > 0 {
        // 字符串 system → 转为数组
        if let Some(text) = body
            .get("system")
            .and_then(|s| s.as_str())
            .map(str::to_string)
        {
            body["system"] = json!([{"type": "text", "text": text}]);
        }

        if let Some(system) = body.get_mut("system").and_then(|s| s.as_array_mut()) {
            if let Some(last) = system.last_mut() {
                if last.get("cache_control").is_none() {
                    if let Some(o) = last.as_object_mut() {
                        o.insert("cache_control".to_string(), make_cache_control(ttl));
                    }
                    budget -= 1;
                    injected.push("system");
                }
            }
        }
    }

    // (c) 最后一条可缓存消息的最后一个非 thinking block。工具循环通常以
    // user/tool_result 结束；只标 assistant 会让最新稳定前缀无法命中缓存。
    if budget > 0 {
        if let Some(messages) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
            for message in messages.iter_mut().rev() {
                if inject_message_breakpoint(message, ttl) {
                    budget -= 1;
                    injected.push("msgs-latest");
                    break;
                }
            }

            // Preserve the exact prior request's write point when that entire
            // prefix is unchanged. A large appended tail can exceed the API's
            // 20-block lookback even when no historical content changed.
            if budget > 0 {
                if let Some((message_index, block_index)) = previous_anchor {
                    if let Some(block) = messages
                        .get_mut(message_index)
                        .and_then(|message| message.get_mut("content"))
                        .and_then(Value::as_array_mut)
                        .and_then(|blocks| blocks.get_mut(block_index))
                    {
                        if inject_block_breakpoint(block, ttl) {
                            budget -= 1;
                            injected.push("msgs-previous-request");
                        }
                    }
                }
            }

            // (d) A second, older user anchor helps long tool-result turns where
            // the stable prefix falls outside Anthropic's 20-block lookback from
            // the newest breakpoint. Keep this best-effort and inside the 4-BP cap.
            if budget > 0 && messages.len() >= 4 {
                let mut user_count = 0;
                for message in messages.iter_mut().rev() {
                    if message.get("role").and_then(Value::as_str) != Some("user") {
                        continue;
                    }
                    user_count += 1;
                    if user_count == 2 {
                        if inject_message_breakpoint(message, ttl) {
                            injected.push("msgs-prior-user");
                        }
                        break;
                    }
                }
            }
        }
    }

    log::info!(
        "[OPT] cache: {}bp({},{},pre={existing})",
        injected.len(),
        injected.join("+"),
        ttl.unwrap_or("5m"),
    );
}

fn is_thinking(block: &Value) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("thinking" | "redacted_thinking")
    )
}

/// Cheap content-size estimate in characters; images and documents count as a
/// fixed size because their token cost is not proportional to base64 length.
fn estimate_block_chars(block: &Value) -> usize {
    match block {
        Value::String(text) => text.len(),
        Value::Array(items) => items.iter().map(estimate_block_chars).sum(),
        Value::Object(object) => match object.get("type").and_then(Value::as_str) {
            Some("text") => object
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, str::len),
            Some("thinking") => object
                .get("thinking")
                .and_then(Value::as_str)
                .map_or(0, str::len),
            Some("image" | "document") => 8_000,
            Some("tool_result") => object.get("content").map_or(0, estimate_block_chars),
            Some("tool_use") => {
                object
                    .get("input")
                    .map_or(0, |input| input.to_string().len())
                    + 100
            }
            _ => 200,
        },
        _ => 0,
    }
}

fn message_blocks(message: &Value) -> Vec<&Value> {
    match message.get("content") {
        Some(Value::Array(blocks)) => blocks.iter().collect(),
        Some(other) => vec![other],
        None => Vec::new(),
    }
}

fn prefix_fixed_chars(body: &Value) -> usize {
    let mut chars = 0;
    for field in ["tools", "system"] {
        if let Some(value) = body.get(field) {
            chars += estimate_block_chars(value).max(match value {
                Value::Array(items) => items.iter().map(|v| v.to_string().len()).sum(),
                _ => 0,
            });
        }
    }
    chars
}

/// Mixed TTL layout: system(1h, covers tools) + long history anchor(1h) followed
/// by 5m previous-request / latest breakpoints. Returns false (nothing changed)
/// when the context is too small to justify a 1h anchor.
fn inject_mixed(body: &mut Value, config: &OptimizerConfig, anchors: CacheAnchors) -> bool {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return false;
    };
    // Positions of eligible (non-thinking, array-content) blocks with cumulative size.
    let mut points: Vec<((usize, usize), usize)> = Vec::new();
    let mut cumulative = prefix_fixed_chars(body);
    let mut latest_pos = None;
    for (mi, message) in messages.iter().enumerate() {
        let blocks = message_blocks(message);
        let is_array = message.get("content").map_or(false, Value::is_array);
        for (bi, block) in blocks.iter().enumerate() {
            cumulative += estimate_block_chars(block);
            if is_array && !is_thinking(block) && block.is_object() {
                points.push(((mi, bi), cumulative));
                latest_pos = Some((mi, bi));
            }
        }
    }
    let total = cumulative;
    let Some(latest) = latest_pos else {
        return false;
    };
    if total < config.cache_long_min_chars {
        return false;
    }
    let cum_at = |pos: (usize, usize)| points.iter().find(|(p, _)| *p == pos).map(|(_, c)| *c);

    // Newest eligible point that still leaves >= tail_keep chars after it.
    let target = points
        .iter()
        .rev()
        .find(|(pos, c)| *pos < latest && total - *c >= config.cache_tail_keep_chars)
        .map(|(pos, c)| (*pos, *c));

    let long = match (anchors.long.and_then(|p| cum_at(p).map(|c| (p, c))), target) {
        (Some((prev, prev_c)), Some((t, t_c))) => {
            if t > prev && t_c - prev_c >= config.cache_long_advance_chars {
                Some(t)
            } else {
                Some(prev)
            }
        }
        (Some((prev, _)), None) => Some(prev),
        (None, Some((t, _))) => Some(t),
        (None, None) => None,
    };
    let Some(long) = long else {
        return false;
    };

    let mut injected = Vec::new();
    let one_hour = Some("1h");
    // system (preferred, covers tools) else last tool, both at 1h.
    if let Some(text) = body
        .get("system")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        body["system"] = json!([{"type": "text", "text": text}]);
    }
    let mut prefix_marked = false;
    if let Some(last) = body
        .get_mut("system")
        .and_then(Value::as_array_mut)
        .and_then(|v| v.last_mut())
    {
        if inject_block_breakpoint(last, one_hour) {
            prefix_marked = true;
            injected.push("system:1h");
        }
    }
    if !prefix_marked {
        if let Some(last) = body
            .get_mut("tools")
            .and_then(Value::as_array_mut)
            .and_then(|v| v.last_mut())
        {
            if inject_block_breakpoint(last, one_hour) {
                injected.push("tools:1h");
            }
        }
    }
    let mut mark =
        |body: &mut Value, pos: (usize, usize), ttl: Option<&str>, label: &'static str| {
            if let Some(block) = body
                .get_mut("messages")
                .and_then(|m| m.get_mut(pos.0))
                .and_then(|m| m.get_mut("content"))
                .and_then(Value::as_array_mut)
                .and_then(|b| b.get_mut(pos.1))
            {
                if inject_block_breakpoint(block, ttl) {
                    injected.push(label);
                }
            }
        };
    mark(body, long, one_hour, "msgs-long:1h");
    if let Some(prev) = anchors.previous {
        if prev > long && prev < latest {
            mark(body, prev, None, "msgs-previous-request:5m");
        }
    }
    mark(body, latest, None, "msgs-latest:5m");
    log::info!(
        "[OPT] cache: {}bp({},mixed,chars={total},long=({},{}))",
        injected.len(),
        injected.join("+"),
        long.0,
        long.1
    );
    true
}

fn inject_message_breakpoint(message: &mut Value, ttl: Option<&str>) -> bool {
    let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
        return false;
    };
    let Some(block) = content.iter_mut().rev().find(|block| {
        !matches!(
            block.get("type").and_then(Value::as_str),
            Some("thinking" | "redacted_thinking")
        )
    }) else {
        return false;
    };
    inject_block_breakpoint(block, ttl)
}

fn inject_block_breakpoint(block: &mut Value, ttl: Option<&str>) -> bool {
    if matches!(
        block.get("type").and_then(Value::as_str),
        Some("thinking" | "redacted_thinking")
    ) || block.get("cache_control").is_some()
    {
        return false;
    }
    let Some(object) = block.as_object_mut() else {
        return false;
    };
    object.insert("cache_control".to_string(), make_cache_control(ttl));
    true
}

fn make_cache_control(ttl: Option<&str>) -> Value {
    match ttl {
        Some(ttl) => json!({"type": "ephemeral", "ttl": ttl}),
        None => json!({"type": "ephemeral"}),
    }
}

fn count_existing(body: &Value) -> usize {
    let mut count = 0;

    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        count += tools
            .iter()
            .filter(|t| t.get("cache_control").is_some())
            .count();
    }

    if let Some(system) = body.get("system").and_then(|s| s.as_array()) {
        count += system
            .iter()
            .filter(|b| b.get("cache_control").is_some())
            .count();
    }

    if let Some(messages) = body.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
                count += content
                    .iter()
                    .filter(|b| b.get("cache_control").is_some())
                    .count();
            }
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn default_config() -> OptimizerConfig {
        OptimizerConfig {
            enabled: true,
            thinking_optimizer: true,
            cache_injection: true,
            ..OptimizerConfig::default()
        }
    }

    #[test]
    fn test_single_user_text_block_gets_cache_control() {
        let mut body = json!({"model": "test", "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]});
        inject(&mut body, &default_config());
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn test_inject_three_breakpoints() {
        let mut body = json!({
            "model": "test",
            "tools": [{"name": "tool1"}, {"name": "tool2"}],
            "system": [{"type": "text", "text": "sys prompt"}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hi"}]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "hello"}
                ]}
            ]
        });

        inject(&mut body, &default_config());

        // tools last element
        assert!(body["tools"][1].get("cache_control").is_some());
        assert!(body["tools"][1]["cache_control"].get("ttl").is_none());
        // system last element
        assert!(body["system"][0].get("cache_control").is_some());
        // assistant last non-thinking block
        assert!(body["messages"][1]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn test_long_history_uses_fourth_prior_user_breakpoint() {
        let mut body = json!({
            "model":"test",
            "tools":[{"name":"tool1"}],
            "system":[{"type":"text","text":"sys"}],
            "messages":[
                {"role":"user","content":[{"type":"text","text":"first"}]},
                {"role":"assistant","content":[{"type":"text","text":"answer"}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"result"}]},
                {"role":"assistant","content":[{"type":"text","text":"latest"}]}
            ]
        });

        inject(&mut body, &default_config());
        assert_eq!(count_existing(&body), 4);
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
        assert!(body["messages"][3]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn test_existing_four_breakpoints_preserve_caller_ttl() {
        let mut body = json!({
            "model": "test",
            "tools": [
                {"name": "t1", "cache_control": {"type": "ephemeral", "ttl": "1h"}},
                {"name": "t2", "cache_control": {"type": "ephemeral", "ttl": "1h"}}
            ],
            "system": [
                {"type": "text", "text": "sys", "cache_control": {"type": "ephemeral", "ttl": "1h"}}
            ],
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "text", "text": "ok", "cache_control": {"type": "ephemeral", "ttl": "1h"}}
                ]}
            ]
        });

        inject(&mut body, &default_config());

        // Existing markers are caller-owned; only newly injected markers are fixed to 5m.
        assert_eq!(body["tools"][0]["cache_control"]["ttl"], "1h");
        assert_eq!(body["tools"][1]["cache_control"]["ttl"], "1h");
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"]["ttl"],
            "1h"
        );
    }

    #[test]
    fn test_existing_two_injects_two_more() {
        let mut body = json!({
            "model": "test",
            "tools": [
                {"name": "t1", "cache_control": {"type": "ephemeral"}},
                {"name": "t2", "cache_control": {"type": "ephemeral"}}
            ],
            "system": [{"type": "text", "text": "sys"}],
            "messages": [
                {"role": "assistant", "content": [{"type": "text", "text": "ok"}]}
            ]
        });

        inject(&mut body, &default_config());

        // budget = 4 - 2 = 2, inject system + msgs
        assert!(body["system"][0].get("cache_control").is_some());
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_some());
    }

    #[test]
    fn test_more_than_four_existing_breakpoints_are_preserved() {
        let mut body = json!({
            "model": "test",
            "tools": [
                {"name": "t1", "cache_control": {"type": "ephemeral"}},
                {"name": "t2", "cache_control": {"type": "ephemeral"}}
            ],
            "system": [
                {"type": "text", "text": "s1", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "s2", "cache_control": {"type": "ephemeral"}}
            ],
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "m1", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "m2"}
            ]}]
        });

        inject(&mut body, &default_config());

        assert_eq!(count_existing(&body), 5);
        assert!(body["messages"][0]["content"][1]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn test_system_string_converted_to_array() {
        let mut body = json!({
            "model": "test",
            "system": "You are a helpful assistant",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
        });

        inject(&mut body, &default_config());

        assert!(body["system"].is_array());
        let sys = body["system"].as_array().unwrap();
        assert_eq!(sys.len(), 1);
        assert_eq!(sys[0]["type"], "text");
        assert_eq!(sys[0]["text"], "You are a helpful assistant");
        assert!(sys[0].get("cache_control").is_some());
    }

    #[test]
    fn test_standard_five_minute_cache_control_omits_ttl() {
        let mut body = json!({
            "model": "test",
            "tools": [{"name": "tool1"}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
        });

        inject(&mut body, &default_config());

        let cc = &body["tools"][0]["cache_control"];
        assert_eq!(cc["type"], "ephemeral");
        assert!(cc.get("ttl").is_none() || cc["ttl"].is_null());
    }

    #[test]
    fn test_disabled_no_change() {
        let config = OptimizerConfig {
            cache_injection: false,
            ..default_config()
        };
        let mut body = json!({
            "model": "test",
            "tools": [{"name": "tool1"}],
            "system": [{"type": "text", "text": "sys"}],
            "messages": [{"role": "assistant", "content": [{"type": "text", "text": "ok"}]}]
        });
        let original = body.clone();

        inject(&mut body, &config);

        assert_eq!(body, original);
    }

    #[test]
    fn test_optimizer_disabled_no_change() {
        let config = OptimizerConfig {
            enabled: false,
            cache_injection: true,
            ..default_config()
        };
        let mut body = json!({
            "model":"test",
            "tools":[{"name":"tool1"}],
            "messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]
        });
        let original = body.clone();

        inject(&mut body, &config);
        assert_eq!(body, original);
    }

    #[test]
    fn test_skip_thinking_blocks_in_assistant() {
        let mut body = json!({
            "model": "test",
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "hmm"},
                    {"type": "text", "text": "result"},
                    {"type": "redacted_thinking", "data": "xxx"}
                ]}
            ]
        });

        inject(&mut body, &default_config());

        // Should inject on "text" block (last non-thinking), not on thinking/redacted_thinking
        assert!(body["messages"][0]["content"][1]
            .get("cache_control")
            .is_some());
        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(body["messages"][0]["content"][2]
            .get("cache_control")
            .is_none());
    }

    #[test]
    fn test_injects_latest_tool_result_instead_of_older_assistant() {
        let mut body = json!({
            "messages": [
                {"role": "assistant", "content": [{"type": "tool_use", "id": "call_1", "name": "Read", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "done"}]}
            ]
        });

        inject(&mut body, &default_config());

        assert!(body["messages"][0]["content"][0]
            .get("cache_control")
            .is_none());
        assert!(body["messages"][1]["content"][0]
            .get("cache_control")
            .is_some());
    }

    fn mixed_config() -> OptimizerConfig {
        OptimizerConfig {
            enabled: true,
            cache_ttl_mode: CacheTtlMode::Mixed,
            cache_long_min_chars: 1_000,
            cache_long_advance_chars: 2_000,
            cache_tail_keep_chars: 500,
            ..OptimizerConfig::default()
        }
    }

    fn turns(n: usize, size: usize) -> Value {
        let mut messages = Vec::new();
        for i in 0..n {
            messages.push(json!({"role":"user","content":[{"type":"text","text":format!("{i}{}", "u".repeat(size))}]}));
            messages.push(
                json!({"role":"assistant","content":[{"type":"text","text":"a".repeat(size)}]}),
            );
        }
        messages.push(json!({"role":"user","content":[{"type":"text","text":"now"}]}));
        json!({"tools":[{"name":"t"}],"system":[{"type":"text","text":"sys"}],"messages":messages})
    }

    /// TTL of every marker in wire order: tools, system, messages.
    fn wire_ttls(body: &Value) -> Vec<String> {
        let mut out = Vec::new();
        let mut visit = |v: &Value| {
            if let Some(cc) = v.get("cache_control") {
                out.push(
                    cc.get("ttl")
                        .and_then(Value::as_str)
                        .unwrap_or("5m")
                        .to_string(),
                );
            }
        };
        for f in ["tools", "system"] {
            for b in body[f].as_array().into_iter().flatten() {
                visit(b);
            }
        }
        for m in body["messages"].as_array().into_iter().flatten() {
            for b in m["content"].as_array().into_iter().flatten() {
                visit(b);
            }
        }
        out
    }

    #[test]
    fn mixed_small_context_matches_five_minute_mode() {
        let mut mixed = turns(1, 10);
        let mut plain = mixed.clone();
        inject(&mut mixed, &mixed_config());
        inject(&mut plain, &default_config());
        assert_eq!(mixed, plain);
        assert!(wire_ttls(&mixed).iter().all(|t| t == "5m"));
    }

    #[test]
    fn mixed_large_context_orders_one_hour_before_five_minute() {
        let mut body = turns(6, 800);
        inject(&mut body, &mixed_config());
        let ttls = wire_ttls(&body);
        assert!(ttls.len() <= 4);
        let first_5m = ttls.iter().position(|t| t == "5m").unwrap();
        assert!(ttls[..first_5m].iter().all(|t| t == "1h"));
        assert!(ttls[first_5m..].iter().all(|t| t == "5m"));
        assert_eq!(ttls.iter().filter(|t| *t == "1h").count(), 2);
        // The system marker is 1h and covers tools; tools stay unmarked.
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");
        assert!(body["tools"][0].get("cache_control").is_none());
        // The newest block is a 5m tail marker.
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert!(last["content"][0]["cache_control"].get("ttl").is_none());
    }

    #[test]
    fn mixed_long_anchor_is_kept_until_growth_threshold() {
        let mut first = turns(6, 800);
        inject(&mut first, &mixed_config());
        let long_pos = |b: &Value| {
            b["messages"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .find_map(|(i, m)| {
                    (m["content"][0]["cache_control"]["ttl"] == "1h").then_some((i, 0))
                })
        };
        let anchor = long_pos(&first).unwrap();

        let mut small_growth = turns(6, 800);
        small_growth["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"assistant","content":[{"type":"text","text":"x".repeat(600)}]}));
        small_growth["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":[{"type":"text","text":"more"}]}));
        inject_with_anchors(
            &mut small_growth,
            &mixed_config(),
            CacheAnchors {
                previous: None,
                long: Some(anchor),
            },
        );
        assert_eq!(long_pos(&small_growth), Some(anchor));

        let mut big_growth = turns(12, 800);
        inject_with_anchors(
            &mut big_growth,
            &mixed_config(),
            CacheAnchors {
                previous: None,
                long: Some(anchor),
            },
        );
        assert!(long_pos(&big_growth).unwrap() > anchor);
    }

    #[test]
    fn mixed_skips_when_caller_supplied_markers() {
        let mut body = turns(6, 800);
        body["messages"][0]["content"][0]["cache_control"] = json!({"type":"ephemeral","ttl":"1h"});
        inject(&mut body, &mixed_config());
        assert!(!wire_ttls(&body).iter().any(|_| false));
        assert_eq!(wire_ttls(&body).iter().filter(|t| *t == "1h").count(), 1);
    }

    #[test]
    fn one_hour_mode_marks_every_injected_breakpoint() {
        let mut body = turns(1, 10);
        let config = OptimizerConfig {
            cache_ttl_mode: CacheTtlMode::OneHour,
            ..default_config()
        };
        inject(&mut body, &config);
        let ttls = wire_ttls(&body);
        assert!(ttls.len() >= 3 && ttls.iter().all(|t| t == "1h"));
    }
}
