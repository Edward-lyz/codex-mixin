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
mod openai_chat;
mod state;

pub use anthropic::map_anthropic_sse;
pub(crate) use anthropic::map_anthropic_sse_with_image_routes;
pub use openai_chat::map_openai_chat_sse;
pub(crate) use openai_chat::map_openai_chat_sse_with_image_routes;

#[cfg(test)]
mod tests;
