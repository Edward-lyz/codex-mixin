use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::stream::{self, BoxStream, SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{Value, json};
use tokio_tungstenite_proxy::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite_proxy::{MaybeTlsStream, WebSocketStream};
use tower_http::decompression::RequestDecompressionLayer;
use uuid::Uuid;

use crate::anthropic::{MessageRequest, ModelInfo};
use crate::benchmark::{
    BenchmarkSnapshotResponse, BenchmarkTarget, ModelBenchmarkManager, StartBenchmarkRequest,
};
use crate::catalog::{codex_catalog_from_models_with_metadata, load_template_catalog};
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::fusion::{FusionEngine, should_fuse_turn, validate_fusion_profiles};
use crate::gateway::{
    CacheShapeTracker, ModelRouter, ProviderTokenUsage, RequestPlan, ResolvedModelRoute,
    UpstreamExecutor,
};
use crate::gateway::{ResponseStream, UpstreamRouting};
use crate::images::ImageRouteRegistry;
use crate::protocol::sse::{SseDecoder, encode_event};
use crate::provider::MetadataResolver;
use crate::provider::capabilities::ProviderCapabilities;
use crate::provider::{
    ProviderRegistry, ProviderRuntime, ResolvedProviderModel, catalog_model_slug,
};
use crate::web_search::{WebSearchCapabilities, WebSearchProbeSummary};

mod anthropic_compat;
pub(crate) mod auth;
mod compact;
mod images;
mod messages_http;
mod realtime;
mod responses_http;
mod responses_ws;
mod routes;
mod state;
mod websocket_proxy;

pub(crate) use responses_http::stream_official_response;
pub use routes::{router, serve, serve_on_listener};
pub use state::{AnthropicByteStream, AppState};

#[cfg(test)]
mod tests;
