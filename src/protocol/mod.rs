//! Translation between the downstream Codex Responses protocol and the
//! upstream provider protocols.
//!
//! convert: Responses requests -> Anthropic Messages requests.
//! openai_chat: Responses requests -> OpenAI Chat Completions requests.
//! gemini: Responses requests -> Google Gemini requests.
//! openai_events: upstream Anthropic / OpenAI Chat / Gemini streams -> Responses
//! SSE events.
//! sse: SSE encode/decode primitives shared by every stream.
//! compaction: self-contained conversation-summary tokens that survive
//! provider switches.
//! model_reasoning: per-model reasoning-effort mapping.

pub(crate) mod anthropic_compat;
pub(crate) mod chat_compat;
pub(crate) mod compaction;
pub mod convert;
pub mod gemini;
pub(crate) mod kiro;
pub mod model_reasoning;
pub mod openai_chat;
pub mod openai_events;
pub mod sse;

use std::convert::Infallible;

use bytes::Bytes;
use futures_util::stream::BoxStream;
use serde_json::Value;

/// A stream of Responses-protocol SSE bytes.
pub type ResponseStream = BoxStream<'static, Result<Bytes, Infallible>>;

/// A fully drained Responses stream.
#[derive(Clone, Debug)]
pub struct CollectedResponse {
    pub response: Value,
}

impl CollectedResponse {
    pub(crate) fn output_items(&self) -> &[Value] {
        self.response
            .get("output")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub(crate) fn output_text(&self) -> String {
        self.output_items()
            .iter()
            .flat_map(|item| {
                item.get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(|part| {
                matches!(
                    part.get("type").and_then(Value::as_str),
                    Some("output_text" | "text")
                )
                .then(|| part.get("text").and_then(Value::as_str))
                .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
