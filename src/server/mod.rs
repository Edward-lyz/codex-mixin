use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::stream::{self, BoxStream, SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
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
use crate::image_generation::ImageRouteRegistry;
use crate::model_metadata::ModelMetadataResolver;
use crate::provider::{
    ProviderRegistry, ProviderRuntime, ResolvedProviderModel, catalog_model_slug,
};
use crate::sse::{SseDecoder, encode_event};
use crate::upstream::{ResponseStream, UpstreamRouting, stream_response_with_options};
use crate::web_search::{WebSearchCapabilities, WebSearchProbeSummary};

mod auth;
mod images;
mod responses_http;
mod responses_ws;
mod routes;
mod state;

pub(crate) use responses_http::stream_official_response;
pub use routes::{router, serve, serve_on_listener};
pub use state::AppState;
pub(crate) use state::ResolvedModelRoute;

#[cfg(test)]
mod tests;
