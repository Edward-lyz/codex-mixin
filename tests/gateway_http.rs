use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod common;

#[path = "gateway_http/auth.rs"]
mod auth;
#[path = "gateway_http/chat_and_gemini.rs"]
mod chat_and_gemini;
#[path = "gateway_http/collaboration.rs"]
mod collaboration;
#[path = "gateway_http/compression.rs"]
mod compression;
#[path = "gateway_http/context.rs"]
mod context;
#[path = "gateway_http/conversion.rs"]
mod conversion;
#[path = "gateway_http/corpus.rs"]
mod corpus;
#[path = "gateway_http/fusion.rs"]
mod fusion;
#[path = "gateway_http/native_messages.rs"]
mod native_messages;
#[path = "gateway_http/official.rs"]
mod official;
#[path = "gateway_http/payload_and_failover.rs"]
mod payload_and_failover;
#[path = "gateway_http/providers.rs"]
mod providers;
#[path = "gateway_http/websocket.rs"]
mod websocket;

use chat_and_gemini::spawn_mock_gemini;
use websocket::websocket_response_frames;

use axum::body::Body;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use codex_mixin::anthropic::ModelInfo;
use codex_mixin::config::{
    FailoverRoute, FusionMode, FusionProfile, FusionTimeRoute, PanelToolsConfig,
};
use codex_mixin::config::{GatewayConfig, ThinkingMode};
use codex_mixin::protocol::sse::drain_events;
use codex_mixin::provider::{
    ProviderModel, ProviderModelSource, ProviderPreset, ProviderProtocol, ProviderRegistry,
    ProviderRequestPolicy, apply_discovered_models, aws_bedrock_aksk_provider,
    discover_provider_models,
};
use codex_mixin::server::{AppState, router};
use codex_mixin::web_search::WebSearchCapabilities;
use futures_util::future::join_all;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// Marker the gateway leaves behind when it drops a tool-result image. Pinned
/// here on purpose: the text is part of the provider-visible prompt, so changing
/// it invalidates every cached prefix in flight.
const TOOL_IMAGE_PLACEHOLDER: &str = "[tool image omitted from replay to preserve prompt cache]";

/// Client-visible deadline for a gateway websocket round trip. The behaviour
/// under test is the gateway's own (millisecond) timeout; a tight outer bound
/// only measures how loaded the runner is while this binary runs ~160 tests in
/// parallel, each with its own gateway and mock upstream.
const WS_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

fn png_data_url(width: u32, height: u32) -> String {
    use base64::Engine;
    use image::codecs::png::PngEncoder;
    use image::{ColorType, ImageBuffer, ImageEncoder, Rgba};

    let pixels = ImageBuffer::from_fn(width, height, |x, y| {
        Rgba([x as u8, y as u8, (x ^ y) as u8, 255])
    });
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(&pixels, width, height, ColorType::Rgba8.into())
        .unwrap();
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn base64_png_dimensions(data: &str) -> (u32, u32) {
    use base64::Engine;

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    image::ImageReader::new(std::io::Cursor::new(decoded))
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap()
}

fn data_url_dimensions(image_url: &str) -> (u32, u32) {
    let (_, data) = image_url.split_once(";base64,").unwrap();
    base64_png_dimensions(data)
}

#[derive(Clone)]
enum MockMode {
    Text,
    Thinking,
    UnsignedThinking,
    Tool,
    ToolThenText,
    FableMcpTool,
    NamespacedTool,
    CustomTool,
    ToolSearch,
    WebSearchRetry,
    ImageTool,
    ImageToolFailure,
    ContextStatusThenText,
    ContextStatusOnly,
    ContextInvalidCompress,
    ContextCompressThenText,
    ContextMixedToolsThenText,
}

#[derive(Clone)]
struct MockState {
    mode: MockMode,
    requests: Arc<Mutex<Vec<Value>>>,
}

#[derive(Clone)]
struct ProtocolRouteState {
    requests: Arc<Mutex<Vec<Value>>>,
}

#[derive(Clone)]
struct FusionMockState {
    requests: Arc<Mutex<Vec<Value>>>,
    failing_models: Arc<Vec<String>>,
    panel_barrier: Option<Arc<tokio::sync::Barrier>>,
    active_panels: Arc<AtomicUsize>,
    max_active_panels: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct HangingWsUpstreamState {
    requests: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
    cancelled: Arc<tokio::sync::Notify>,
}

struct NotifyOnDrop(Arc<tokio::sync::Notify>);

impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[derive(Clone)]
struct OfficialState {
    requests: Arc<Mutex<Vec<Value>>>,
    auth_headers: Arc<Mutex<Vec<Option<String>>>>,
    account_headers: Arc<Mutex<Vec<Option<String>>>>,
    forwarded_headers: Arc<Mutex<Vec<HeaderMap>>>,
    websocket_connections: Arc<AtomicUsize>,
    websocket_behavior: OfficialWebSocketBehavior,
}

#[derive(Clone, Copy)]
enum OfficialWebSocketBehavior {
    Persistent,
    CloseAfterCreated,
    CloseAfterCompletedWithCustomTool,
    CloseAfterCompletedWithCollaborationTool,
    NativeAgentEnvelopeGate,
    TerminalFailuresBeforeRecovery,
    ConnectionLimitThenComplete,
    Silent,
    SlowHandshake,
    RateLimitedHttp,
}

fn has_native_agent_envelope(body: &Value) -> bool {
    body.get("input")
        .and_then(Value::as_array)
        .is_some_and(|input| {
            input.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("agent_message")
                    && item
                        .get("content")
                        .and_then(Value::as_array)
                        .is_some_and(|content| {
                            content.iter().any(|part| {
                                part.get("type").and_then(Value::as_str)
                                    == Some("encrypted_content")
                            })
                        })
            })
        })
}

fn test_config(upstream_base_url: String) -> GatewayConfig {
    let mut provider = ProviderPreset::Custom.create("custom", "upstream-key");
    provider.base_url = upstream_base_url;
    // Most gateway fixtures still exercise the Anthropic Messages path.
    provider.protocol = ProviderProtocol::AnthropicMessages;
    provider.api_path = "/v1/messages".to_owned();
    provider.anthropic_version = Some("2023-06-01".to_owned());
    provider.cached_models = [
        "DeepSeek-V4-Flash",
        "Claude Sonnet 5",
        "Kimi-K2.7-Code",
        "Fable 5",
        "panel-a",
        "panel-b",
        "judge",
        "final",
        "gpt-5.5",
        "gpt-5.6-sol",
    ]
    .into_iter()
    .map(|id| ProviderModel {
        id: id.to_owned(),
        ..ProviderModel::default()
    })
    .collect();
    provider.selected_models = provider
        .cached_models
        .iter()
        .map(|model| model.id.clone())
        .collect();
    GatewayConfig {
        context: Default::default(),
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        codex_auth_path: std::path::PathBuf::from("/tmp/codex-auth.json"),
        gateway_api_key: Some("gateway-key".to_owned()),
        gateway_api_keys: Vec::new(),
        cors_allowed_origins: codex_mixin::config::StoredGatewayConfig::default()
            .cors_allowed_origins,
        gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys::default(),
        accept_codex_oauth: true,
        official_ech_fallback_reason: None,
        official_ech_proxy: false,
        official_selected_models: None,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(30),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
        failover_routes: Vec::new(),
    }
}

fn configure_baidu_policy(config: &mut GatewayConfig) {
    config.providers[0].preset_id = Some("baidu-oneapi".to_owned());
    config.providers[0].request_policy = ProviderRequestPolicy {
        session_affinity_header: Some("x-hash-key".to_owned()),
        mcp_bridge_for_fable: true,
        ..ProviderRequestPolicy::default()
    };
}

fn configure_custom_headers_from_env(config: &mut GatewayConfig) {
    config.providers[0]
        .request_policy
        .custom_headers_from_env
        .insert("x-example-auth".to_owned(), "TEST_EXAMPLE_AUTH".to_owned());
}

fn configure_openai_chat(config: &mut GatewayConfig, api_path: &str) {
    config.providers[0].protocol = ProviderProtocol::OpenAiChat;
    config.providers[0].api_path = api_path.to_owned();
}

fn configure_model_protocol(
    config: &mut GatewayConfig,
    model_id: &str,
    protocol: ProviderProtocol,
    api_path: &str,
) {
    let model = config.providers[0]
        .cached_models
        .iter_mut()
        .find(|model| model.id == model_id)
        .unwrap();
    model.protocol = Some(protocol);
    model.api_path = Some(api_path.to_owned());
}

async fn spawn_router(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    format!("http://{addr}")
}

async fn spawn_mock_upstream(mode: MockMode) -> (String, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = MockState {
        mode,
        requests: requests.clone(),
    };
    let app = Router::new()
        .route("/v1/models", get(mock_models))
        .route("/v1/messages", post(mock_messages))
        .route("/v1/realtime", get(mock_custom_realtime_ws))
        .route("/v1/realtime/calls", post(mock_custom_realtime_call))
        .route(
            "/v1/live",
            get(mock_custom_realtime_ws).post(mock_custom_realtime_call),
        )
        .route("/v1/live/{call_id}", get(mock_custom_realtime_ws))
        .route("/v1/images/generations", post(mock_image_generations))
        .with_state(state);
    (spawn_router(app).await, requests)
}

async fn hanging_ws_messages(State(state): State<HangingWsUpstreamState>, body: Body) -> Response {
    let _ = axum::body::to_bytes(body, 4 * 1024 * 1024).await.unwrap();
    let request_index = state.requests.fetch_add(1, Ordering::SeqCst);
    if request_index == 0 {
        state.started.notify_one();
        let cancel_guard = NotifyOnDrop(Arc::clone(&state.cancelled));
        let stream = async_stream::stream! {
            let _cancel_guard = cancel_guard;
            std::future::pending::<()>().await;
            yield Ok::<_, std::convert::Infallible>(bytes::Bytes::new());
        };
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    } else {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(text_sse()))
            .unwrap()
    }
}

async fn mock_custom_realtime_ws(
    State(state): State<MockState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let query = uri.query().unwrap_or_default().to_owned();
    let path = uri.path().to_owned();
    ws.on_upgrade(move |mut socket| async move {
        while let Some(Ok(message)) = socket.next().await {
            let body = match message {
                AxumWsMessage::Text(text) => serde_json::from_str::<Value>(&text).unwrap(),
                AxumWsMessage::Binary(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap(),
                AxumWsMessage::Ping(bytes) => {
                    socket.send(AxumWsMessage::Pong(bytes)).await.unwrap();
                    continue;
                }
                AxumWsMessage::Pong(_) => continue,
                AxumWsMessage::Close(_) => break,
            };
            state.requests.lock().unwrap().push(body);
            socket
                .send(AxumWsMessage::Text(
                    json!({
                        "type": "session.updated",
                        "authorization": authorization,
                        "upstream_path": path,
                        "upstream_query": query,
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        }
    })
    .into_response()
}

async fn mock_custom_realtime_call(
    State(state): State<MockState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = axum::body::to_bytes(body, 4 * 1024 * 1024).await.unwrap();
    state.requests.lock().unwrap().push(json!({
        "authorization": authorization,
        "body": String::from_utf8(body.to_vec()).unwrap(),
        "content_type": content_type,
        "path": uri.path(),
        "query": uri.query().unwrap_or_default(),
    }));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/sdp")
        .header(
            header::LOCATION,
            "https://custom.example/v1/realtime/calls/custom-call",
        )
        .body(Body::from("custom-answer-sdp"))
        .unwrap()
}

async fn spawn_baidu_protocol_upstream() -> (String, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = ProtocolRouteState {
        requests: requests.clone(),
    };
    let app = Router::new()
        .route("/v1/messages", post(mock_baidu_routed_messages))
        .route("/v1/responses", post(mock_baidu_routed_responses))
        .with_state(state);
    (spawn_router(app).await, requests)
}

async fn spawn_deepseek_reasoning_upstream() -> (String, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route(
            "/v1/responses",
            post(|State(requests): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                requests.lock().unwrap().push(body.clone());
                let has_replayable_reasoning = body["input"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|item| item["type"] == "reasoning")
                    .all(|item| {
                        item["content"].as_array().is_some_and(|content| {
                            content.iter().any(|part| {
                                part["type"] == "reasoning_text"
                                    && part["text"]
                                        .as_str()
                                        .is_some_and(|text| !text.trim().is_empty())
                            })
                        })
                    });
                if !has_replayable_reasoning {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error":{"message":"The `reasoning_text` in the thinking mode must be passed back to the API."}})),
                    )
                        .into_response();
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(text_sse()))
                    .unwrap()
            }),
        )
        .with_state(requests.clone());
    (spawn_router(app).await, requests)
}

async fn spawn_fusion_mock_upstream(
    panel_barrier: Option<Arc<tokio::sync::Barrier>>,
    failing_models: Vec<&str>,
) -> (String, Arc<Mutex<Vec<Value>>>, Arc<AtomicUsize>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let max_active_panels = Arc::new(AtomicUsize::new(0));
    let state = FusionMockState {
        requests: requests.clone(),
        failing_models: Arc::new(failing_models.into_iter().map(str::to_owned).collect()),
        panel_barrier,
        active_panels: Arc::new(AtomicUsize::new(0)),
        max_active_panels: max_active_panels.clone(),
    };
    let app = Router::new()
        .route("/v1/models", get(mock_models))
        .route(
            "/v1/messages",
            post(
                |State(state): State<FusionMockState>, Json(body): Json<Value>| async move {
                    let model = body["model"].as_str().unwrap_or_default().to_owned();
                    state.requests.lock().unwrap().push(body);
                    let is_panel = model.starts_with("panel-");
                    if is_panel {
                        let active = state.active_panels.fetch_add(1, Ordering::SeqCst) + 1;
                        state.max_active_panels.fetch_max(active, Ordering::SeqCst);
                        if let Some(barrier) = &state.panel_barrier {
                            barrier.wait().await;
                        }
                        state.active_panels.fetch_sub(1, Ordering::SeqCst);
                    }
                    if state.failing_models.contains(&model) {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "intentional fusion panel failure".to_owned(),
                        )
                            .into_response();
                    }
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(if is_panel {
                            panel_report_sse()
                        } else {
                            text_sse()
                        }))
                        .unwrap()
                },
            ),
        )
        .with_state(state);
    (spawn_router(app).await, requests, max_active_panels)
}

async fn spawn_mock_openai_chat() -> (String, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/models", get(mock_models))
        .route("/chat/completions", post(mock_openai_chat_completions))
        .with_state(requests.clone());
    (spawn_router(app).await, requests)
}

async fn spawn_mock_openai_image_chat() -> (String, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let state = MockState {
        mode: MockMode::ImageTool,
        requests: requests.clone(),
    };
    let app = Router::new()
        .route(
            "/chat/completions",
            post(mock_openai_image_chat_completions),
        )
        .route("/images/generations", post(mock_image_generations))
        .with_state(state);
    (spawn_router(app).await, requests)
}

async fn spawn_baidu_metadata_upstream() -> String {
    let app = Router::new().route(
        "/openapi/v2/available_models",
        post(mock_baidu_available_models),
    );
    spawn_router(app).await
}

async fn spawn_session_required_upstream() -> String {
    let app = Router::new().route(
        "/v1/messages",
        post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            let hash_key = headers
                .get("x-hash-key")
                .and_then(|value| value.to_str().ok())
                .filter(|value| uuid::Uuid::parse_str(value).is_ok());
            if body["metadata"]["user_id"].as_str() != hash_key {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"SESSION_REQUIRED"}})),
                )
                    .into_response();
            }
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(text_sse()))
                .unwrap()
        }),
    );
    spawn_router(app).await
}

async fn spawn_mock_official(
    websocket_behavior: OfficialWebSocketBehavior,
) -> (
    String,
    Arc<Mutex<Vec<Value>>>,
    Arc<Mutex<Vec<Option<String>>>>,
    Arc<Mutex<Vec<Option<String>>>>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<HeaderMap>>>,
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let auth_headers = Arc::new(Mutex::new(Vec::new()));
    let account_headers = Arc::new(Mutex::new(Vec::new()));
    let forwarded_headers = Arc::new(Mutex::new(Vec::new()));
    let websocket_connections = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/v1/realtime", get(mock_official_realtime_ws))
        .route(
            "/v1/live",
            get(mock_official_realtime_ws).post(mock_official_realtime_call),
        )
        .route("/v1/live/{call_id}", get(mock_official_realtime_ws))
        .route("/v1/realtime/calls", post(mock_official_realtime_call))
        .route(
            "/v1/responses",
            get(mock_official_responses_ws).post(mock_official_responses),
        )
        .route("/v1/responses/compact", post(mock_official_responses))
        .route("/v1/images/generations", post(mock_official_images))
        .route("/v1/images/edits", post(mock_official_images))
        .with_state(OfficialState {
            requests: requests.clone(),
            auth_headers: auth_headers.clone(),
            account_headers: account_headers.clone(),
            forwarded_headers: forwarded_headers.clone(),
            websocket_connections: websocket_connections.clone(),
            websocket_behavior,
        });
    (
        spawn_router(app).await,
        requests,
        auth_headers,
        account_headers,
        websocket_connections,
        forwarded_headers,
    )
}

async fn mock_official_realtime_ws(
    State(state): State<OfficialState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    state
        .forwarded_headers
        .lock()
        .unwrap()
        .push(headers.clone());
    state.auth_headers.lock().unwrap().push(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    state.account_headers.lock().unwrap().push(
        headers
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    state.websocket_connections.fetch_add(1, Ordering::SeqCst);
    let query = uri.query().unwrap_or_default().to_owned();
    ws.on_upgrade(move |mut socket| async move {
        while let Some(Ok(message)) = socket.next().await {
            let body = match message {
                AxumWsMessage::Text(text) => serde_json::from_str::<Value>(&text).unwrap(),
                AxumWsMessage::Binary(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap(),
                AxumWsMessage::Ping(bytes) => {
                    socket.send(AxumWsMessage::Pong(bytes)).await.unwrap();
                    continue;
                }
                AxumWsMessage::Pong(_) => continue,
                AxumWsMessage::Close(_) => break,
            };
            state.requests.lock().unwrap().push(body);
            socket
                .send(AxumWsMessage::Text(
                    json!({
                        "type": "session.updated",
                        "session": {"id": "voice-session"},
                        "upstream_path": uri.path(),
                        "upstream_query": query,
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        }
    })
    .into_response()
}

async fn mock_official_realtime_call(
    State(state): State<OfficialState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    state.auth_headers.lock().unwrap().push(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    state.account_headers.lock().unwrap().push(
        headers
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    state.requests.lock().unwrap().push(json!({
        "body": body,
        "path": uri.path(),
        "query": uri.query().unwrap_or_default(),
    }));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/sdp")
        .header(header::LOCATION, "/v1/realtime/calls/call-456")
        .body(Body::from("answer-sdp"))
        .unwrap()
}

async fn mock_models(headers: HeaderMap) -> impl IntoResponse {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    assert_eq!(auth, Some("Bearer upstream-key"));
    Json(json!({
        "object": "list",
        "data": [
            {"id": "DeepSeek-V4-Flash", "object": "model", "created": 1, "owned_by": "custom"},
            {"id": "Claude Sonnet 5", "object": "model", "created": 1, "owned_by": "custom"},
            {"id": "Kimi-K2.7-Code", "object": "model", "created": 1, "owned_by": "custom"}
        ]
    }))
}

async fn mock_baidu_available_models(
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    assert_eq!(auth, Some("Bearer upstream-key"));
    assert_eq!(body, json!({}));
    Json(json!({
        "success": true,
        "message": "",
        "data": [{
            "model": "DeepSeek-V4-Flash",
            "capability": {
                "supports_image": false,
                "supports_thinking": true,
                "context_window": 1024000,
                "ratio": "0.2x",
                "model_description": "Fast coding model"
            },
            "price_type": "Value model"
        }, {
            "model": "Claude Sonnet 5-custom",
            "capability": null,
            "price_type": "Expensive model"
        }, {
            "model": "Kimi-K2.7-Code-内部",
            "capability": {
                "supports_image": true,
                "supports_thinking": true,
                "context_window": 256000,
                "ratio": "1.0x",
                "model_description": "Long-context coding model"
            },
            "price_type": "Value model"
        }, {
            "model": "auto-内部",
            "capability": {
                "supports_image": false,
                "supports_thinking": true,
                "context_window": 100000,
                "ratio": "2.0x",
                "model_description": "Internal default model"
            },
            "price_type": "Internal model"
        }, {
            "model": "auto",
            "capability": {
                "supports_image": false,
                "supports_thinking": true,
                "context_window": 200000,
                "ratio": "1.0x",
                "model_description": "Default model"
            },
            "price_type": "Value model"
        }]
    }))
}

async fn mock_messages(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    assert!(
        auth == Some("Bearer upstream-key")
            || auth.is_some_and(|value| value.starts_with("AWS4-HMAC-SHA256 "))
    );
    let has_hosted_web_search = body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("name").and_then(Value::as_str) == Some("web_search")
                    && tool
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|tool_type| tool_type.starts_with("web_search_"))
            })
        });
    let hosted_web_search_forced = body.get("tool_choice").is_some_and(|tool_choice| {
        tool_choice.get("type").and_then(Value::as_str) == Some("tool")
            && tool_choice.get("name").and_then(Value::as_str) == Some("web_search")
    });
    let is_capability_probe = body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("name").and_then(Value::as_str) == Some("codex_mixin_probe_noop")
            })
        });
    let is_fusion_panel = body["system"]
        .to_string()
        .contains("substantive, concise report");
    let has_tool_result = body
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|messages| {
            messages.iter().any(|message| {
                message
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|content| {
                        content.iter().any(|block| block["type"] == "tool_result")
                    })
            })
        });
    let request_index = state.requests.lock().unwrap().len();
    body["__x_hash_key"] = headers
        .get("x-hash-key")
        .and_then(|value| value.to_str().ok())
        .map_or(Value::Null, |value| json!(value));
    body["__anthropic_beta"] = headers
        .get("anthropic-beta")
        .and_then(|value| value.to_str().ok())
        .map_or(Value::Null, |value| json!(value));
    if auth.is_some_and(|value| value.starts_with("AWS4-HMAC-SHA256 ")) {
        body["__authorization"] = auth.map_or(Value::Null, |value| json!(value));
        body["__aws_session_token"] = headers
            .get("x-amz-security-token")
            .and_then(|value| value.to_str().ok())
            .map_or(Value::Null, |value| json!(value));
    }
    state.requests.lock().unwrap().push(body.clone());
    let payload = match state.mode {
        MockMode::Text if is_fusion_panel => panel_report_sse(),
        MockMode::Text => text_sse(),
        MockMode::Thinking => thinking_sse(),
        MockMode::UnsignedThinking => unsigned_thinking_sse(),
        MockMode::Tool => tool_sse("exec_command", json!({"cmd":"pwd"})),
        MockMode::ToolThenText if has_tool_result => text_sse(),
        MockMode::ToolThenText => text_then_tool_sse("exec_command", json!({"cmd":"pwd"})),
        MockMode::FableMcpTool => tool_sse("mcp__codex__exec_command", json!({"cmd":"pwd"})),
        MockMode::NamespacedTool => tool_sse(
            "collaboration__spawn_agent",
            json!({"task_name":"test","message":"run"}),
        ),
        MockMode::CustomTool => tool_sse(
            "apply_patch",
            json!({"input":"*** Begin Patch\n*** End Patch"}),
        ),
        MockMode::ToolSearch => {
            tool_sse("tool_search", json!({"query":"calendar create","limit":1}))
        }
        MockMode::WebSearchRetry if !has_hosted_web_search => text_sse(),
        MockMode::WebSearchRetry if is_capability_probe && hosted_web_search_forced => {
            web_search_sse()
        }
        MockMode::WebSearchRetry if hosted_web_search_forced && request_index >= 1 => {
            web_search_sse()
        }
        MockMode::WebSearchRetry => tool_sse("web_search", json!({})),
        MockMode::ContextStatusThenText if has_tool_result => text_sse(),
        MockMode::ContextInvalidCompress => tool_sse(
            "mixin_context_compress",
            json!({
                "ranges": [{"start": "m000001", "end": "m000002"}],
                "topic": "invalid range", "summary": "Cannot compress protected or missing history."
            }),
        ),
        MockMode::ContextStatusThenText | MockMode::ContextStatusOnly => {
            tool_sse("mixin_context_status", json!({}))
        }
        MockMode::ContextMixedToolsThenText if has_tool_result => text_sse(),
        MockMode::ContextMixedToolsThenText => double_tool_sse(
            ("mixin_context_status", json!({})),
            ("exec_command", json!({"cmd":"pwd"})),
        ),
        MockMode::ContextCompressThenText if has_tool_result => text_sse(),
        MockMode::ContextCompressThenText => tool_sse(
            "mixin_context_compress",
            json!({
                "ranges": [{"start": "m000003", "end": "m000004"}],
                "topic": "exploration",
                "summary": "alpha and beta explored two ideas"
            }),
        ),
        MockMode::ImageTool | MockMode::ImageToolFailure => tool_sse(
            "image_gen__imagegen",
            json!({
                "prompt":"draw a blue square",
                "referenced_image_paths": [],
                "num_last_images_to_include": 0
            }),
        ),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(payload))
        .unwrap()
}

async fn mock_baidu_routed_messages(
    State(state): State<ProtocolRouteState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    record_baidu_protocol_request(&state, "/v1/messages", &headers, body);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(text_sse()))
        .unwrap()
}

async fn mock_baidu_routed_responses(
    State(state): State<ProtocolRouteState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let contains_websocket_envelope =
        body.get("type").is_some() || body.get("previous_response_id").is_some();
    let is_compaction = body["tools"].as_array().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool["name"].as_str() == Some("submit_compaction"))
    });
    let is_claude_compat = body["instructions"].as_str() == Some("You are Claude Code.");
    let has_foreign_ciphertext = body["input"].as_array().is_some_and(|input| {
        input
            .iter()
            .any(|item| item["encrypted_content"] == "gAAAAA-foreign-account")
    });
    record_baidu_protocol_request(&state, "/v1/responses", &headers, body);
    if has_foreign_ciphertext {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": "The encrypted content for item rs_parent could not be verified. Reason: Encrypted content could not be decrypted or parsed.",
                    "type": "invalid_request_error",
                    "code": "invalid_encrypted_content"
                }
            })),
        )
            .into_response();
    }
    if contains_websocket_envelope {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": "Responses WebSocket envelope fields are not accepted over HTTP"
                }
            })),
        )
            .into_response();
    }
    if is_compaction {
        let arguments = json!({
            "goal":"continue task",
            "constraints":["no tools"],
            "decisions":["use compact"],
            "files":["src/server/compact.rs"],
            "tool_results":["tests passed"],
            "pending_work":["run e2e"]
        });
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!(
                "event: response.output_item.done\ndata: {}\n\nevent: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp_baidu\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"output\":[]}}}}\n\n",
                json!({
                    "type":"response.output_item.done",
                    "item": {
                        "type":"function_call",
                        "id":"fc_baidu",
                        "call_id":"call_baidu",
                        "name":"submit_compaction",
                        "arguments":arguments.to_string()
                    }
                })
            )))
            .unwrap();
    }
    if is_claude_compat {
        let item = json!({
            "type":"function_call",
            "id":"fc_claude",
            "call_id":"call_claude",
            "name":"exec_command",
            "arguments":"{\"cmd\":\"pwd\"}"
        });
        let events = [
            json!({"type":"response.output_item.added","output_index":0,"item":item.clone()}),
            json!({"type":"response.function_call_arguments.delta","item_id":"fc_claude","output_index":0,"delta":"{\"cmd\":"}),
            json!({"type":"response.function_call_arguments.delta","item_id":"fc_claude","output_index":0,"delta":"\"pwd\"}"}),
            json!({"type":"response.function_call_arguments.done","item_id":"fc_claude","output_index":0,"arguments":"{\"cmd\":\"pwd\"}"}),
            json!({"type":"response.output_item.done","output_index":0,"item":item.clone()}),
            json!({
                "type":"response.completed",
                "response":{
                    "id":"resp_claude",
                    "object":"response",
                    "status":"completed",
                    "model":"gpt-5.6-sol",
                    "output":[item],
                    "usage":{"input_tokens":12,"output_tokens":4,"total_tokens":16}
                }
            }),
        ];
        let payload = events
            .iter()
            .map(|event| {
                format!(
                    "event: {}\ndata: {event}\n\n",
                    event["type"].as_str().unwrap()
                )
            })
            .collect::<String>();
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(payload))
            .unwrap();
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_baidu\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
        )))
        .unwrap()
}

fn record_baidu_protocol_request(
    state: &ProtocolRouteState,
    path: &str,
    headers: &HeaderMap,
    body: Value,
) {
    assert_eq!(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer upstream-key")
    );
    assert!(
        headers.get("x-example-auth").is_some(),
        "request must include the configured custom header"
    );
    state.requests.lock().unwrap().push(json!({
        "path": path,
        "anthropic_version": headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok()),
        "x_hash_key": headers
            .get("x-hash-key")
            .and_then(|value| value.to_str().ok()),
        "body": body,
    }));
}

async fn mock_image_generations(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    assert_eq!(auth, Some("Bearer upstream-key"));
    state.requests.lock().unwrap().push(body.clone());
    if matches!(state.mode, MockMode::ImageToolFailure) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"message":"image model unavailable"}})),
        )
            .into_response();
    }
    Json(json!({
        "created": 1,
        "data": [{
            "b64_json": "aW1hZ2UtYnl0ZXM=",
            "revised_prompt": "a blue square"
        }]
    }))
    .into_response()
}

async fn mock_openai_chat_completions(
    State(requests): State<Arc<Mutex<Vec<Value>>>>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    assert_eq!(auth, Some("Bearer upstream-key"));
    body["__x_hash_key"] = headers
        .get("x-hash-key")
        .and_then(|value| value.to_str().ok())
        .map_or(Value::Null, |value| json!(value));
    let is_compaction = body["tools"].as_array().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool["function"]["name"].as_str() == Some("submit_compaction"))
    });
    requests.lock().unwrap().push(body);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(if is_compaction {
            openai_tool_sse(
                "submit_compaction",
                json!({
                    "goal":"continue task",
                    "constraints":["no tools"],
                    "decisions":["use compact"],
                    "files":["src/server/compact.rs"],
                    "tool_results":["tests passed"],
                    "pending_work":["run e2e"]
                }),
            )
        } else {
            openai_chat_sse()
        }))
        .unwrap()
}

async fn mock_openai_image_chat_completions(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    assert_eq!(auth, Some("Bearer upstream-key"));
    state.requests.lock().unwrap().push(body);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(openai_image_tool_sse()))
        .unwrap()
}

async fn mock_official_responses(
    State(state): State<OfficialState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    state.requests.lock().unwrap().push(body.clone());
    state
        .forwarded_headers
        .lock()
        .unwrap()
        .push(headers.clone());
    state.auth_headers.lock().unwrap().push(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    state.account_headers.lock().unwrap().push(
        headers
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    if matches!(
        state.websocket_behavior,
        OfficialWebSocketBehavior::RateLimitedHttp
    ) {
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "error": {
                        "message": "slow down",
                        "type": "rate_limit_error"
                    }
                })
                .to_string(),
            ))
            .unwrap();
    }
    if uri.path().ends_with("/responses/compact") {
        return Json(json!({
            "id": "resp_official_compact",
            "object": "response",
            "status": "completed",
            "model": body["model"],
            "output": [{
                "type": "compaction",
                "id": "cmp_official",
                "created_by": "official",
                "encrypted_content": "official-opaque-token"
            }]
        }))
        .into_response();
    }
    if matches!(
        state.websocket_behavior,
        OfficialWebSocketBehavior::NativeAgentEnvelopeGate
    ) && has_native_agent_envelope(&body)
    {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "error": {
                        "message": "encrypted_content could not be decrypted",
                        "type": "invalid_request_error"
                    }
                })
                .to_string(),
            ))
            .unwrap();
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\"}}\n\n",
        ))
        .unwrap()
}

async fn mock_official_images(
    State(state): State<OfficialState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.requests.lock().unwrap().push(body);
    state
        .forwarded_headers
        .lock()
        .unwrap()
        .push(headers.clone());
    state.auth_headers.lock().unwrap().push(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    state.account_headers.lock().unwrap().push(
        headers
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    Json(json!({
        "created": 1,
        "data": [{"b64_json":"b2ZmaWNpYWwtaW1hZ2U="}]
    }))
}

async fn mock_official_responses_ws(
    State(state): State<OfficialState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    state
        .forwarded_headers
        .lock()
        .unwrap()
        .push(headers.clone());
    state.auth_headers.lock().unwrap().push(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    state.account_headers.lock().unwrap().push(
        headers
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    );
    if matches!(
        state.websocket_behavior,
        OfficialWebSocketBehavior::SlowHandshake
    ) {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let connection_id = state.websocket_connections.fetch_add(1, Ordering::SeqCst) + 1;
    ws.on_upgrade(move |socket| serve_mock_official_websocket(socket, state, connection_id))
        .into_response()
}

#[allow(clippy::cognitive_complexity)]
async fn serve_mock_official_websocket(
    mut socket: WebSocket,
    state: OfficialState,
    connection_id: usize,
) {
    let mut last_response_id = None;
    while let Some(Ok(message)) = socket.next().await {
        let body = match message {
            AxumWsMessage::Text(text) => serde_json::from_str::<Value>(&text).unwrap(),
            AxumWsMessage::Binary(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap(),
            AxumWsMessage::Ping(bytes) => {
                socket.send(AxumWsMessage::Pong(bytes)).await.unwrap();
                continue;
            }
            AxumWsMessage::Pong(_) => continue,
            AxumWsMessage::Close(_) => break,
        };
        let previous_response_id = body
            .get("previous_response_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let should_generate = body.get("generate").and_then(Value::as_bool) != Some(false);
        let gated_native_envelope = matches!(
            state.websocket_behavior,
            OfficialWebSocketBehavior::NativeAgentEnvelopeGate
        ) && has_native_agent_envelope(&body);
        let orphan_custom_tool_output = previous_response_id
            .is_none()
            .then(|| {
                body.get("input")
                    .and_then(Value::as_array)
                    .and_then(|input| {
                        input.iter().enumerate().find_map(|(index, item)| {
                            if item.get("type").and_then(Value::as_str)
                                != Some("custom_tool_call_output")
                            {
                                return None;
                            }
                            let call_id = item.get("call_id").and_then(Value::as_str)?;
                            let has_matching_call = input[..index].iter().any(|candidate| {
                                candidate.get("type").and_then(Value::as_str)
                                    == Some("custom_tool_call")
                                    && candidate.get("call_id").and_then(Value::as_str)
                                        == Some(call_id)
                            });
                            (!has_matching_call).then(|| call_id.to_owned())
                        })
                    })
            })
            .flatten();
        let request_number = {
            let mut requests = state.requests.lock().unwrap();
            requests.push(body);
            requests.len()
        };
        if gated_native_envelope
            || matches!(state.websocket_behavior, OfficialWebSocketBehavior::Silent)
        {
            while socket.next().await.is_some() {}
            return;
        }
        let validation_error = if previous_response_id.is_some()
            && previous_response_id != last_response_id
        {
            Some(format!(
                "Previous response with id '{}' not found.",
                previous_response_id.as_deref().unwrap()
            ))
        } else {
            orphan_custom_tool_output.map(|call_id| {
                format!("No tool call found for custom tool call output with call_id {call_id}.")
            })
        };
        if let Some(message) = validation_error {
            let error = json!({"message": message, "type": "invalid_request_error"});
            socket
                .send(AxumWsMessage::Text(
                    json!({
                        "type": "response.failed",
                        "response": {
                            "id": format!("failed_{connection_id}_{request_number}"),
                            "object": "response",
                            "status": "failed",
                            "error": error,
                            "output": []
                        },
                        "error": error
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            continue;
        }
        let terminal_failure = match (state.websocket_behavior, request_number) {
            (OfficialWebSocketBehavior::TerminalFailuresBeforeRecovery, 2) => {
                Some("response.failed")
            }
            (OfficialWebSocketBehavior::TerminalFailuresBeforeRecovery, 3) => {
                Some("response.incomplete")
            }
            _ => None,
        };
        if let Some(event_type) = terminal_failure {
            let error = json!({
                "code": "invalid_prompt",
                "message": format!("synthetic {event_type}")
            });
            socket
                .send(AxumWsMessage::Text(
                    json!({
                        "type": event_type,
                        "response": {
                            "id": format!("failed_{connection_id}_{request_number}"),
                            "object": "response",
                            "status": event_type.strip_prefix("response.").unwrap(),
                            "error": error,
                            "output": []
                        },
                        "error": error
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            continue;
        }
        if matches!(
            state.websocket_behavior,
            OfficialWebSocketBehavior::ConnectionLimitThenComplete
        ) && request_number == 1
        {
            socket
                .send(AxumWsMessage::Text(
                    json!({
                        "type": "error",
                        "status": 400,
                        "error": {
                            "type": "invalid_request_error",
                            "code": "websocket_connection_limit_reached",
                            "message": "Responses websocket connection limit reached (60 minutes). Create a new websocket connection to continue."
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            continue;
        }
        let response_id = format!("official_{connection_id}_{request_number}");
        socket
            .send(AxumWsMessage::Text(
                json!({
                    "type": "response.created",
                    "response": {
                        "id": response_id,
                        "object": "response",
                        "status": "in_progress",
                        "output": []
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        if matches!(
            state.websocket_behavior,
            OfficialWebSocketBehavior::CloseAfterCreated
        ) {
            return;
        }
        let output_item = should_generate.then(|| {
            if matches!(
                state.websocket_behavior,
                OfficialWebSocketBehavior::CloseAfterCompletedWithCollaborationTool
            ) && request_number == 1
            {
                return json!({
                    "type":"function_call", "id":"fc_delegation", "status":"completed",
                    "namespace":"collaboration", "name":"spawn_agent",
                    "call_id":"call_delegation", "arguments":"{\"message\":\"read probe\"}"
                });
            }
            if matches!(
                state.websocket_behavior,
                OfficialWebSocketBehavior::CloseAfterCompletedWithCustomTool
            ) && request_number == 1
            {
                json!({
                    "type": "custom_tool_call",
                    "id": "ctc_reconnect",
                    "status": "completed",
                    "call_id": "call_reconnect_custom",
                    "name": "exec",
                    "input": "{}"
                })
            } else {
                json!({
                    "type": "message",
                    "id": format!("message_{connection_id}_{request_number}"),
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type":"output_text","text":"official reply","annotations":[]}]
                })
            }
        });
        if let Some(output_item) = output_item.as_ref() {
            socket
                .send(AxumWsMessage::Text(
                    json!({
                        "type": "response.output_item.done",
                        "output_index": 0,
                        "item": output_item
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        }
        let completed_output = if matches!(
            state.websocket_behavior,
            OfficialWebSocketBehavior::CloseAfterCompletedWithCustomTool
        ) && request_number == 1
        {
            json!([])
        } else {
            Value::Array(output_item.into_iter().collect())
        };
        socket
            .send(AxumWsMessage::Text(
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": response_id,
                        "object": "response",
                        "status": "completed",
                        "output": completed_output
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        last_response_id = Some(response_id);
        if matches!(
            state.websocket_behavior,
            OfficialWebSocketBehavior::CloseAfterCompletedWithCustomTool
                | OfficialWebSocketBehavior::CloseAfterCompletedWithCollaborationTool
        ) {
            return;
        }
    }
}

fn text_sse() -> String {
    text_sse_with_parts(&["hello", " codex"])
}

fn thinking_sse() -> String {
    [
        json!({"type":"message_start","message":{"id":"msg_thinking","type":"message","role":"assistant","content":[],"model":"Claude Sonnet 5","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"checked constraints"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed-state"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":8}}),
        json!({"type":"message_stop"}),
    ]
    .into_iter()
    .map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()))
    .collect()
}

fn unsigned_thinking_sse() -> String {
    [
        json!({"type":"message_start","message":{"id":"msg_unsigned_thinking","type":"message","role":"assistant","content":[],"model":"Claude Haiku 4.5","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"unsigned thought"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":8}}),
        json!({"type":"message_stop"}),
    ]
    .into_iter()
    .map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()))
    .collect()
}

fn panel_report_sse() -> String {
    text_sse_with(
        r#"{"findings":["The implementation has a concrete finding."],"risks":[],"recommendations":["Apply the recommended change."],"evidence":["mock evidence"]}"#,
    )
}

fn text_sse_with(text: &str) -> String {
    text_sse_with_parts(&[text])
}

fn text_sse_with_parts(parts: &[&str]) -> String {
    let mut events = vec![
        json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"DeepSeek-V4-Flash","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
    ];
    events.extend(parts.iter().map(|text| {
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}})
    }));
    events.extend([
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
        json!({"type":"message_stop"}),
    ]);
    events
        .into_iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect()
}

fn openai_chat_sse() -> String {
    [
        r#"data: {"id":"chatcmpl_1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}

"#,
        r#"data: {"id":"chatcmpl_1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}

"#,
        r#"data: {"id":"chatcmpl_1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":" openai"},"finish_reason":null}]}

"#,
        r#"data: {"id":"chatcmpl_1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}

"#,
        "data: [DONE]\n\n",
    ]
    .join("")
}

fn openai_image_tool_sse() -> String {
    let chunk = json!({
        "id": "chatcmpl_image",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {"tool_calls": [{
                "index": 0,
                "id": "call_image",
                "function": {
                    "name": "image_gen__imagegen",
                    "arguments": "{\"prompt\":\"draw a blue square\"}"
                }
            }]},
            "finish_reason": "tool_calls"
        }]
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

fn openai_tool_sse(name: &str, arguments: Value) -> String {
    let chunk = json!({
        "id": "chatcmpl_compact",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {"tool_calls": [{
                "index": 0,
                "id": "call_compact",
                "function": {
                    "name": name,
                    "arguments": arguments.to_string()
                }
            }]},
            "finish_reason": "tool_calls"
        }]
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n")
}

fn tool_sse(name: &str, arguments: Value) -> String {
    [
        json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"DeepSeek-V4-Flash","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":name,"input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":arguments.to_string()}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
        json!({"type":"message_stop"}),
    ]
    .into_iter()
    .map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()))
    .collect()
}

fn double_tool_sse(first: (&str, Value), second: (&str, Value)) -> String {
    let blocks = [first, second];
    let mut events = vec![
        json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"DeepSeek-V4-Flash","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}),
    ];
    for (index, (name, arguments)) in blocks.iter().enumerate() {
        events.push(json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":format!("toolu_{index}"),"name":name,"input":{}}}));
        events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":arguments.to_string()}}));
        events.push(json!({"type":"content_block_stop","index":index}));
    }
    events.push(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}));
    events.push(json!({"type":"message_stop"}));
    events
        .into_iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect()
}

fn text_then_tool_sse(name: &str, arguments: Value) -> String {
    [
        json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"DeepSeek-V4-Flash","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"checking"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":name,"input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":arguments.to_string()}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
        json!({"type":"message_stop"}),
    ]
    .into_iter()
    .map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()))
    .collect()
}

fn web_search_sse() -> String {
    [
        json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"Claude Sonnet 5","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srvtoolu_123","name":"web_search","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"OpenAI Codex\"}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_123","content":[{"type":"web_search_result","title":"Codex","url":"https://openai.com/codex"}]}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"message_stop"}),
    ]
    .into_iter()
    .map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()))
    .collect()
}

async fn spawn_gateway(upstream_base_url: String) -> String {
    spawn_gateway_with_config(test_config(upstream_base_url)).await
}

async fn spawn_gateway_with_config(config: GatewayConfig) -> String {
    let database = tempfile::tempdir().unwrap();
    let state = AppState::with_env_lookup(config, |name| match name {
        codex_mixin::context::DATABASE_ENV => {
            Some(database.path().join("context.db").display().to_string())
        }
        "TEST_EXAMPLE_AUTH" => Some("signed-value".to_owned()),
        _ => None,
    })
    .unwrap();
    spawn_router(router(state).layer(axum::Extension(Arc::new(database)))).await
}

async fn spawn_gateway_with_context(
    config: GatewayConfig,
    database_path: &std::path::Path,
) -> String {
    let database = database_path.to_owned();
    let state = AppState::with_env_lookup(config, move |name| match name {
        codex_mixin::context::DATABASE_ENV => Some(database.display().to_string()),
        "TEST_EXAMPLE_AUTH" => Some("signed-value".to_owned()),
        _ => None,
    })
    .unwrap();
    spawn_router(router(state)).await
}

async fn mock_official_gateway_config(
    behavior: OfficialWebSocketBehavior,
    request_timeout: Duration,
) -> (
    GatewayConfig,
    Arc<Mutex<Vec<Value>>>,
    Arc<AtomicUsize>,
    tempfile::TempDir,
) {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, _, _, official_websocket_connections, _) =
        spawn_mock_official(behavior).await;
    let mut config = test_config(upstream_url);
    config.request_timeout = request_timeout;
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    (
        config,
        official_requests,
        official_websocket_connections,
        codex_home,
    )
}

async fn spawn_gateway_with_mock_official(
    behavior: OfficialWebSocketBehavior,
    request_timeout: Duration,
) -> (
    String,
    Arc<Mutex<Vec<Value>>>,
    Arc<AtomicUsize>,
    tempfile::TempDir,
) {
    let (config, official_requests, official_websocket_connections, codex_home) =
        mock_official_gateway_config(behavior, request_timeout).await;
    (
        spawn_gateway_with_config(config).await,
        official_requests,
        official_websocket_connections,
        codex_home,
    )
}

async fn spawn_gateway_with_mock_official_context(
    behavior: OfficialWebSocketBehavior,
    request_timeout: Duration,
    database_path: &std::path::Path,
) -> (
    String,
    Arc<Mutex<Vec<Value>>>,
    Arc<AtomicUsize>,
    tempfile::TempDir,
) {
    let (mut config, official_requests, official_websocket_connections, codex_home) =
        mock_official_gateway_config(behavior, request_timeout).await;
    config.context.enabled = true;
    (
        spawn_gateway_with_context(config, database_path).await,
        official_requests,
        official_websocket_connections,
        codex_home,
    )
}

fn responses_request() -> Value {
    json!({
        "model": "DeepSeek-V4-Flash-custom",
        "stream": true,
        "instructions": "You are Codex.",
        "input": [
            {"type":"message","role":"developer","content":[{"type":"input_text","text":"dev rules"}]},
            {"type":"message","role":"user","content":[{"type":"input_text","text":"say hi"}]}
        ],
        "tools": [
            {"type":"function","name":"exec_command","description":"run shell","parameters":{"type":"object","properties":{"cmd":{"type":"string"}}}}
        ]
    })
}

fn gzip_bytes(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

async fn spawn_compression_upstream(
    reject_gzip: bool,
    failure: Option<StatusCode>,
) -> (String, Arc<Mutex<Vec<Value>>>) {
    use std::io::Read;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let url = spawn_router(Router::new().route("/v1/messages", post(move |headers: HeaderMap, bytes: bytes::Bytes| {
        let captured = Arc::clone(&captured);
        async move {
            let encoding = headers.get(header::CONTENT_ENCODING).and_then(|value| value.to_str().ok());
            let mut decoded = Vec::new();
            if encoding == Some("gzip") {
                flate2::read::GzDecoder::new(bytes.as_ref()).read_to_end(&mut decoded).unwrap();
            } else { decoded.extend_from_slice(&bytes); }
            assert_eq!(headers[header::CONTENT_LENGTH].to_str().unwrap().parse::<usize>().unwrap(), bytes.len());
            let body: Value = serde_json::from_slice(&decoded).unwrap();
            captured.lock().unwrap().push(json!({"encoding":encoding,"wire_bytes":bytes.len(),"decoded_bytes":decoded.len(),"body":body,"accept_encoding":headers.get(header::ACCEPT_ENCODING).unwrap().to_str().unwrap()}));
            if reject_gzip && encoding == Some("gzip") {
                return (StatusCode::UNSUPPORTED_MEDIA_TYPE, [(header::ACCEPT_ENCODING,"identity")], "request compression is unsupported").into_response();
            }
            if let Some(status) = failure { return (status, "upstream failure").into_response(); }
            Response::builder().status(StatusCode::OK)
                .header(header::CONTENT_TYPE,"text/event-stream")
                .header(header::CONTENT_ENCODING,"gzip")
                .body(Body::from(gzip_bytes(text_sse().as_bytes()))).unwrap()
        }
    }).options(|| async { [(header::ACCEPT_ENCODING, "gzip")] }))).await;
    (url, requests)
}

fn fusion_profile() -> FusionProfile {
    FusionProfile {
        id: "default".to_owned(),
        mode: Default::default(),
        time_routes: Vec::new(),
        panel_models: vec!["panel-a-custom".to_owned(), "panel-b-custom".to_owned()],
        judge_model: "judge-custom".to_owned(),
        final_model: "final-custom".to_owned(),
        min_successful: 2,
        max_completion_tokens: 2048,
        timeout_ms: 5_000,
        show_intermediate_results: true,
        panel_tools: PanelToolsConfig {
            enabled: false,
            ..PanelToolsConfig::default()
        },
    }
}

fn fusion_request() -> Value {
    let mut request = responses_request();
    request["model"] = json!("auto/default");
    request["instructions"] = json!(
        "You are Codex.\n<collaboration_mode># Collaboration Mode: Plan\nPlan the requested work.</collaboration_mode>"
    );
    request
}

fn image_tool_request() -> Value {
    let mut request = responses_request();
    request["tools"] = json!([{
        "type": "namespace",
        "name": "image_gen",
        "tools": [{
            "type": "function",
            "name": "imagegen",
            "description": "Generate an image",
            "parameters": {
                "type": "object",
                "properties": {"prompt": {"type": "string"}},
                "required": ["prompt"]
            }
        }]
    }]);
    request
}

fn image_tool_arguments(response_body: &str) -> Value {
    let mut encoded = response_body.as_bytes().to_vec();
    let event = drain_events(&mut encoded)
        .into_iter()
        .find(|event| {
            let Ok(payload) = serde_json::from_str::<Value>(&event.data) else {
                return false;
            };
            payload["type"] == "response.output_item.done"
                && payload["item"]["namespace"] == "image_gen"
                && payload["item"]["name"] == "imagegen"
        })
        .expect("response did not contain a completed image_gen.imagegen call");
    let payload: Value = serde_json::from_str(&event.data).unwrap();
    serde_json::from_str(payload["item"]["arguments"].as_str().unwrap()).unwrap()
}
