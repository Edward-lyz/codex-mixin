use super::*;
use crate::config::{StoredGatewayConfig, ThinkingMode};
use crate::provider::{ProviderModel, ProviderProtocol, custom_provider};
use axum::http::Request;
use std::time::Duration;
use tower::ServiceExt;

async fn fixture() -> (Router, tokio::task::JoinHandle<()>) {
    let upstream = Router::new().route("/v1/chat/completions", post(|headers: HeaderMap, Json(body): Json<Value>| async move {
        assert_eq!(headers[header::AUTHORIZATION], "Bearer upstream-key");
        assert!(!headers.contains_key("x-goog-api-key"));
        assert_eq!(body["model"], "vendor/model");
        let used_tool = body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool");
        let events = if used_tool {
            assert!(body["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool" && m["content"].as_str().unwrap().contains("ok")));
            concat!("data: {\"id\":\"chat1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"chat1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n", "data: [DONE]\n\n")
        } else {
            concat!("data: {\"id\":\"chat1\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"chat1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\n", "data: [DONE]\n\n")
        };
        ([(header::CONTENT_TYPE,"text/event-stream")], events)
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let mut provider = custom_provider("gemini-test", "upstream-key");
    provider.base_url = format!("http://{address}");
    provider.protocol = ProviderProtocol::OpenAiChat;
    provider.api_path = "/v1/chat/completions".to_owned();
    provider.selected_models = vec!["vendor/model".to_owned()];
    provider.cached_models = vec![ProviderModel {
        id: "vendor/model".to_owned(),
        context_window: Some(64_000),
        max_output_tokens: Some(4_096),
        ..ProviderModel::default()
    }];
    let config = GatewayConfig {
        context: Default::default(),
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: "https://example.invalid/responses".to_owned(),
        codex_auth_path: tempfile::tempdir().unwrap().path().join("auth.json"),
        gateway_api_key: Some("client-key".to_owned()),
        cors_allowed_origins: StoredGatewayConfig::default().cors_allowed_origins,
        gateway_client_keys: crate::gateway_access::GatewayClientKeys::default(),
        accept_codex_oauth: false,
        official_ech_fallback_reason: None,
        official_ech_proxy: false,
        official_selected_models: None,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(5),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    };
    (super::super::router(AppState::new(config).unwrap()), task)
}

async fn request(app: &Router, path: &str, body: Value) -> Response {
    app.clone()
        .oneshot(
            Request::post(path)
                .header("x-goog-api-key", "client-key")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn json_body(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn gemini_http_tool_roundtrip_models_and_slash_routes() {
    let (app, task) = fixture().await;
    let response = app
        .clone()
        .oneshot(
            Request::get("/v1beta/models")
                .header("x-goog-api-key", "client-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let models = json_body(response).await;
    let name = models["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["name"].as_str())
        .find(|s| s.contains("vendor/model"))
        .unwrap();
    let prompt = json!({"contents":[{"role":"user","parts":[{"text":"read"}]}],"tools":[{"functionDeclarations":[{"name":"read","parameters":{"type":"OBJECT"}}]}]});
    let response = request(&app, &format!("/v1beta/{name}:generateContent"), prompt).await;
    assert_eq!(response.status(), StatusCode::OK);
    let call = json_body(response).await;
    assert_eq!(
        call["candidates"][0]["content"]["parts"][0]["functionCall"]["name"],
        "read"
    );
    let history = json!({"contents":[{"role":"user","parts":[{"text":"read"}]},call["candidates"][0]["content"],{"role":"user","parts":[{"functionResponse":{"id":call["candidates"][0]["content"]["parts"][0]["functionCall"]["id"],"name":"read","response":{"output":"ok"}}}]}]});
    let encoded_name = name.replace('/', "%2F");
    let response = request(
        &app,
        &format!(
            "/v1beta/{}:streamGenerateContent?alt=sse",
            encoded_name.replacen("models%2F", "models/", 1)
        ),
        history,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("done"), "{text}");
    assert!(text.contains("STOP"));
    assert!(text.contains("\"totalTokenCount\":7"));
    task.abort();
}

#[tokio::test]
async fn gemini_http_rejects_unsupported_fields_actions_and_auth() {
    let (app, task) = fixture().await;
    let response = request(
        &app,
        "/v1beta/models/vendor/model:generateContent",
        json!({"contents":[{"parts":[{"text":"hi"}]}],"safetySettings":[]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = request(&app, "/v1beta/models/vendor/model:countTokens", json!({})).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = request(
        &app,
        "/v1beta/models/vendor/model:streamGenerateContent",
        json!({}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    if cfg!(feature = "client-auth") {
        let response = app
            .oneshot(
                Request::post("/v1beta/models/vendor/model:generateContent")
                    .header("x-goog-api-key", "wrong")
                    .body(Body::from("invalid json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    task.abort();
}

#[tokio::test]
async fn gemini_token_estimates_and_muse_discovery_do_not_generate() {
    let (app, task) = fixture().await;
    let response = app
        .clone()
        .oneshot(
            Request::get("/muse-code/models")
                .header("authorization", "Bearer client-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let model = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| {
            m["id"]
                .as_str()
                .is_some_and(|id| id.contains("vendor/model"))
        })
        .unwrap();
    let meta = &model["metadata"]["muse-code"];
    assert_eq!(meta["is_hidden"], false);
    assert_eq!(meta["limit"]["context"], 64_000);
    assert_eq!(meta["limit"]["output"], 4_096);
    let model_id = model["id"].as_str().unwrap();
    let response = request(
        &app,
        &format!("/v1beta/models/{model_id}:countTokens"),
        json!({"contents":[{"parts":[{"text":"estimate this input without calling the model"}]}]}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-codex-mixin-token-count"], "estimated");
    assert!(json_body(response).await["totalTokens"].as_u64().unwrap() > 0);
    if cfg!(feature = "client-auth") {
        let response = app
            .oneshot(
                Request::get("/muse-code/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    task.abort();
}
