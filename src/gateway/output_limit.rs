//! Provider-specific output-token ceilings learned at runtime.
//!
//! When a client omits `max_output_tokens`, the gateway asks Anthropic
//! Messages providers for its generous default. Hosts cap the same model
//! differently (Baidu OneAPI serves GLM-5.3 with a 256K ceiling while other
//! hosts allow more or less), so no static table is correct. The first
//! rejection that names the ceiling teaches the gateway the real limit for
//! that provider model; later requests use it directly.

use std::collections::HashMap;
use std::sync::Mutex;

/// Smallest ceiling worth learning. A smaller number in an error message is
/// almost certainly not an output limit (a status code, a parameter index).
const MINIMUM_LEARNED_LIMIT: u64 = 1024;

#[derive(Debug, Default)]
pub(crate) struct OutputLimitMemory {
    limits: Mutex<HashMap<(String, String), u64>>,
}

impl OutputLimitMemory {
    /// Default `max_tokens` for a provider model: the learned ceiling when it
    /// is lower than the configured default.
    pub(crate) fn default_for(&self, provider_id: &str, model: &str, configured: u64) -> u64 {
        let limits = self
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        limits
            .get(&(provider_id.to_owned(), model.to_ascii_lowercase()))
            .map_or(configured, |&learned| learned.min(configured))
    }

    pub(crate) fn record(&self, provider_id: &str, model: &str, limit: u64) {
        let mut limits = self
            .limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        limits.insert((provider_id.to_owned(), model.to_ascii_lowercase()), limit);
    }
}

/// Extract the provider's output-token ceiling from a rejection of a request
/// that asked for `requested` tokens. Returns `None` unless the message is
/// about the output limit alone and names a ceiling below the request, so
/// context-window overflows and unrelated validation errors are never
/// mistaken for an output limit.
pub(crate) fn output_limit_from_error(message: &str, requested: u64) -> Option<u64> {
    let lower = message.to_ascii_lowercase();
    let names_output_limit = [
        "max_tokens",
        "max_output_tokens",
        "max tokens",
        "output tokens",
        // Baidu OneAPI: "Requested token count exceeds the configured maximum
        // output length of 131072 tokens." No max_tokens spelling.
        "output length",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let names_context = ["context", "prompt", "input"]
        .iter()
        .any(|needle| lower.contains(needle));
    if !names_output_limit || names_context {
        return None;
    }
    numbers(&lower)
        .filter(|&value| (MINIMUM_LEARNED_LIMIT..requested).contains(&value))
        .max()
}

fn numbers(text: &str) -> impl Iterator<Item = u64> + '_ {
    text.split(|character: char| {
        !(character.is_ascii_digit() || character == '_' || character == ',')
    })
    .filter_map(|token| {
        let digits = token.replace(['_', ','], "");
        (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_ceiling_from_common_rejection_shapes() {
        for (message, limit) in [
            (
                "max_tokens: 512000 > 262144, which is the maximum allowed number of output tokens for glm-5.3",
                262_144,
            ),
            ("Range of max_tokens should be [1, 262144]", 262_144),
            (
                "`max_tokens` must be less than or equal to `131072`",
                131_072,
            ),
            (
                "max_tokens(512000) exceeds the model limit 256,000",
                256_000,
            ),
            (
                r#"{"error":{"type":"invalid_request_error","message":"max_tokens is too large: 512000. This model supports at most 64000 completion tokens"}}"#,
                64_000,
            ),
            (
                "Requested token count exceeds the configured maximum output length of 131072 tokens. (request id: abc) (request id: def)",
                131_072,
            ),
        ] {
            assert_eq!(
                output_limit_from_error(message, 512_000),
                Some(limit),
                "{message}"
            );
        }
    }

    #[test]
    fn ignores_context_overflows_and_unrelated_errors() {
        assert_eq!(
            output_limit_from_error(
                "input length and max_tokens exceed context limit: 150000 + 512000 > 200000",
                512_000
            ),
            None
        );
        assert_eq!(
            output_limit_from_error("tools.3.input_schema is invalid", 512_000),
            None
        );
        assert_eq!(
            output_limit_from_error("max_tokens must be a positive integer", 512_000),
            None
        );
        assert_eq!(
            output_limit_from_error("max_tokens: 512000 > 600000", 512_000),
            None
        );
    }

    #[test]
    fn remembers_the_ceiling_per_provider_model() {
        let memory = OutputLimitMemory::default();
        assert_eq!(memory.default_for("baidu", "GLM-5.3", 512_000), 512_000);
        memory.record("baidu", "GLM-5.3", 262_144);
        assert_eq!(memory.default_for("baidu", "glm-5.3", 512_000), 262_144);
        assert_eq!(memory.default_for("other", "glm-5.3", 512_000), 512_000);
        assert_eq!(memory.default_for("baidu", "glm-5.3", 100_000), 100_000);
    }
}
