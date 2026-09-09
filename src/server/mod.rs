use std::convert::Infallible;

use axum::body::Body;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{OriginalUri, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::stream::{BoxStream, SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{Value, json};
use tokio_tungstenite_proxy::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite_proxy::{MaybeTlsStream, WebSocketStream};
use tower_http::decompression::RequestDecompressionLayer;
use uuid::Uuid;

use crate::benchmark::{BenchmarkSnapshotResponse, ModelBenchmarkManager, StartBenchmarkRequest};
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::fusion::{FusionEngine, should_fuse_turn, validate_fusion_profiles};
use crate::gateway::UpstreamRouting;
use crate::gateway::{CacheShapeTracker, ProviderTokenUsage, RequestPlan, ResolvedModelRoute};
use crate::images::ImageRouteRegistry;
use crate::protocol::ResponseStream;
use crate::protocol::sse::encode_event;
use crate::provider::capabilities::ProviderCapabilities;
use crate::provider::{ProviderRegistry, ProviderRuntime};
use crate::web_search::{WebSearchCapabilities, WebSearchProbeSummary};

pub(crate) mod auth;
mod compact;
mod error_response;
mod images;
mod messages_http;
mod realtime;
mod request_body;
mod responses_http;
mod responses_ws;
mod routes;
mod state;
mod websocket_proxy;

pub use routes::{ServeExit, router, serve, serve_on_listener, serve_on_listener_with_reload};
pub use state::{AnthropicByteStream, AppState};

#[cfg(test)]
mod tests;
