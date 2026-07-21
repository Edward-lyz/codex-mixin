use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use codex_mixin::anthropic::ModelInfo;
use codex_mixin::config::{
    GatewayConfig, ProviderPreset, ThinkingMode, UpstreamAuthHeader, UpstreamKind,
};
use codex_mixin::fusion::{FusionProfile, PanelToolsConfig};
use codex_mixin::server::{AppState, router};
use codex_mixin::sse::drain_events;
use codex_mixin::web_search::WebSearchCapabilities;
use futures_util::future::join_all;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[derive(Clone)]
enum MockMode {
    Text,
    Tool,
    FableMcpTool,
    NamespacedTool,
    CustomTool,
    ToolSearch,
    WebSearchRetry,
    ImageTool,
    ImageToolFailure,
}

#[derive(Clone)]
struct MockState {
    mode: MockMode,
    requests: Arc<Mutex<Vec<Value>>>,
}

#[derive(Clone)]
struct FusionMockState {
    requests: Arc<Mutex<Vec<Value>>>,
    failing_models: Arc<Vec<String>>,
    panel_delay: Duration,
    active_panels: Arc<AtomicUsize>,
    max_active_panels: Arc<AtomicUsize>,
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
    TerminalFailuresBeforeRecovery,
    ConnectionLimitThenComplete,
    Silent,
    SlowHandshake,
}

fn test_config(upstream_base_url: String) -> GatewayConfig {
    GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        provider_preset: ProviderPreset::Custom,
        upstream_kind: UpstreamKind::AnthropicMessages,
        upstream_base_url,
        upstream_messages_path: "/v1/messages".to_owned(),
        upstream_models_path: "/v1/models".to_owned(),
        upstream_image_generation_path: None,
        upstream_api_key: "upstream-key".to_owned(),
        quota_url: None,
        quota_username: None,
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        codex_auth_path: std::path::PathBuf::from("/tmp/codex-auth.json"),
        upstream_auth_header: UpstreamAuthHeader::AuthorizationBearer,
        anthropic_version: "2023-06-01".to_owned(),
        anthropic_beta: None,
        gateway_api_key: Some("gateway-key".to_owned()),
        accept_codex_oauth: true,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(30),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    }
}

async fn spawn_router(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
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
        .route("/v1/images/generations", post(mock_image_generations))
        .with_state(state);
    (spawn_router(app).await, requests)
}

async fn spawn_fusion_mock_upstream(
    panel_delay: Duration,
    failing_models: Vec<&str>,
) -> (String, Arc<Mutex<Vec<Value>>>, Arc<AtomicUsize>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let max_active_panels = Arc::new(AtomicUsize::new(0));
    let state = FusionMockState {
        requests: requests.clone(),
        failing_models: Arc::new(failing_models.into_iter().map(str::to_owned).collect()),
        panel_delay,
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
                        tokio::time::sleep(state.panel_delay).await;
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
            let valid_hash_key = headers
                .get("x-hash-key")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| uuid::Uuid::parse_str(value).is_ok());
            if body["metadata"]["session_id"] != "stable-session" || !valid_hash_key {
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
        .route(
            "/v1/responses",
            get(mock_official_responses_ws).post(mock_official_responses),
        )
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
            "model": "Claude Sonnet 5",
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
    assert_eq!(auth, Some("Bearer upstream-key"));
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
    let request_index = state.requests.lock().unwrap().len();
    body["__x_hash_key"] = headers
        .get("x-hash-key")
        .and_then(|value| value.to_str().ok())
        .map_or(Value::Null, |value| json!(value));
    body["__anthropic_beta"] = headers
        .get("anthropic-beta")
        .and_then(|value| value.to_str().ok())
        .map_or(Value::Null, |value| json!(value));
    state.requests.lock().unwrap().push(body);
    let payload = match state.mode {
        MockMode::Text if is_fusion_panel => panel_report_sse(),
        MockMode::Text => text_sse(),
        MockMode::Tool => tool_sse("exec_command", json!({"cmd":"pwd"})),
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
        MockMode::WebSearchRetry if hosted_web_search_forced && request_index >= 2 => {
            web_search_sse()
        }
        MockMode::WebSearchRetry => tool_sse("web_search", json!({})),
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

async fn mock_image_generations(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    assert_eq!(auth, Some("Bearer upstream-key"));
    state.requests.lock().unwrap().push(body);
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
    requests.lock().unwrap().push(body);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(openai_chat_sse()))
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
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
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
        if matches!(state.websocket_behavior, OfficialWebSocketBehavior::Silent) {
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
        ) {
            return;
        }
    }
}

fn text_sse() -> String {
    text_sse_with_parts(&["hello", " codex"])
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
    let state = AppState::new(test_config(upstream_base_url)).unwrap();
    spawn_router(router(state)).await
}

async fn spawn_gateway_with_config(config: GatewayConfig) -> String {
    let state = AppState::new(config).unwrap();
    spawn_router(router(state)).await
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
        spawn_gateway_with_config(config).await,
        official_requests,
        official_websocket_connections,
        codex_home,
    )
}

fn responses_request() -> Value {
    json!({
        "model": "DeepSeek-V4-Flash",
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

fn fusion_profile() -> FusionProfile {
    FusionProfile {
        id: "default".to_owned(),
        panel_models: vec!["panel-a".to_owned(), "panel-b".to_owned()],
        judge_model: "judge".to_owned(),
        final_model: "final".to_owned(),
        min_successful: 2,
        max_completion_tokens: 2048,
        timeout_ms: 5_000,
        fuse_every_user_turn: true,
        show_intermediate_results: true,
        panel_tools: PanelToolsConfig {
            enabled: false,
            ..PanelToolsConfig::default()
        },
    }
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

#[tokio::test]
async fn proxies_models_and_generates_catalog() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();

    let models: Value = client
        .get(format!("{gateway_url}/v1/models"))
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"][0]["id"], "DeepSeek-V4-Flash");

    let catalog: Value = client
        .get(format!("{gateway_url}/v1/codex-model-catalog"))
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog["models"][1]["slug"], "Claude Sonnet 5");
}

#[tokio::test]
async fn uses_baidu_available_models_as_authoritative_catalog_source() {
    let upstream_url = spawn_baidu_metadata_upstream().await;
    let mut config = test_config(upstream_url);
    config.provider_preset = ProviderPreset::BaiduOneApi;
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();

    let models: Value = client
        .get(format!("{gateway_url}/v1/models"))
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"][0]["ratio"], "0.2x");
    assert_eq!(models["data"][0]["description"], "Fast coding model");
    assert_eq!(models["data"][0]["context_window"], 1_024_000);
    assert_eq!(models["data"].as_array().unwrap().len(), 3);
    assert_eq!(models["data"][1]["id"], "Kimi-K2.7-Code");
    assert_eq!(models["data"][1]["ratio"], "1.0x");
    assert_eq!(models["data"][2]["id"], "auto");
    assert_eq!(models["data"][2]["description"], "Default model");
    assert_eq!(models["data"][2]["context_window"], 200_000);

    let catalog: Value = client
        .get(format!("{gateway_url}/v1/codex-model-catalog"))
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let model = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == "DeepSeek-V4-Flash")
        .unwrap();
    assert_eq!(
        model["description"],
        "Fast coding model | 0.2x | Value model"
    );
    assert_eq!(model["context_window"], 1_024_000);
    assert_eq!(model["input_modalities"], json!(["text"]));
    let auto = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == "auto")
        .unwrap();
    assert_eq!(auto["description"], "Default model | 1.0x | Value model");
    assert_eq!(auto["context_window"], 200_000);
}

#[tokio::test]
async fn model_request_smoke_succeeds_end_to_end() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("session-id", "generic-provider-session")
        .json(&responses_request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("response.output_text.delta"));
    assert!(body.contains("hello codex"));
    assert!(body.contains("response.completed"));

    let upstream_request = requests.lock().unwrap()[0].clone();
    assert_eq!(upstream_request["model"], "DeepSeek-V4-Flash");
    assert_eq!(upstream_request["messages"][0]["role"], "user");
    assert_eq!(upstream_request["tools"].as_array().unwrap().len(), 1);
    assert!(upstream_request.get("metadata").is_none());
    assert!(
        upstream_request["system"][0]["text"]
            .as_str()
            .unwrap()
            .contains("You are Codex")
    );
}

#[tokio::test]
async fn fusion_runs_on_later_user_turns_and_directs_tool_continuations() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![fusion_profile()];
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();

    let models: Value = client
        .get(format!("{gateway_url}/v1/models"))
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let fusion_model = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == "mixin/fusion/default")
        .unwrap();
    assert_eq!(
        fusion_model["display_name"],
        "Fusion (default): panel-a+panel-b → judge judge"
    );

    let mut request = responses_request();
    request["model"] = json!("mixin/fusion/default");
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = response.text().await.unwrap();
    assert!(response_body.contains("response.output_text.delta"));
    let mut encoded = response_body.into_bytes();
    let events = drain_events(&mut encoded);
    let detail_items = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("response.output_item.done"))
        .filter_map(|event| serde_json::from_str::<Value>(&event.data).ok())
        .filter(|event| {
            event["item"]["type"] == "message"
                && event["item"]["content"][0]["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("## Fusion · "))
        })
        .collect::<Vec<_>>();
    assert_eq!(detail_items.len(), 2);
    assert!(detail_items.iter().all(|event| {
        event["item"]["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("msg_"))
            && event["item"]["role"] == "assistant"
    }));
    let panel_results = detail_items
        .iter()
        .find(|event| {
            event["item"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("## Fusion · Panel Results"))
        })
        .unwrap();
    let panel_results = panel_results["item"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(panel_results.contains("| # | Panel Model | Status | Result |"));
    assert!(panel_results.contains("<code>panel-a</code>"));
    assert!(panel_results.contains("<code>panel-b</code>"));
    assert_eq!(panel_results.matches("<details>").count(), 2);
    assert!(detail_items.iter().any(|event| {
        event["item"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("## Fusion · Judge Synthesis"))
    }));
    let final_message = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("response.output_item.done"))
        .filter_map(|event| serde_json::from_str::<Value>(&event.data).ok())
        .find(|event| {
            event["item"]["type"] == "message"
                && !event["item"]["content"][0]["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("## Fusion · "))
        })
        .unwrap();
    assert_eq!(final_message["output_index"], 2);
    let completed = events
        .into_iter()
        .find(|event| event.event.as_deref() == Some("response.completed"))
        .unwrap();
    let completed: Value = serde_json::from_str(&completed.data).unwrap();
    assert_eq!(completed["response"]["model"], "mixin/fusion/default");
    assert_eq!(completed["response"]["output"][0]["type"], "message");
    assert_eq!(completed["response"]["output"][1]["type"], "message");
    assert_eq!(completed["response"]["output"][2]["type"], "message");

    let first_turn_requests = requests.lock().unwrap().clone();
    assert_eq!(first_turn_requests.len(), 4);
    let mut models = first_turn_requests
        .iter()
        .map(|request| request["model"].as_str().unwrap())
        .collect::<Vec<_>>();
    models.sort_unstable();
    assert_eq!(models, ["final", "judge", "panel-a", "panel-b"]);
    let final_request = first_turn_requests
        .iter()
        .find(|request| request["model"] == "final")
        .unwrap();
    assert!(
        final_request["system"]
            .to_string()
            .contains("JUDGE_ANALYSIS")
    );
    assert!(final_request["system"].to_string().contains("hello codex"));
    for panel_request in first_turn_requests
        .iter()
        .filter(|request| request["model"].as_str().unwrap().starts_with("panel-"))
    {
        assert!(
            !panel_request["system"]
                .to_string()
                .contains("You are Codex")
        );
    }

    request["input"] = json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"follow up"}]}
    ]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let later_body = response.text().await.unwrap();
    assert!(later_body.contains("response.reasoning_summary_text.delta"));
    assert_eq!(requests.lock().unwrap().len(), 8);

    request["input"] = json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"run a tool"}]},
        {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
        {"type":"function_call_output","call_id":"call_1","output":"/tmp"}
    ]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let continuation_body = response.text().await.unwrap();
    assert!(!continuation_body.contains("response.reasoning_summary_text.delta"));
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 9);
    assert_eq!(captured.last().unwrap()["model"], "final");
}

#[tokio::test]
async fn fusion_uses_codex_inline_visualization_when_thread_root_is_available() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let codex_home = tempfile::tempdir().unwrap();
    let thread_id = uuid::Uuid::new_v4().to_string();
    let visualization_dir = codex_home
        .path()
        .join("visualizations/2026/07/21")
        .join(&thread_id);
    let mut config = test_config(upstream_url);
    config.codex_auth_path = codex_home.path().join("auth.json");
    config.fusion_profiles = vec![fusion_profile()];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("mixin/fusion/default");
    request["input"].as_array_mut().unwrap().insert(
        0,
        json!({
            "type":"message",
            "role":"developer",
            "content":[{
                "type":"input_text",
                "text":format!(
                    "<workspace_roots><root>{}</root></workspace_roots>",
                    visualization_dir.display()
                )
            }]
        }),
    );

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("thread-id", &thread_id)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let events = drain_events(&mut encoded);
    let detail_texts = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("response.output_item.done"))
        .filter_map(|event| serde_json::from_str::<Value>(&event.data).ok())
        .filter_map(|event| {
            event["item"]["content"][0]["text"]
                .as_str()
                .map(str::to_owned)
        })
        .filter(|text| text.starts_with("## Fusion · "))
        .collect::<Vec<_>>();
    assert_eq!(detail_texts.len(), 1, "{detail_texts:#?}");
    let visualization = detail_texts
        .iter()
        .find(|text| text.starts_with("## Fusion · Review"))
        .unwrap();
    let file_name = visualization
        .lines()
        .find_map(|line| {
            line.strip_prefix("::codex-inline-vis{file=\"")
                .and_then(|value| value.strip_suffix("\"}"))
        })
        .unwrap();
    let fragment = std::fs::read_to_string(visualization_dir.join(file_name)).unwrap();
    assert!(fragment.contains("class=\"viz-grid fusion-panels\""));
    assert!(fragment.contains("class=\"card fusion-panel\""));
    assert!(fragment.contains("panel-a"));
    assert!(fragment.contains("panel-b"));
    assert!(fragment.contains("Judge synthesis"));
    assert!(fragment.contains("data-fusion-point"));
    assert!(
        !detail_texts
            .iter()
            .any(|text| text.contains("Final Answer"))
    );
    let completed = events
        .iter()
        .find(|event| event.event.as_deref() == Some("response.completed"))
        .unwrap();
    let completed: Value = serde_json::from_str(&completed.data).unwrap();
    assert_eq!(completed["response"]["output"].as_array().unwrap().len(), 2);
    assert_eq!(requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn fusion_can_hide_intermediate_results() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut profile = fusion_profile();
    profile.show_intermediate_results = false;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![profile];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("mixin/fusion/default");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let events = drain_events(&mut encoded);
    assert!(!events.iter().any(|event| {
        serde_json::from_str::<Value>(&event.data)
            .ok()
            .and_then(|event| {
                event["item"]["content"][0]["text"]
                    .as_str()
                    .map(str::to_owned)
            })
            .is_some_and(|text| text.starts_with("## Fusion · "))
    }));
    let final_message = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("response.output_item.done"))
        .filter_map(|event| serde_json::from_str::<Value>(&event.data).ok())
        .find(|event| event["item"]["type"] == "message")
        .unwrap();
    assert_eq!(final_message["output_index"], 0);
    let completed = events
        .iter()
        .find(|event| event.event.as_deref() == Some("response.completed"))
        .unwrap();
    let completed: Value = serde_json::from_str(&completed.data).unwrap();
    assert_eq!(completed["response"]["output"].as_array().unwrap().len(), 1);
    assert_eq!(requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn fusion_panels_run_concurrently() {
    let delay = Duration::from_millis(200);
    let (upstream_url, _, max_active) = spawn_fusion_mock_upstream(delay, Vec::new()).await;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![fusion_profile()];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("mixin/fusion/default");

    let started = Instant::now();
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.text().await.unwrap();
    assert_eq!(max_active.load(Ordering::SeqCst), 2);
    assert!(started.elapsed() < Duration::from_millis(380));
}

#[tokio::test]
async fn fusion_routes_models_across_official_and_upstream_providers() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let official_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = official_requests.clone();
    let official_url = spawn_router(Router::new().route(
        "/v1/responses",
        post(move |Json(body): Json<Value>| {
            let captured = captured.clone();
            async move {
                captured.lock().unwrap().push(body.clone());
                let model = body["model"].as_str().unwrap_or("gpt-5.6-sol");
                let output = if body["instructions"]
                    .as_str()
                    .is_some_and(|instructions| {
                        instructions.contains("substantive, concise report")
                    })
                {
                    r#"{"findings":["Official panel finding."],"risks":[],"recommendations":["Official recommendation."],"evidence":["official evidence"]}"#
                } else {
                    "official result"
                };
                let item = json!({
                    "id":"msg_official",
                    "type":"message",
                    "status":"completed",
                    "role":"assistant",
                    "content":[{"type":"output_text","text":output,"annotations":[]}]
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(format!(
                        "event: response.output_item.done\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
                        json!({
                            "type":"response.output_item.done",
                            "output_index":0,
                            "item":item
                        }),
                        json!({
                            "type":"response.completed",
                            "response":{
                                "id":"resp_official",
                                "object":"response",
                                "status":"completed",
                                "model":model,
                                "output":[],
                                "usage":null
                            }
                        })
                    )))
                    .unwrap()
            }
        }),
    ))
    .await;
    let mut config = test_config(upstream_url);
    config.provider_preset = ProviderPreset::BaiduOneApi;
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    config.fusion_profiles = vec![FusionProfile {
        id: "mixed".to_owned(),
        panel_models: vec![
            "official:gpt-5.6-sol".to_owned(),
            "baidu-oneapi:panel-custom".to_owned(),
        ],
        judge_model: "baidu-oneapi:judge-custom".to_owned(),
        final_model: "official:gpt-5.6-sol".to_owned(),
        min_successful: 2,
        max_completion_tokens: 2048,
        timeout_ms: 300_000,
        fuse_every_user_turn: true,
        show_intermediate_results: true,
        panel_tools: PanelToolsConfig {
            enabled: false,
            ..PanelToolsConfig::default()
        },
    }];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("mixin/fusion/mixed");
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("official result"));
    let upstream_models = upstream_requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request["model"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(upstream_models.contains(&"panel-custom".to_owned()));
    assert!(upstream_models.contains(&"judge-custom".to_owned()));
    {
        let official = official_requests.lock().unwrap();
        assert_eq!(official.len(), 2);
        assert!(
            official
                .iter()
                .all(|request| request["model"] == "gpt-5.6-sol")
        );
        assert!(official.iter().all(|request| request["store"] == false));
        assert!(
            official
                .iter()
                .all(|request| request.get("max_output_tokens").is_none())
        );
        assert!(official[0].get("text").is_none());
        assert!(official[1].to_string().contains("JUDGE_ANALYSIS"));
    }

    request["input"] = json!([
        {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
        {"type":"function_call_output","call_id":"call_1","output":"/tmp"}
    ]);
    let continuation = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(continuation.status(), StatusCode::OK);
    assert!(
        continuation
            .text()
            .await
            .unwrap()
            .contains("mixin/fusion/mixed")
    );
    assert_eq!(official_requests.lock().unwrap().len(), 3);
    assert_eq!(upstream_requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn fusion_degrades_on_partial_and_total_panel_failure() {
    let (upstream_url, requests, _) =
        spawn_fusion_mock_upstream(Duration::ZERO, vec!["panel-b"]).await;
    let mut partial_profile = fusion_profile();
    partial_profile.min_successful = 1;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![partial_profile];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("mixin/fusion/default");
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("response.completed"));
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|body| body["model"] == "judge")
    );

    let (upstream_url, requests, _) =
        spawn_fusion_mock_upstream(Duration::ZERO, vec!["panel-a", "panel-b"]).await;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![fusion_profile()];
    let gateway_url = spawn_gateway_with_config(config).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("panel 成功数不足"));
    assert!(body.contains("response.completed"));
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 3);
    assert!(!captured.iter().any(|body| body["model"] == "judge"));
    assert!(captured.iter().any(|body| body["model"] == "final"));
}

#[tokio::test]
async fn fusion_panel_tool_loop_stops_at_round_limit() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let app = Router::new()
        .route(
            "/v1/messages",
            post(
                |State(requests): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    let model = body["model"].as_str().unwrap_or_default().to_owned();
                    requests.lock().unwrap().push(body);
                    let payload = if model == "panel-a" {
                        tool_sse("read_file", json!({"path":"input.txt"}))
                    } else {
                        text_sse()
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(payload))
                        .unwrap()
                },
            ),
        )
        .with_state(requests.clone());
    let upstream_url = spawn_router(app).await;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("input.txt"), "tool input").unwrap();
    let mut profile = fusion_profile();
    profile.panel_models = vec!["panel-a".to_owned()];
    profile.min_successful = 1;
    profile.panel_tools.enabled = true;
    profile.panel_tools.max_rounds = 1;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![profile];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("mixin/fusion/default");
    request["input"][1]["content"][0]["text"] = json!(format!(
        "inspect the file\n<environment_context><cwd>{}</cwd></environment_context>",
        workspace.path().display()
    ));
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("response.completed")
    );
    let captured = requests.lock().unwrap();
    let panel_requests = captured
        .iter()
        .filter(|request| request["model"] == "panel-a")
        .collect::<Vec<_>>();
    assert_eq!(panel_requests.len(), 2);
    assert!(
        panel_requests[1]
            .to_string()
            .contains("tool budget is exhausted")
    );
    assert_eq!(captured.last().unwrap()["model"], "final");
}

#[tokio::test]
async fn maps_fast_service_tier_to_anthropic_request_and_beta() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.anthropic_beta = Some("existing-beta".to_owned());
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["service_tier"] = json!("priority");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.text().await.unwrap();

    let upstream_request = requests.lock().unwrap()[0].clone();
    assert_eq!(upstream_request["speed"], "fast");
    assert_eq!(
        upstream_request["__anthropic_beta"],
        "existing-beta,fast-mode-2026-02-01"
    );
}

#[tokio::test]
async fn maps_stable_session_to_anthropic_metadata() {
    let upstream_url = spawn_session_required_upstream().await;
    let mut config = test_config(upstream_url);
    config.provider_preset = ProviderPreset::BaiduOneApi;
    let gateway_url = spawn_gateway_with_config(config).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("session-id", "stable-session")
        .json(&responses_request())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn maps_compaction_style_request_without_tools() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "Claude Sonnet 5",
            "stream": true,
            "instructions": "Summarize the conversation for continuation.",
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"first user turn"}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"first assistant turn"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"compress now"}]}
            ],
            "tools": []
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("response.completed"));

    let upstream_request = requests.lock().unwrap()[0].clone();
    assert_eq!(upstream_request["model"], "Claude Sonnet 5");
    assert_eq!(
        upstream_request["system"][0]["text"],
        "Summarize the conversation for continuation."
    );
    assert_eq!(upstream_request["messages"].as_array().unwrap().len(), 3);
    assert_eq!(upstream_request["messages"][1]["role"], "assistant");
    assert!(upstream_request.get("tools").is_none());
}

#[tokio::test]
async fn maps_openai_chat_stream_to_responses_sse() {
    let (upstream_url, requests) = spawn_mock_openai_chat().await;
    let mut config = test_config(upstream_url);
    config.provider_preset = ProviderPreset::DeepSeek;
    config.upstream_kind = UpstreamKind::OpenAiChat;
    config.upstream_messages_path = "/chat/completions".to_owned();
    config.upstream_models_path = "/models".to_owned();
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&responses_request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("response.output_text.delta"));
    assert!(body.contains("hello openai"));
    assert!(body.contains("response.completed"));

    let upstream_request = requests.lock().unwrap()[0].clone();
    assert_eq!(upstream_request["model"], "DeepSeek-V4-Flash");
    assert_eq!(upstream_request["messages"][0]["role"], "system");
    assert_eq!(upstream_request["messages"][1]["role"], "system");
    assert_eq!(upstream_request["messages"][2]["role"], "user");
    assert_eq!(upstream_request["tools"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn maps_oneapi_affinity_for_openai_chat() {
    let (upstream_url, requests) = spawn_mock_openai_chat().await;
    let mut config = test_config(upstream_url);
    config.provider_preset = ProviderPreset::BaiduOneApi;
    config.upstream_kind = UpstreamKind::OpenAiChat;
    config.upstream_messages_path = "/chat/completions".to_owned();
    config.upstream_models_path = "/models".to_owned();
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["service_tier"] = json!("priority");
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("thread-id", "openai-chat-thread")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.text().await.unwrap();

    let upstream_request = requests.lock().unwrap()[0].clone();
    let hash_key = upstream_request["__x_hash_key"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(hash_key).is_ok());
    assert_eq!(upstream_request["service_tier"], "priority");
}

#[tokio::test]
async fn maps_tool_use_to_responses_function_call() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Tool).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&responses_request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("\"type\":\"function_call\""));
    assert!(body.contains("\"name\":\"exec_command\""));
    assert!(body.contains("\\\"cmd\\\":\\\"pwd\\\""));
    assert!(!body.contains("{}{\\\"cmd\\\""));

    let upstream_request = requests.lock().unwrap()[0].clone();
    assert_eq!(upstream_request["tools"][0]["name"], "exec_command");
}

#[tokio::test]
async fn maps_baidu_fable_mcp_tool_name_back_to_codex() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::FableMcpTool).await;
    let mut config = test_config(upstream_url);
    config.provider_preset = ProviderPreset::BaiduOneApi;
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["model"] = json!("Fable 5");

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("session-id", "fable-tool-session")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("\"type\":\"function_call\""));
    assert!(body.contains("\"name\":\"exec_command\""));
    assert!(!body.contains("\"name\":\"mcp__codex__exec_command\""));

    let upstream_request = requests.lock().unwrap()[0].clone();
    assert_eq!(upstream_request["model"], "Fable 5");
    assert_eq!(
        upstream_request["tools"][0]["name"],
        "mcp__codex__exec_command"
    );
}

#[tokio::test]
async fn preserves_namespaced_tool_call_for_codex_executor() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::NamespacedTool).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["tools"] = json!([{
        "type": "namespace",
        "name": "collaboration",
        "tools": [{
            "type": "function",
            "name": "spawn_agent",
            "parameters": {"type": "object"}
        }]
    }]);

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("\"namespace\":\"collaboration\""));
    assert!(body.contains("\"name\":\"spawn_agent\""));
    assert!(!body.contains("\"name\":\"collaboration__spawn_agent\""));
    assert!(!body.contains("collaboration.spawn_agent"));
    assert_eq!(
        requests.lock().unwrap()[0]["tools"][0]["name"],
        "collaboration__spawn_agent"
    );
}

#[tokio::test]
async fn custom_image_tool_calls_configured_upstream_generation_endpoint() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ImageTool).await;
    let mut config = test_config(upstream_url);
    config.upstream_image_generation_path = Some("/v1/images/generations".to_owned());
    let gateway_url = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&image_tool_request())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("\"type\":\"function_call\""), "{body}");
    assert!(body.contains("event: response.completed"));
    assert!(!body.contains("\"type\":\"image_generation_call\""));
    let arguments = image_tool_arguments(&body);
    let routed_prompt = arguments["prompt"].as_str().unwrap();
    assert!(routed_prompt.starts_with("draw a blue square"));
    assert!(routed_prompt.contains("codex-mixin-image-route:"));

    let image_response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "prompt": routed_prompt,
            "model": "gpt-image-2",
            "background": "auto",
            "quality": "auto",
            "size": "auto"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(image_response.status(), StatusCode::OK);
    let image: Value = image_response.json().await.unwrap();
    assert_eq!(image["data"][0]["b64_json"], "aW1hZ2UtYnl0ZXM=");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["tools"][0]["name"], "image_gen__imagegen");
    assert_eq!(requests[1]["model"], "gpt-image-2");
    assert_eq!(requests[1]["prompt"], "draw a blue square");
}

#[tokio::test]
async fn openai_chat_image_tool_calls_configured_upstream_generation_endpoint() {
    let (upstream_url, requests) = spawn_mock_openai_image_chat().await;
    let mut config = test_config(upstream_url);
    config.provider_preset = ProviderPreset::DeepSeek;
    config.upstream_kind = UpstreamKind::OpenAiChat;
    config.upstream_messages_path = "/chat/completions".to_owned();
    config.upstream_image_generation_path = Some("/images/generations".to_owned());
    let gateway_url = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&image_tool_request())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("\"type\":\"function_call\""), "{body}");
    assert!(body.contains("event: response.completed"));
    assert!(!body.contains("\"type\":\"image_generation_call\""));
    let routed_prompt = image_tool_arguments(&body)["prompt"]
        .as_str()
        .unwrap()
        .to_owned();

    let image_response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .bearer_auth("gateway-key")
        .json(&json!({"prompt":routed_prompt,"model":"gpt-image-2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(image_response.status(), StatusCode::OK);

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]["tools"][0]["function"]["name"],
        "image_gen__imagegen"
    );
    assert_eq!(requests[1]["model"], "gpt-image-2");
}

#[tokio::test]
async fn custom_image_tool_uses_official_backend_without_upstream_image_endpoint() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ImageTool).await;
    let (official_url, official_requests, auth_headers, account_headers, _, _) =
        spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&image_tool_request())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("\"type\":\"function_call\""), "{body}");
    assert!(body.contains("\"namespace\":\"image_gen\""));
    assert!(body.contains("\"name\":\"imagegen\""));
    assert!(!body.contains("\"type\":\"image_generation_call\""));
    let prompt = image_tool_arguments(&body)["prompt"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!prompt.contains("codex-mixin-image-route:"));

    let image_request = json!({"prompt":prompt,"model":"gpt-image-2"});
    let image_response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .bearer_auth("gateway-key")
        .json(&image_request)
        .send()
        .await
        .unwrap();
    assert_eq!(image_response.status(), StatusCode::OK);
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(
        official_requests.lock().unwrap().as_slice(),
        &[image_request]
    );
    assert_eq!(
        auth_headers.lock().unwrap().as_slice(),
        &[Some("Bearer codex-oauth-token".to_owned())]
    );
    assert_eq!(
        account_headers.lock().unwrap().as_slice(),
        &[Some("account-1".to_owned())]
    );
}

#[tokio::test]
async fn custom_image_generation_endpoint_failure_is_explicit() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::ImageToolFailure).await;
    let mut config = test_config(upstream_url);
    config.upstream_image_generation_path = Some("/v1/images/generations".to_owned());
    let gateway_url = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&image_tool_request())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    let routed_prompt = image_tool_arguments(&body)["prompt"]
        .as_str()
        .unwrap()
        .to_owned();

    let image_response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .bearer_auth("gateway-key")
        .json(&json!({"prompt":routed_prompt,"model":"gpt-image-2"}))
        .send()
        .await
        .unwrap();

    assert_eq!(image_response.status(), StatusCode::BAD_GATEWAY);
    let error: Value = image_response.json().await.unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("image model unavailable")
    );
}

#[tokio::test]
async fn rejects_unknown_custom_image_route_marker_without_official_fallback() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ImageTool).await;
    let mut config = test_config(upstream_url);
    config.upstream_image_generation_path = Some("/v1/images/generations".to_owned());
    let gateway_url = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "prompt": "draw\n\n<!-- codex-mixin-image-route:00000000000000000000000000000000 -->",
            "model": "gpt-image-2"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn maps_freeform_tool_back_to_custom_tool_call() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::CustomTool).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["tools"] = json!([{
        "type": "custom",
        "name": "apply_patch",
        "description": "Apply a patch",
        "format": {"type": "grammar", "syntax": "lark", "definition": "start: /.+/"}
    }]);

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("\"type\":\"custom_tool_call\""));
    assert!(body.contains("\"name\":\"apply_patch\""));
    assert!(body.contains("*** Begin Patch"));
    assert_eq!(
        requests.lock().unwrap()[0]["tools"][0]["name"],
        "apply_patch"
    );
    assert_eq!(
        requests.lock().unwrap()[0]["tools"][0]["input_schema"]["required"],
        json!(["input"])
    );
}

#[tokio::test]
async fn maps_deferred_tool_search_back_to_codex_call() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::ToolSearch).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["tools"] = json!([{
        "type": "tool_search",
        "execution": "client",
        "description": "Search deferred tools",
        "parameters": {
            "type": "object",
            "properties": {"query": {"type": "string"}}
        }
    }]);

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("\"type\":\"tool_search_call\""));
    assert!(body.contains("\"execution\":\"client\""));
    assert!(body.contains("\"query\":\"calendar create\""));
}

#[tokio::test]
async fn maps_custom_websocket_to_responses_frames() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.provider_preset = ProviderPreset::BaiduOneApi;
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    request
        .headers_mut()
        .insert("session-id", "websocket-session".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = responses_request();
    body.as_object_mut().unwrap().remove("stream");
    body["type"] = json!("response.create");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let mut frames = websocket_response_frames(&mut socket).await;
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();
    frames.extend(websocket_response_frames(&mut socket).await);
    let joined = frames.join("\n");
    assert!(joined.contains("\"type\":\"response.output_text.delta\""));
    assert!(joined.contains("\"delta\":\"hello\""));
    assert!(joined.contains("\"type\":\"response.completed\""));
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["model"], "DeepSeek-V4-Flash");
    assert_eq!(requests[0]["metadata"]["session_id"], "websocket-session");
}

#[tokio::test]
async fn fusion_uses_same_pipeline_on_custom_websocket() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![fusion_profile()];
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = responses_request();
    body.as_object_mut().unwrap().remove("stream");
    body["type"] = json!("response.create");
    body["model"] = json!("mixin/fusion/default");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let frames = websocket_response_frames(&mut socket).await;
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("response.reasoning_summary_text.delta"))
    );
    let completed: Value = frames
        .iter()
        .filter_map(|frame| serde_json::from_str(frame).ok())
        .find(|event: &Value| event["type"] == "response.completed")
        .unwrap();
    assert_eq!(completed["response"]["model"], "mixin/fusion/default");

    let follow_up = json!({
        "type":"response.create",
        "model":"mixin/fusion/default",
        "previous_response_id":completed["response"]["id"],
        "input":[{
            "type":"message",
            "role":"user",
            "content":[{"type":"input_text","text":"follow up"}]
        }]
    });
    socket
        .send(WsMessage::Text(follow_up.to_string().into()))
        .await
        .unwrap();
    let follow_up_frames = websocket_response_frames(&mut socket).await;
    assert!(
        follow_up_frames
            .iter()
            .any(|frame| frame.contains("response.reasoning_summary_text.delta"))
    );
    assert_eq!(requests.lock().unwrap().len(), 8);
    let follow_up_completed: Value = follow_up_frames
        .iter()
        .filter_map(|frame| serde_json::from_str(frame).ok())
        .find(|event: &Value| event["type"] == "response.completed")
        .unwrap();

    let tool_continuation = json!({
        "type":"response.create",
        "model":"mixin/fusion/default",
        "previous_response_id":follow_up_completed["response"]["id"],
        "input":[
            {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
            {"type":"function_call_output","call_id":"call_1","output":"/tmp"}
        ]
    });
    socket
        .send(WsMessage::Text(tool_continuation.to_string().into()))
        .await
        .unwrap();
    let continuation_frames = websocket_response_frames(&mut socket).await;
    assert!(
        continuation_frames
            .iter()
            .all(|frame| !frame.contains("response.reasoning_summary_text.delta"))
    );
    assert_eq!(requests.lock().unwrap().len(), 9);
}

#[tokio::test]
async fn retries_demoted_web_search_on_custom_websocket() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::WebSearchRetry).await;
    let mut config = test_config(upstream_url);
    config.provider_preset = ProviderPreset::BaiduOneApi;
    config.enable_web_search_tool = true;
    let capability_dir = tempfile::tempdir().unwrap();
    let capabilities = WebSearchCapabilities::load(
        capability_dir.path().join("web-search-capabilities.json"),
        &config,
    )
    .unwrap();
    let mut models = vec![ModelInfo {
        id: "Claude Sonnet 5".to_owned(),
        ..ModelInfo::default()
    }];
    capabilities
        .probe_models(&mut models, &config, true)
        .await
        .unwrap();
    requests.lock().unwrap().clear();

    let state = AppState::with_web_search_capabilities(config, capabilities).unwrap();
    let gateway_url = spawn_router(router(state)).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    request
        .headers_mut()
        .insert("session-id", "web-search-session".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = responses_request();
    body.as_object_mut().unwrap().remove("stream");
    body["type"] = json!("response.create");
    body["model"] = json!("Claude Sonnet 5");
    body["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"web_search","external_web_access":true}));
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(frames.contains("\"type\":\"web_search_call\""));
    assert!(frames.contains("\"type\":\"response.completed\""));
    assert!(!frames.contains("\"name\":\"web_search\",\"type\":\"function_call\""));
    let upstream_requests = requests.lock().unwrap();
    assert_eq!(upstream_requests.len(), 3);
    assert_eq!(
        upstream_requests[0]["metadata"]["session_id"],
        "web-search-session"
    );
    assert!(
        upstream_requests[1]["metadata"]["session_id"]
            .as_str()
            .unwrap()
            .starts_with("web-search-session-web-search-retry-")
    );
    assert_eq!(upstream_requests[2]["tool_choice"]["name"], "web_search");
    let hash_key = upstream_requests[0]["__x_hash_key"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(hash_key).is_ok());
    assert!(
        upstream_requests
            .iter()
            .all(|request| request["__x_hash_key"].as_str() == Some(hash_key))
    );
}

#[tokio::test]
async fn rebuilds_custom_history_from_previous_response_id() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();

    let mut first = responses_request();
    first.as_object_mut().unwrap().remove("stream");
    first["type"] = json!("response.create");
    socket
        .send(WsMessage::Text(first.to_string().into()))
        .await
        .unwrap();
    let first_frames = websocket_response_frames(&mut socket).await;
    let completed = first_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    let previous_response_id = completed["response"]["id"].as_str().unwrap();

    let second = json!({
        "type": "response.create",
        "model": "DeepSeek-V4-Flash",
        "previous_response_id": previous_response_id,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type":"input_text","text":"second turn"}]
        }],
        "tools": []
    });
    socket
        .send(WsMessage::Text(second.to_string().into()))
        .await
        .unwrap();
    let second_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(second_frames.contains("\"type\":\"response.completed\""));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["messages"].as_array().unwrap().len(), 3);
    assert_eq!(requests[1]["messages"][0]["role"], "user");
    assert_eq!(requests[1]["messages"][1]["role"], "assistant");
    assert_eq!(requests[1]["messages"][2]["role"], "user");
    assert_eq!(
        requests[1]["messages"][2]["content"][0]["text"],
        "second turn"
    );
}

#[tokio::test]
async fn switches_between_official_and_custom_models_on_one_websocket() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (
        official_url,
        official_requests,
        official_auth_headers,
        official_account_headers,
        official_websocket_connections,
        official_forwarded_headers,
    ) = spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    request
        .headers_mut()
        .insert("originator", "codex_cli_rs".parse().unwrap());
    request
        .headers_mut()
        .insert("x-codex-originator", "legacy-codex".parse().unwrap());
    request
        .headers_mut()
        .insert("x-openai-subagent", "review".parse().unwrap());
    request
        .headers_mut()
        .insert("x-openai-memgen-request", "true".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();

    let mut official = responses_request();
    official.as_object_mut().unwrap().remove("stream");
    official["type"] = json!("response.create");
    official["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(official.to_string().into()))
        .await
        .unwrap();
    let first_official_frames = websocket_response_frames(&mut socket).await;
    let first_official_completed = first_official_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    let first_official_response_id = first_official_completed["response"]["id"].as_str().unwrap();

    let official_follow_up = json!({
        "type": "response.create",
        "model": "gpt-5.5",
        "previous_response_id": first_official_response_id,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type":"input_text","text":"official follow-up"}]
        }]
    });
    socket
        .send(WsMessage::Text(official_follow_up.to_string().into()))
        .await
        .unwrap();
    assert!(
        websocket_response_frames(&mut socket)
            .await
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );

    let mut custom = responses_request();
    custom.as_object_mut().unwrap().remove("stream");
    custom["type"] = json!("response.create");
    socket
        .send(WsMessage::Text(custom.to_string().into()))
        .await
        .unwrap();
    assert!(
        websocket_response_frames(&mut socket)
            .await
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );

    socket
        .send(WsMessage::Text(official.to_string().into()))
        .await
        .unwrap();
    assert!(
        websocket_response_frames(&mut socket)
            .await
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );

    assert_eq!(upstream_requests.lock().unwrap().len(), 1);
    let official_requests = official_requests.lock().unwrap();
    assert_eq!(official_requests.len(), 3);
    assert_eq!(
        official_requests[1]["previous_response_id"],
        first_official_response_id
    );
    assert!(
        official_requests
            .iter()
            .all(|request| request["model"] == "gpt-5.5")
    );
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 2);
    assert!(
        official_auth_headers
            .lock()
            .unwrap()
            .iter()
            .all(|header| header.as_deref() == Some("Bearer codex-oauth-token"))
    );
    assert!(
        official_account_headers
            .lock()
            .unwrap()
            .iter()
            .all(|header| header.as_deref() == Some("account-1"))
    );
    assert!(
        official_forwarded_headers
            .lock()
            .unwrap()
            .iter()
            .all(|headers| {
                headers
                    .get("originator")
                    .and_then(|value| value.to_str().ok())
                    == Some("codex_cli_rs")
                    && headers
                        .get("x-codex-originator")
                        .and_then(|value| value.to_str().ok())
                        == Some("legacy-codex")
                    && headers
                        .get("x-openai-subagent")
                        .and_then(|value| value.to_str().ok())
                        == Some("review")
                    && headers
                        .get("x-openai-memgen-request")
                        .and_then(|value| value.to_str().ok())
                        == Some("true")
            })
    );
}

#[tokio::test]
async fn reconnects_official_upstream_without_resetting_client_websocket() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, _, _, official_websocket_connections, _) =
        spawn_mock_official(OfficialWebSocketBehavior::CloseAfterCompletedWithCustomTool).await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();

    let mut official = responses_request();
    official.as_object_mut().unwrap().remove("stream");
    official["type"] = json!("response.create");
    official["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(official.to_string().into()))
        .await
        .unwrap();
    let first_frames = websocket_response_frames(&mut socket).await;
    let first_completed = first_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    let first_response_id = first_completed["response"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_output = first_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.output_item.done")
        .unwrap()["item"]
        .clone();
    assert_eq!(first_output["type"], "custom_tool_call");

    tokio::time::sleep(Duration::from_millis(25)).await;
    let incremental_input = json!({
        "type": "custom_tool_call_output",
        "call_id": first_output["call_id"],
        "output": "tool completed"
    });
    let mut follow_up = official.clone();
    follow_up["previous_response_id"] = json!(first_response_id);
    follow_up["input"] = json!([incremental_input.clone()]);
    socket
        .send(WsMessage::Text(follow_up.to_string().into()))
        .await
        .unwrap();
    let second_frames = tokio::time::timeout(
        Duration::from_secs(1),
        websocket_response_frames(&mut socket),
    )
    .await
    .expect("gateway did not finish the second official response");
    let second_joined = second_frames.join("\n");
    assert!(
        second_joined.contains("\"type\":\"response.completed\""),
        "unexpected second response frames: {second_joined}"
    );
    let second_completed = second_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    let second_response_id = second_completed["response"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second_output = second_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.output_item.done")
        .unwrap()["item"]
        .clone();

    tokio::time::sleep(Duration::from_millis(25)).await;
    let compaction_trigger = json!({"type":"compaction_trigger"});
    let mut compaction = official.clone();
    compaction["previous_response_id"] = json!(second_response_id);
    compaction["input"] = json!([compaction_trigger.clone()]);
    socket
        .send(WsMessage::Text(compaction.to_string().into()))
        .await
        .unwrap();
    let compaction_frames = tokio::time::timeout(
        Duration::from_secs(1),
        websocket_response_frames(&mut socket),
    )
    .await
    .expect("gateway did not finish compaction after reconnect");
    assert!(
        compaction_frames
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );

    let official_requests = official_requests.lock().unwrap();
    assert_eq!(official_requests.len(), 3);
    assert!(official_requests[1].get("previous_response_id").is_none());
    let mut expected_input = official["input"].as_array().unwrap().clone();
    expected_input.push(first_output);
    expected_input.push(incremental_input);
    assert_eq!(
        official_requests[1]["input"],
        Value::Array(expected_input.clone())
    );
    assert!(official_requests[2].get("previous_response_id").is_none());
    expected_input.push(second_output);
    expected_input.push(compaction_trigger);
    assert_eq!(official_requests[2]["input"], Value::Array(expected_input));
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn discards_official_connection_after_failed_and_incomplete_responses() {
    let (gateway_url, official_requests, official_websocket_connections, _codex_home) =
        spawn_gateway_with_mock_official(
            OfficialWebSocketBehavior::TerminalFailuresBeforeRecovery,
            Duration::from_secs(1),
        )
        .await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();

    let mut first = responses_request();
    first.as_object_mut().unwrap().remove("stream");
    first["type"] = json!("response.create");
    first["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(first.to_string().into()))
        .await
        .unwrap();
    let first_frames = websocket_response_frames(&mut socket).await;
    let first_response_id = first_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap()["response"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut failed = first.clone();
    failed["previous_response_id"] = json!(first_response_id);
    failed["input"] = json!([{
        "type": "message",
        "role": "user",
        "content": [{"type":"input_text","text":"trigger failed response"}]
    }]);
    socket
        .send(WsMessage::Text(failed.to_string().into()))
        .await
        .unwrap();
    let failed_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(failed_frames.contains("\"type\":\"response.failed\""));

    let mut incomplete = first.clone();
    incomplete["input"] = json!([{
        "type": "message",
        "role": "user",
        "content": [{"type":"input_text","text":"trigger incomplete response"}]
    }]);
    socket
        .send(WsMessage::Text(incomplete.to_string().into()))
        .await
        .unwrap();
    let incomplete_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(incomplete_frames.contains("\"type\":\"response.incomplete\""));

    let mut recovery = first.clone();
    recovery["input"] = json!([{
        "type": "message",
        "role": "user",
        "content": [{"type":"input_text","text":"recover on a fresh connection"}]
    }]);
    socket
        .send(WsMessage::Text(recovery.to_string().into()))
        .await
        .unwrap();
    let recovery_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(recovery_frames.contains("\"type\":\"response.completed\""));

    let official_requests = official_requests.lock().unwrap();
    assert_eq!(official_requests.len(), 4);
    assert!(official_requests[2].get("previous_response_id").is_none());
    assert!(official_requests[3].get("previous_response_id").is_none());
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn terminates_wrapped_official_error_without_waiting_for_close_handshake() {
    let (gateway_url, official_requests, official_websocket_connections, _codex_home) =
        spawn_gateway_with_mock_official(
            OfficialWebSocketBehavior::ConnectionLimitThenComplete,
            Duration::from_secs(1),
        )
        .await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();

    let mut body = responses_request();
    body.as_object_mut().unwrap().remove("stream");
    body["type"] = json!("response.create");
    body["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();
    let error_frames = tokio::time::timeout(
        Duration::from_secs(1),
        websocket_response_frames(&mut socket),
    )
    .await
    .expect("wrapped error was not forwarded promptly")
    .join("\n");
    assert!(error_frames.contains("websocket_connection_limit_reached"));

    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();
    let recovery_frames = tokio::time::timeout(
        Duration::from_secs(1),
        websocket_response_frames(&mut socket),
    )
    .await
    .expect("gateway remained blocked after wrapped error")
    .join("\n");
    assert!(recovery_frames.contains("\"type\":\"response.completed\""));
    assert_eq!(official_requests.lock().unwrap().len(), 2);
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn times_out_silent_official_websocket() {
    let (gateway_url, _, official_websocket_connections, _codex_home) =
        spawn_gateway_with_mock_official(
            OfficialWebSocketBehavior::Silent,
            Duration::from_millis(30),
        )
        .await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = responses_request();
    body["type"] = json!("response.create");
    body["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let frames = tokio::time::timeout(
        Duration::from_secs(1),
        websocket_response_frames(&mut socket),
    )
    .await
    .expect("silent official websocket did not time out")
    .join("\n");
    assert!(frames.contains("\"type\":\"response.failed\""));
    assert!(frames.contains("idle timeout waiting for official websocket"));
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn times_out_official_websocket_handshake() {
    let (gateway_url, _, _, _codex_home) = spawn_gateway_with_mock_official(
        OfficialWebSocketBehavior::SlowHandshake,
        Duration::from_millis(30),
    )
    .await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = responses_request();
    body["type"] = json!("response.create");
    body["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let frames = tokio::time::timeout(
        Duration::from_secs(1),
        websocket_response_frames(&mut socket),
    )
    .await
    .expect("official websocket handshake did not time out")
    .join("\n");
    assert!(frames.contains("\"type\":\"response.failed\""));
    assert!(frames.contains("official websocket connect timed out"));
}

#[tokio::test]
async fn preserves_official_prewarm_and_full_request_semantics() {
    let (gateway_url, official_requests, official_websocket_connections, _codex_home) =
        spawn_gateway_with_mock_official(
            OfficialWebSocketBehavior::Persistent,
            Duration::from_secs(1),
        )
        .await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();

    let mut prewarm = responses_request();
    prewarm.as_object_mut().unwrap().remove("stream");
    prewarm["type"] = json!("response.create");
    prewarm["model"] = json!("gpt-5.5");
    prewarm["generate"] = json!(false);
    socket
        .send(WsMessage::Text(prewarm.to_string().into()))
        .await
        .unwrap();
    let prewarm_frames = websocket_response_frames(&mut socket).await;
    assert!(
        prewarm_frames
            .iter()
            .all(|frame| !frame.contains("response.output_item.done"))
    );
    let prewarm_response_id = prewarm_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap()["response"]["id"]
        .clone();

    let mut incremental = prewarm.clone();
    incremental.as_object_mut().unwrap().remove("generate");
    incremental["previous_response_id"] = prewarm_response_id.clone();
    incremental["input"] = json!([]);
    socket
        .send(WsMessage::Text(incremental.to_string().into()))
        .await
        .unwrap();
    assert!(
        websocket_response_frames(&mut socket)
            .await
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );

    let mut full = prewarm.clone();
    full.as_object_mut().unwrap().remove("generate");
    full["input"] = json!([{
        "type": "message",
        "role": "user",
        "content": [{"type":"input_text","text":"non-prefix full request"}]
    }]);
    socket
        .send(WsMessage::Text(full.to_string().into()))
        .await
        .unwrap();
    assert!(
        websocket_response_frames(&mut socket)
            .await
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );

    let official_requests = official_requests.lock().unwrap();
    assert_eq!(official_requests.len(), 3);
    assert_eq!(official_requests[0]["generate"], false);
    assert_eq!(
        official_requests[1]["previous_response_id"],
        prewarm_response_id
    );
    assert_eq!(official_requests[1]["input"], json!([]));
    assert!(official_requests[2].get("previous_response_id").is_none());
    assert_eq!(official_requests[2]["input"], full["input"]);
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn keeps_client_websocket_open_after_partial_official_response_disconnect() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, _, _, official_websocket_connections, _) =
        spawn_mock_official(OfficialWebSocketBehavior::CloseAfterCreated).await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();

    let mut official = responses_request();
    official.as_object_mut().unwrap().remove("stream");
    official["type"] = json!("response.create");
    official["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(official.to_string().into()))
        .await
        .unwrap();
    let failed_frames = tokio::time::timeout(
        Duration::from_secs(1),
        websocket_response_frames(&mut socket),
    )
    .await
    .expect("gateway did not report the interrupted official response");
    let failed_events = failed_frames
        .iter()
        .map(|frame| serde_json::from_str::<Value>(frame).unwrap())
        .collect::<Vec<_>>();
    let created_id = failed_events
        .iter()
        .find(|event| event["type"] == "response.created")
        .and_then(|event| event.pointer("/response/id"))
        .and_then(Value::as_str)
        .unwrap();
    let failed_id = failed_events
        .iter()
        .find(|event| event["type"] == "response.failed")
        .and_then(|event| event.pointer("/response/id"))
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(failed_id, created_id);

    let mut custom = responses_request();
    custom.as_object_mut().unwrap().remove("stream");
    custom["type"] = json!("response.create");
    socket
        .send(WsMessage::Text(custom.to_string().into()))
        .await
        .unwrap();
    assert!(
        websocket_response_frames(&mut socket)
            .await
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );
    assert_eq!(official_requests.lock().unwrap().len(), 1);
    assert_eq!(upstream_requests.lock().unwrap().len(), 1);
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn keeps_custom_websocket_open_after_noop_request() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    socket
        .send(WsMessage::Text(
            json!({
                "type": "response.create",
                "model": "DeepSeek-V4-Flash",
                "generate": false,
                "input": []
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let warmup_frames = tokio::time::timeout(
        Duration::from_secs(1),
        websocket_response_frames(&mut socket),
    )
    .await
    .expect("noop response timed out");
    let warmup = warmup_frames.join("\n");
    assert!(warmup.contains("\"type\":\"response.created\""));
    assert!(warmup.contains("\"type\":\"response.completed\""));
    let warmup_completed: Value = warmup_frames
        .iter()
        .filter_map(|frame| serde_json::from_str(frame).ok())
        .find(|event: &Value| event["type"] == "response.completed")
        .unwrap();

    let mut body = responses_request();
    body["type"] = json!("response.create");
    body["previous_response_id"] = warmup_completed["response"]["id"].clone();
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let joined = websocket_response_frames(&mut socket).await.join("\n");
    assert!(joined.contains("\"type\":\"response.completed\""));
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn ignores_unavailable_web_search_without_resetting_custom_websocket() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut search_body = responses_request();
    search_body["type"] = json!("response.create");
    search_body["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"web_search","external_web_access":true}));
    socket
        .send(WsMessage::Text(search_body.to_string().into()))
        .await
        .unwrap();

    let search_frames = tokio::time::timeout(
        Duration::from_secs(1),
        websocket_response_frames(&mut socket),
    )
    .await
    .expect("search compatibility response timed out")
    .join("\n");
    assert!(search_frames.contains("\"type\":\"response.completed\""));
    assert!(!search_frames.contains("\"type\":\"response.failed\""));
    assert_eq!(
        requests.lock().unwrap()[0]["tools"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let mut valid_body = responses_request();
    valid_body["type"] = json!("response.create");
    socket
        .send(WsMessage::Text(valid_body.to_string().into()))
        .await
        .unwrap();
    let completed_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(completed_frames.contains("\"type\":\"response.completed\""));
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn returns_request_error_without_resetting_custom_websocket() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut invalid_body = responses_request();
    invalid_body["type"] = json!("response.create");
    invalid_body["tools"] = json!([{"type":"computer_use_preview"}]);
    socket
        .send(WsMessage::Text(invalid_body.to_string().into()))
        .await
        .unwrap();

    let error_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(error_frames.contains("\"type\":\"response.failed\""));
    assert!(error_frames.contains("unsupported tool type: computer_use_preview"));

    let mut valid_body = responses_request();
    valid_body["type"] = json!("response.create");
    socket
        .send(WsMessage::Text(valid_body.to_string().into()))
        .await
        .unwrap();
    let completed_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(completed_frames.contains("\"type\":\"response.completed\""));
    assert_eq!(requests.lock().unwrap().len(), 1);
}

async fn websocket_response_frames<S>(socket: &mut S) -> Vec<String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let mut frames = Vec::new();
    while let Some(message) = socket.next().await {
        match message.unwrap() {
            WsMessage::Text(text) => {
                let text = text.to_string();
                let terminal = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|event| event.get("type").and_then(Value::as_str).map(str::to_owned))
                    .is_some_and(|event_type| {
                        matches!(
                            event_type.as_str(),
                            "response.completed"
                                | "response.failed"
                                | "response.incomplete"
                                | "error"
                        )
                    });
                frames.push(text);
                if terminal {
                    break;
                }
            }
            WsMessage::Close(_) => break,
            _ => {}
        }
    }
    frames
}

#[tokio::test]
async fn forwards_thinking_and_anthropic_server_web_search() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::WebSearchRetry).await;
    let mut config = test_config(upstream_url);
    config.thinking_mode = ThinkingMode::Auto;
    config.enable_web_search_tool = true;
    config.web_search_max_uses = Some(1);
    let capability_dir = tempfile::tempdir().unwrap();
    let capabilities = WebSearchCapabilities::load(
        capability_dir.path().join("web-search-capabilities.json"),
        &config,
    )
    .unwrap();
    let mut models = vec![ModelInfo {
        id: "Claude Sonnet 5".to_owned(),
        ..ModelInfo::default()
    }];
    capabilities
        .probe_models(&mut models, &config, true)
        .await
        .unwrap();
    let probe_requests = requests.lock().unwrap().clone();
    assert_eq!(probe_requests.len(), 1 + 1);
    assert_eq!(probe_requests[0]["tools"].as_array().unwrap().len(), 2);
    assert_eq!(probe_requests[0]["tool_choice"]["type"], "tool");
    assert_eq!(probe_requests[0]["tool_choice"]["name"], "web_search");
    assert_eq!(probe_requests[0]["tools"][0]["name"], "web_search");
    assert_eq!(
        probe_requests[0]["tools"][1]["name"],
        "codex_mixin_probe_noop"
    );
    requests.lock().unwrap().clear();
    let state = AppState::with_web_search_capabilities(config, capabilities).unwrap();
    let gateway_url = spawn_router(router(state)).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["model"] = json!("Claude Sonnet 5");
    request["reasoning"] = json!({"effort": "xhigh"});
    request["input"] = json!([
        {"type":"message","role":"developer","content":[{"type":"input_text","text":"dev rules"}]},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"Teach me KDA attention"}]},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"KDA keeps a recurrent matrix state."}]},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"Continue polishing the lesson"}]}
    ]);
    request["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"web_search","external_web_access":true}));

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("response.completed"));
    assert!(body.contains("\"type\":\"web_search_call\""));
    assert!(body.contains("\"query\":\"OpenAI Codex\""));

    let supported_requests = requests.lock().unwrap().clone();
    assert_eq!(supported_requests.len(), 3);
    assert_ne!(supported_requests[0]["tool_choice"]["type"], "tool");
    assert_eq!(supported_requests[1]["tool_choice"]["type"], "tool");
    assert_eq!(supported_requests[1]["tool_choice"]["name"], "web_search");
    assert_eq!(supported_requests[2]["tool_choice"]["type"], "tool");
    assert_eq!(supported_requests[2]["tool_choice"]["name"], "web_search");
    let upstream_request = supported_requests[2].clone();
    assert_eq!(upstream_request["thinking"]["type"], "adaptive");
    assert_eq!(upstream_request["output_config"]["effort"], "max");
    assert_eq!(upstream_request["system"].as_array().unwrap().len(), 2);
    assert_eq!(upstream_request["messages"].as_array().unwrap().len(), 3);
    assert_eq!(
        upstream_request["messages"][0]["content"][0]["text"],
        "Teach me KDA attention"
    );
    assert_eq!(
        upstream_request["messages"][2]["content"][0]["text"],
        "Continue polishing the lesson"
    );
    assert_eq!(upstream_request["tools"].as_array().unwrap().len(), 2);
    assert_eq!(upstream_request["tools"][0]["name"], "exec_command");
    assert_eq!(upstream_request["tools"][1]["type"], "web_search_20250305");
    assert_eq!(upstream_request["tools"][1]["name"], "web_search");
    assert_eq!(upstream_request["tools"][1]["max_uses"], 1);
    assert!(upstream_request["tools"][1].get("input_schema").is_none());

    let mut unsupported_request = responses_request();
    unsupported_request["model"] = json!("DeepSeek-V4-Flash");
    unsupported_request["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"web_search","external_web_access":true}));
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&unsupported_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.text().await.unwrap();

    let upstream_request = requests.lock().unwrap()[3].clone();
    assert_eq!(upstream_request["tools"].as_array().unwrap().len(), 1);
    assert_ne!(upstream_request["tools"][0]["name"], "web_search");
}

#[tokio::test]
async fn proxies_codex_image_generation_to_official_backend() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, auth_headers, account_headers, _, _) =
        spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let mut config = test_config(upstream_url);
    config.upstream_image_generation_path = Some("/v1/images/generations".to_owned());
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let request = json!({
        "prompt": "draw the Codex logo",
        "model": "gpt-image-2",
        "background": "auto",
        "quality": "auto",
        "size": "auto"
    });

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .bearer_auth("gateway-key")
        .header("x-codex-originator", "codex_cli_rs")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"][0]["b64_json"], "b2ZmaWNpYWwtaW1hZ2U=");
    assert!(upstream_requests.lock().unwrap().is_empty());
    assert_eq!(official_requests.lock().unwrap().as_slice(), &[request]);
    assert_eq!(
        auth_headers.lock().unwrap().as_slice(),
        &[Some("Bearer codex-oauth-token".to_owned())]
    );
    assert_eq!(
        account_headers.lock().unwrap().as_slice(),
        &[Some("account-1".to_owned())]
    );
}

#[tokio::test]
async fn proxies_codex_image_edits_to_official_backend() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, auth_headers, account_headers, _, _) =
        spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let request = json!({
        "images": [{"image_url":"data:image/png;base64,aW1hZ2U="}],
        "prompt": "add a blue border",
        "model": "gpt-image-2",
        "background": "auto",
        "quality": "auto",
        "size": "auto"
    });

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/edits"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(official_requests.lock().unwrap().as_slice(), &[request]);
    assert_eq!(
        auth_headers.lock().unwrap().as_slice(),
        &[Some("Bearer codex-oauth-token".to_owned())]
    );
    assert_eq!(
        account_headers.lock().unwrap().as_slice(),
        &[Some("account-1".to_owned())]
    );
}

#[tokio::test]
async fn routes_official_gpt_and_custom_gpt_aliases_separately() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (
        official_url,
        official_requests,
        official_auth_headers,
        official_account_headers,
        _,
        official_forwarded_headers,
    ) = spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    let original_auth =
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#;
    std::fs::write(&config.codex_auth_path, original_auth).unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();

    let mut official_request = responses_request();
    official_request["model"] = json!("gpt-5.5");
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("originator", "codex_cli_rs")
        .header("x-codex-originator", "legacy-codex")
        .header("x-openai-subagent", "review")
        .header("x-openai-memgen-request", "true")
        .json(&official_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(official_requests.lock().unwrap()[0]["model"], "gpt-5.5");
    assert_eq!(
        official_auth_headers.lock().unwrap()[0].as_deref(),
        Some("Bearer codex-oauth-token")
    );
    assert_eq!(
        official_account_headers.lock().unwrap()[0].as_deref(),
        Some("account-1")
    );
    {
        let forwarded_headers = official_forwarded_headers.lock().unwrap();
        assert_eq!(
            forwarded_headers[0]
                .get("originator")
                .and_then(|value| value.to_str().ok()),
            Some("codex_cli_rs")
        );
        assert_eq!(
            forwarded_headers[0]
                .get("x-codex-originator")
                .and_then(|value| value.to_str().ok()),
            Some("legacy-codex")
        );
        assert_eq!(
            forwarded_headers[0]
                .get("x-openai-subagent")
                .and_then(|value| value.to_str().ok()),
            Some("review")
        );
        assert_eq!(
            forwarded_headers[0]
                .get("x-openai-memgen-request")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
    }
    assert!(upstream_requests.lock().unwrap().is_empty());

    let mut custom_request = responses_request();
    custom_request["model"] = json!("gpt-5.5-custom");
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("codex-oauth-token")
        .json(&custom_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(official_requests.lock().unwrap().len(), 1);
    assert_eq!(upstream_requests.lock().unwrap()[0]["model"], "gpt-5.5");
    assert_eq!(
        std::fs::read_to_string(codex_home.path().join("auth.json")).unwrap(),
        original_auth
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn perf_smoke_handles_parallel_streams() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let started = Instant::now();
    let jobs = (0..100).map(|_| {
        let client = client.clone();
        let url = format!("{gateway_url}/v1/responses");
        async move {
            let response = client
                .post(url)
                .bearer_auth("gateway-key")
                .json(&responses_request())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = response.text().await.unwrap();
            assert!(body.contains("response.completed"));
        }
    });
    join_all(jobs).await;
    let elapsed = started.elapsed();
    let requests_per_second = 100.0 / elapsed.as_secs_f64();
    eprintln!("perf_smoke req/s: {requests_per_second:.2}, elapsed: {elapsed:?}");
    assert!(
        requests_per_second > 20.0,
        "gateway mock throughput too low: {requests_per_second:.2} req/s in {elapsed:?}"
    );
}
