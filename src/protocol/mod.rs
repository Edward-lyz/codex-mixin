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

pub(crate) mod compaction;
pub mod convert;
pub mod model_reasoning;
pub mod openai_chat;
pub mod openai_events;
pub(crate) mod request_body;
pub mod sse;
