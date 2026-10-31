//! Provider-specific output-token ceilings learned at runtime.
//!
//! When a client omits `max_output_tokens`, the gateway first uses the model
//! ceiling supplied by the provider or models.dev, then the conservative
//! unknown-model default. Hosts can still cap the same model differently, so
//! a rejection that names a smaller ceiling teaches the gateway that
//! provider-model limit for later requests.

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
    /// is lower than the resolved model capability or unknown-model default.
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
