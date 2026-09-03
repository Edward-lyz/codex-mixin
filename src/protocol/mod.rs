//! Translation between the downstream Codex Responses protocol and the
//! upstream provider protocols.
//!
//! convert: Responses requests -> Anthropic Messages requests.
//! openai_chat: Responses requests -> OpenAI Chat Completions requests.
//! openai_events: upstream Anthropic / OpenAI Chat streams -> Responses
//! SSE events.
//! sse: SSE encode/decode primitives shared by every stream.
//! compaction: self-contained conversation-summary tokens that survive
//! provider switches.
//! request_body: request-body inspection helpers.
//! model_reasoning: per-model reasoning-effort mapping.

pub(crate) mod anthropic_compat;
pub(crate) mod compaction;
pub mod convert;
pub mod model_reasoning;
pub mod openai_chat;
pub mod openai_events;
pub(crate) mod request_body;
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
    pub output: Vec<Value>,
    pub output_text: String,
    pub usage: Value,
}
