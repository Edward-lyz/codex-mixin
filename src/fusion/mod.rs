use std::collections::HashSet;
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use axum::http::HeaderMap;
use bytes::Bytes;
use futures_util::stream::FuturesUnordered;
use futures_util::{StreamExt, TryFutureExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::GatewayError;
use crate::fusion_tools::PanelToolExecutor;
use crate::server::{AppState, stream_official_response};
use crate::sse::{SseDecoder, encode_event, encode_raw_event};
use crate::upstream::{
    ResponseStream, UpstreamRouting, collect_response_stream, stream_response_with_options,
};

mod analysis;
mod engine;
mod profile;
mod prompts;
mod render;
mod routing;
mod types;

pub use engine::FusionEngine;
pub use profile::{
    FUSION_MODEL_PREFIX, FusionProfile, OFFICIAL_MODEL_PREFIX, PanelToolsConfig,
    validate_fusion_profiles,
};
pub use routing::{
    ModelRoute, canonical_upstream_model_alias, is_upstream_model_alias, model_route,
    should_fuse_turn,
};

#[cfg(test)]
use analysis::*;
#[cfg(test)]
use prompts::*;
#[cfg(test)]
mod tests;
