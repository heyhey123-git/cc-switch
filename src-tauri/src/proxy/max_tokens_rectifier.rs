//! Output-token ceiling rectifier for the Codex → Anthropic bridge.
//!
//! A provider's `maxOutputTokens` (or Anthropic's default) can exceed what one upstream
//! model accepts; Anthropic then rejects the request with
//! `max_tokens: 70000 > 64000, which is the maximum allowed number of output tokens for …`.
//! That is a validation error (nothing is billed). Instead of failing every request until
//! someone edits the setting, remember the stated ceiling for the provider and retry once.

use regex::Regex;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const MIN_PLAUSIBLE_LIMIT: u64 = 1024;
const MAX_PLAUSIBLE_LIMIT: u64 = 1_000_000;

static LEARNED: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

/// Extracts the maximum allowed output tokens from an upstream rejection, if that is
/// what the error is about. Returns `None` for every other kind of error.
pub(crate) fn output_limit_from_error(message: Option<&str>) -> Option<u64> {
    let message = message?;
    static PATTERNS: OnceLock<[Regex; 2]> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            // Anthropic: "max_tokens: 70000 > 64000, which is the maximum allowed number of output tokens for MODEL"
            Regex::new(r"max_tokens:\s*\d+\s*>\s*(\d+)\s*,\s*which is the maximum allowed number of output tokens")
                .expect("static pattern"),
            // Gateways: "max_tokens must be less than or equal to 64000", "max_tokens ... at most 64000"
            Regex::new(r"(?i)max_tokens[^0-9]{0,80}(?:less than or equal to|no more than|at most|maximum (?:value )?(?:is|of))\s*(\d+)")
                .expect("static pattern"),
        ]
    });
    patterns.iter().find_map(|pattern| {
        let limit: u64 = pattern.captures(message)?.get(1)?.as_str().parse().ok()?;
        (MIN_PLAUSIBLE_LIMIT..=MAX_PLAUSIBLE_LIMIT)
            .contains(&limit)
            .then_some(limit)
    })
}

/// Remembers a provider's stated output ceiling for the rest of this process.
/// Only ever lowers the stored value.
pub(crate) fn learn(provider_id: &str, limit: u64) {
    let Ok(mut learned) = LEARNED.get_or_init(|| Mutex::new(HashMap::new())).lock() else {
        return;
    };
    let entry = learned.entry(provider_id.to_string()).or_insert(limit);
    *entry = (*entry).min(limit);
}

pub(crate) fn learned(provider_id: &str) -> Option<u64> {
    LEARNED.get()?.lock().ok()?.get(provider_id).copied()
}

/// The `max_output_tokens` to request: the provider override when set, otherwise the
/// request's own value or `default`; never above a ceiling the upstream already stated.
pub(crate) fn effective_max_output_tokens(
    provider_override: Option<u64>,
    request_value: Option<u64>,
    default: u64,
    learned_ceiling: Option<u64>,
) -> u64 {
    let wanted = provider_override
        .filter(|v| *v > 0)
        .or(request_value.filter(|v| *v > 0))
        .unwrap_or(default);
    learned_ceiling.map_or(wanted, |ceiling| wanted.min(ceiling))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_anthropic_and_gateway_limit_messages() {
        let anthropic = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: 70000 > 64000, which is the maximum allowed number of output tokens for claude-opus-4-5"}}"#;
        assert_eq!(output_limit_from_error(Some(anthropic)), Some(64000));
        assert_eq!(
            output_limit_from_error(Some("max_tokens must be less than or equal to 32000")),
            Some(32000)
        );
        assert_eq!(
            output_limit_from_error(Some("Invalid max_tokens value, at most 8192 allowed")),
            Some(8192)
        );
    }

    #[test]
    fn ignores_unrelated_or_implausible_errors() {
        for message in [
            None,
            Some("upstream 502"),
            Some("budget_tokens must be less than max_tokens"),
            Some("prompt is too long: 250000 tokens > 200000 maximum"),
            Some("max_tokens: 9 > 5, which is the maximum allowed number of output tokens"),
        ] {
            assert_eq!(output_limit_from_error(message), None, "{message:?}");
        }
    }

    #[test]
    fn learned_ceiling_only_lowers_and_clamps_requests() {
        assert_eq!(learned("rect-provider-a"), None);
        learn("rect-provider-a", 64000);
        learn("rect-provider-a", 128000);
        assert_eq!(learned("rect-provider-a"), Some(64000));
        learn("rect-provider-a", 32000);
        assert_eq!(learned("rect-provider-a"), Some(32000));
        assert_eq!(learned("rect-provider-b"), None);

        // Provider override wins over the request; the learned ceiling wins over both.
        assert_eq!(
            effective_max_output_tokens(Some(64000), Some(1000), 32000, None),
            64000
        );
        assert_eq!(
            effective_max_output_tokens(Some(64000), None, 32000, Some(32000)),
            32000
        );
        assert_eq!(
            effective_max_output_tokens(None, Some(1000), 32000, Some(32000)),
            1000
        );
        assert_eq!(
            effective_max_output_tokens(None, None, 32000, Some(16000)),
            16000
        );
        assert_eq!(effective_max_output_tokens(None, None, 32000, None), 32000);
    }
}
