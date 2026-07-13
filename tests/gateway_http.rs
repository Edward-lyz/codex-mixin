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
use codex_mixin::config::{
    GatewayConfig, ProviderPreset, ThinkingMode, UpstreamAuthHeader, UpstreamKind,
};
use codex_mixin::server::{AppState, router};
use codex_mixin::sse::drain_events;
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
    NamespacedTool,
    CustomTool,
    ToolSearch,
    WebSearch,
    ImageTool,
    ImageToolFailure,
}

#[derive(Clone)]
struct MockState {
    mode: MockMode,
    requests: Arc<Mutex<Vec<Value>>>,
}

#[derive(Clone)]
struct OfficialState {
    requests: Arc<Mutex<Vec<Value>>>,
    auth_headers: Arc<Mutex<Vec<Option<String>>>>,
    account_headers: Arc<Mutex<Vec<Option<String>>>>,
    websocket_connections: Arc<AtomicUsize>,
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
        web_search_exclusive: true,
        web_search_omit_system_instructions: true,
        web_search_latest_user_only: true,
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
    let app = Router::new().route("/v1/models", get(mock_models)).route(
        "/openapi/v2/available_models",
        post(mock_baidu_available_models),
    );
    spawn_router(app).await
}

async fn spawn_session_required_upstream() -> String {
    let app = Router::new().route(
        "/v1/messages",
        post(|Json(body): Json<Value>| async move {
            if body["metadata"]["session_id"] != "stable-session" {
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

async fn spawn_mock_official() -> (
    String,
    Arc<Mutex<Vec<Value>>>,
    Arc<Mutex<Vec<Option<String>>>>,
    Arc<Mutex<Vec<Option<String>>>>,
    Arc<AtomicUsize>,
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let auth_headers = Arc::new(Mutex::new(Vec::new()));
    let account_headers = Arc::new(Mutex::new(Vec::new()));
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
            websocket_connections: websocket_connections.clone(),
        });
    (
        spawn_router(app).await,
        requests,
        auth_headers,
        account_headers,
        websocket_connections,
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
        }]
    }))
}

async fn mock_messages(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    assert_eq!(auth, Some("Bearer upstream-key"));
    state.requests.lock().unwrap().push(body);
    let payload = match state.mode {
        MockMode::Text => text_sse(),
        MockMode::Tool => tool_sse("exec_command", json!({"cmd":"pwd"})),
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
        MockMode::WebSearch => web_search_sse(),
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
    Json(body): Json<Value>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    assert_eq!(auth, Some("Bearer upstream-key"));
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
    ws.on_upgrade(move |socket| serve_mock_official_websocket(socket, state))
        .into_response()
}

async fn serve_mock_official_websocket(mut socket: WebSocket, state: OfficialState) {
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
        let response_id = format!("official_{}", state.requests.lock().unwrap().len());
        for status in ["in_progress", "completed"] {
            socket
                .send(AxumWsMessage::Text(
                    json!({
                        "type": if status == "completed" { "response.completed" } else { "response.created" },
                        "response": {
                            "id": response_id,
                            "object": "response",
                            "status": status,
                            "output": []
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        }
    }
}

fn text_sse() -> String {
    [
        r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"DeepSeek-V4-Flash","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}

"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}

"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" codex"}}

"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}

"#,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}

"#,
        r#"event: message_stop
data: {"type":"message_stop"}

"#,
    ]
    .join("")
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
async fn enriches_baidu_models_and_catalog_from_available_models() {
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
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"][0]["ratio"], "0.2x");
    assert_eq!(models["data"][0]["description"], "Fast coding model");
    assert_eq!(models["data"][0]["context_window"], 1_024_000);
    assert_eq!(models["data"].as_array().unwrap().len(), 2);
    assert_eq!(models["data"][1]["id"], "Kimi-K2.7-Code");
    assert_eq!(models["data"][1]["ratio"], "1.0x");

    let catalog: Value = client
        .get(format!("{gateway_url}/v1/codex-model-catalog"))
        .bearer_auth("gateway-key")
        .send()
        .await
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
    let (official_url, official_requests, auth_headers, account_headers, _) =
        spawn_mock_official().await;
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
    ) = spawn_mock_official().await;
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
                let completed = text.contains("\"type\":\"response.completed\"")
                    || text.contains("\"type\":\"response.failed\"");
                frames.push(text);
                if completed {
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
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::WebSearch).await;
    let mut config = test_config(upstream_url);
    config.thinking_mode = ThinkingMode::Auto;
    config.enable_web_search_tool = true;
    config.web_search_max_uses = Some(1);
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["model"] = json!("Claude Sonnet 5");
    request["reasoning"] = json!({"effort": "xhigh"});
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

    let upstream_request = requests.lock().unwrap()[0].clone();
    assert_eq!(upstream_request["thinking"]["type"], "adaptive");
    assert_eq!(upstream_request["output_config"]["effort"], "max");
    assert_eq!(upstream_request["tools"].as_array().unwrap().len(), 1);
    assert_eq!(upstream_request["tools"][0]["type"], "web_search_20250305");
    assert_eq!(upstream_request["tools"][0]["name"], "web_search");
    assert_eq!(upstream_request["tools"][0]["max_uses"], 1);
    assert!(upstream_request["tools"][0].get("input_schema").is_none());
}

#[tokio::test]
async fn proxies_codex_image_generation_to_official_backend() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, auth_headers, account_headers, _) =
        spawn_mock_official().await;
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
    let (official_url, official_requests, auth_headers, account_headers, _) =
        spawn_mock_official().await;
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
    let (official_url, official_requests, official_auth_headers, official_account_headers, _) =
        spawn_mock_official().await;
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
