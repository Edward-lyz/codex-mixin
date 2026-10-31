use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::stream::FuturesUnordered;
use futures_util::{StreamExt, TryFutureExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{FusionMode, FusionProfile, OFFICIAL_MODEL_PREFIX};
use crate::error::GatewayError;
use crate::gateway::{GatewayExecutor, RequestPlan, UpstreamRouting, collect_response_stream};
use crate::protocol::ResponseStream;
use crate::protocol::sse::{SseDecoder, encode_event, encode_raw_event};

use self::tools::PanelToolExecutor;

mod analysis;
mod engine;
mod prompts;
mod render;
mod routing;
mod tools;
mod types;
mod visualization;

pub use engine::FusionEngine;
pub use routing::should_fuse_turn;
