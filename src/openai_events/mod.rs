use std::collections::{HashMap, HashSet};
use std::convert::Infallible;

use async_stream::stream;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::convert::ToolNameMap;
use crate::image_generation::ImageRouteRegistry;
use crate::sse::{SseDecoder, encode_event, encode_raw_event};

mod anthropic;
mod ducx;
mod openai_chat;
mod state;

pub use anthropic::map_anthropic_sse;
pub(crate) use anthropic::map_anthropic_sse_with_image_routes;
pub use ducx::map_ducx_events;
pub use openai_chat::map_openai_chat_sse;
pub(crate) use openai_chat::map_openai_chat_sse_with_image_routes;

pub(super) fn coalesce_events(events: &mut Vec<Bytes>) -> Option<Bytes> {
    if events.is_empty() {
        return None;
    }
    let capacity = events.iter().map(Bytes::len).sum::<usize>();
    let mut combined = bytes::BytesMut::with_capacity(capacity);
    for event in events.drain(..) {
        combined.extend_from_slice(&event);
    }
    Some(combined.freeze())
}

#[cfg(test)]
mod tests;
