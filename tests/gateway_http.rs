use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use codex_mixin::config::{
    GatewayConfig, ProviderPreset, ThinkingMode, UpstreamAuthHeader, UpstreamKind,
};
use codex_mixin::server::{AppState, router};
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
}

fn test_config(upstream_base_url: String) -> GatewayConfig {
    GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        provider_preset: ProviderPreset::Custom,
        upstream_kind: UpstreamKind::AnthropicMessages,
        upstream_base_url,
        upstream_messages_path: "/v1/messages".to_owned(),
        upstream_models_path: "/v1/models".to_owned(),
        upstream_api_key: "upstream-key".to_owned(),
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        official_oauth_token_url: "https://auth.openai.com/oauth/token".to_owned(),
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

async fn spawn_mock_official() -> (
    String,
    Arc<Mutex<Vec<Value>>>,
    Arc<Mutex<Vec<Option<String>>>>,
    Arc<Mutex<Vec<Option<String>>>>,
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let auth_headers = Arc::new(Mutex::new(Vec::new()));
    let account_headers = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/responses", post(mock_official_responses))
        .route("/oauth/token", post(mock_oauth_token))
        .with_state(OfficialState {
            requests: requests.clone(),
            auth_headers: auth_headers.clone(),
            account_headers: account_headers.clone(),
        });
    (
        spawn_router(app).await,
        requests,
        auth_headers,
        account_headers,
    )
}

async fn mock_oauth_token(body: String) -> Response {
    assert!(body.contains("grant_type=refresh_token"));
    assert!(body.contains("refresh_token=refresh-token"));
    assert!(body.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"access_token":"official-access-token","refresh_token":"refresh-token-2"}"#,
        ))
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
            {"id": "Claude Sonnet 5", "object": "model", "created": 1, "owned_by": "custom"}
        ]
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
        MockMode::Tool => tool_sse(),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(payload))
        .unwrap()
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

fn tool_sse() -> String {
    [
        r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"DeepSeek-V4-Flash","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}}

"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"exec_command","input":{}}}

"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"pwd\"}"}}

"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}

"#,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}

"#,
        r#"event: message_stop
data: {"type":"message_stop"}

"#,
    ]
    .join("")
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
            {"type":"function","name":"exec_command","description":"run shell","parameters":{"type":"object","properties":{"cmd":{"type":"string"}}}},
            {"type":"web_search","external_web_access":true}
        ]
    })
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
async fn maps_text_stream_to_responses_sse() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
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
    assert!(body.contains("response.output_text.delta"));
    assert!(body.contains("hello codex"));
    assert!(body.contains("response.completed"));

    let upstream_request = requests.lock().unwrap()[0].clone();
    assert_eq!(upstream_request["model"], "DeepSeek-V4-Flash");
    assert_eq!(upstream_request["messages"][0]["role"], "user");
    assert_eq!(upstream_request["tools"].as_array().unwrap().len(), 1);
    assert!(
        upstream_request["system"][0]["text"]
            .as_str()
            .unwrap()
            .contains("You are Codex")
    );
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
async fn maps_custom_websocket_to_responses_frames() {
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
    let mut body = responses_request();
    body["type"] = json!("response.create");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let joined = websocket_response_frames(&mut socket).await.join("\n");
    assert!(joined.contains("\"type\":\"response.completed\""));
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
                let completed = text.contains("\"type\":\"response.completed\"");
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
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.thinking_mode = ThinkingMode::Auto;
    config.enable_web_search_tool = true;
    config.web_search_max_uses = Some(1);
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["model"] = json!("Claude Sonnet 5");
    request["reasoning"] = json!({"effort": "xhigh"});

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
async fn routes_official_gpt_and_custom_gpt_aliases_separately() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, official_auth_headers, official_account_headers) =
        spawn_mock_official().await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    config.official_oauth_token_url = format!("{official_url}/oauth/token");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"refresh_token":"refresh-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();

    let mut official_request = responses_request();
    official_request["model"] = json!("gpt-5.5");
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("codex-oauth-token")
        .header("chatgpt-account-id", "account-1")
        .json(&official_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(official_requests.lock().unwrap()[0]["model"], "gpt-5.5");
    assert_eq!(
        official_auth_headers.lock().unwrap()[0].as_deref(),
        Some("Bearer official-access-token")
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
    let updated_auth = std::fs::read_to_string(codex_home.path().join("auth.json")).unwrap();
    assert!(updated_auth.contains("refresh-token-2"));
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
