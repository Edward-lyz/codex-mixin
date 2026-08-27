use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;

use super::auth::*;
use super::state::{provider_model_display_name, read_codex_official_auth};
use super::*;
use crate::benchmark::ModelBenchmarkManager;
use crate::config::ThinkingMode;
use crate::provider::{ProviderModel, custom_provider};
use crate::server::messages_http::normalize_message_request;

fn test_provider(base_url: String, model: &str) -> crate::provider::ProviderDefinition {
    let mut provider = custom_provider("test-provider", "upstream-key");
    provider.base_url = base_url;
    provider.protocol = crate::provider::ProviderProtocol::AnthropicMessages;
    provider.api_path = "/v1/messages".to_owned();
    provider.anthropic_version = Some("2023-06-01".to_owned());
    provider.selected_models = vec![model.to_owned()];
    provider.cached_models = vec![ProviderModel {
        id: model.to_owned(),
        ..ProviderModel::default()
    }];
    provider
}

#[test]
fn provider_model_display_name_keeps_the_model_id_visible() {
    assert_eq!(
        provider_model_display_name("gpt-5.6-sol", "Baidu OneAPI"),
        "gpt-5.6-sol · Baidu OneAPI"
    );
    assert_eq!(
        provider_model_display_name("gpt-5.6-sol", "AIHub"),
        "gpt-5.6-sol · AIHub"
    );
}

#[test]
fn messages_endpoint_rejects_non_anthropic_provider_protocols() {
    let mut provider = test_provider("https://example.invalid".to_owned(), "gpt-test");
    provider.protocol = crate::provider::ProviderProtocol::OpenAiChat;
    let state = AppState::new(GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: "https://example.invalid/responses".to_owned(),
        codex_auth_path: tempfile::tempdir().unwrap().path().join("auth.json"),
        gateway_api_key: None,
        accept_codex_oauth: false,
        official_selected_models: None,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(2),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    })
    .unwrap();
    let resolved = state.resolve_native_provider_model("gpt-test").unwrap();
    assert_eq!(resolved.provider.id(), "test-provider");
    assert_ne!(
        resolved
            .provider
            .protocol_for_model(resolved.upstream_model_id),
        crate::provider::ProviderProtocol::AnthropicMessages
    );
}

#[tokio::test]
async fn explicit_official_selection_rejects_unselected_models() {
    let directory = tempfile::tempdir().unwrap();
    let auth_path = directory.path().join("auth.json");
    tokio::fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"secret","account_id":"account-one"}}"#,
    )
    .await
    .unwrap();
    let state = AppState::new(GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: Vec::new(),
        official_responses_url: "https://example.invalid/responses".to_owned(),
        codex_auth_path: auth_path,
        gateway_api_key: None,
        accept_codex_oauth: true,
        official_selected_models: Some(vec!["gpt-5.6-sol".to_owned()]),
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(2),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    })
    .unwrap();

    let error = state.resolve_model_route("gpt-5.5").await.unwrap_err();

    assert!(matches!(
        error,
        GatewayError::BadRequest(message) if message.contains("not selected")
    ));
}

#[test]
fn native_message_request_normalizes_anthropic_fields() {
    let request = normalize_message_request(
        &json!({
            "model": "Claude Sonnet 5",
            "max_tokens": 1024,
            "stream": true,
            "system": "Be concise",
            "messages": [
                {"role": "user", "content": "hi"}
            ],
            "tools": [{"name": "bash", "description": "Run shell", "input_schema": {"type": "object"}}],
            "speed": "fast"
        }),
        "Claude Sonnet 5-baidu-oneapi",
    )
    .unwrap();
    assert_eq!(request.model, "Claude Sonnet 5-baidu-oneapi");
    assert_eq!(request.max_tokens, 1024);
    assert!(request.stream);
    assert_eq!(request.messages.len(), 1);
    assert_eq!(
        request.messages[0].content[0],
        crate::anthropic::ContentBlock::Text {
            text: "hi".to_owned()
        }
    );
    assert_eq!(request.system.as_ref().unwrap().len(), 1);
    assert_eq!(request.speed.as_deref(), Some("fast"));
}

#[tokio::test]
async fn gateway_auth_accepts_only_the_configured_key_or_actual_codex_oauth_token() {
    let directory = tempfile::tempdir().unwrap();
    let auth_path = directory.path().join("auth.json");
    tokio::fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"oauth-secret","account_id":"account"}}"#,
    )
    .await
    .unwrap();
    let state = AppState::new(GatewayConfig {
        bind: "127.0.0.1:8787".parse().unwrap(),
        providers: vec![test_provider(
            "https://example.invalid".to_owned(),
            "test-model",
        )],
        official_responses_url: "https://example.invalid/responses".to_owned(),
        codex_auth_path: auth_path,
        gateway_api_key: Some("gateway-secret".to_owned()),
        accept_codex_oauth: true,
        official_selected_models: None,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(2),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    })
    .unwrap();

    for token in ["gateway-secret", "oauth-secret"] {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        assert!(check_gateway_auth(&state, &headers).await.is_ok());
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, "Bearer arbitrary".parse().unwrap());
    assert!(matches!(
        check_gateway_auth(&state, &headers).await,
        Err(GatewayError::Unauthorized)
    ));
}

#[tokio::test]
async fn usage_endpoint_requires_auth_and_returns_recorded_provider_usage() {
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::with_usage_aggregator(
        GatewayConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            providers: Vec::new(),
            official_responses_url: "https://example.invalid/responses".to_owned(),
            codex_auth_path: directory.path().join("auth.json"),
            gateway_api_key: Some("gateway-key".to_owned()),
            accept_codex_oauth: false,
            official_selected_models: None,
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_secs(2),
            thinking_mode: ThinkingMode::Off,
            enable_web_search_tool: false,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            fusion_profiles: Vec::new(),
        },
        crate::gateway::TokenUsageAggregator::default(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });

    let client = Client::new();
    let unauthorized = client
        .get(format!("http://{address}/v1/usage"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let body: Value = client
        .get(format!("http://{address}/v1/usage"))
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, json!([]));
    let daily_body: Value = client
        .get(format!("http://{address}/v1/usage?days=1"))
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(daily_body, json!([]));
    let invalid_range = client
        .get(format!("http://{address}/v1/usage?days=0"))
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid_range.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn oneapi_routing_uses_stable_identifier_priority() {
    let mut headers = HeaderMap::new();
    headers.insert("session-id", "session-value".parse().unwrap());
    headers.insert("thread-id", "thread-value".parse().unwrap());
    headers.insert("x-client-request-id", "request-value".parse().unwrap());
    let body = json!({"prompt_cache_key":"session-value"});

    let routing = stable_oneapi_routing(&headers, &body).unwrap().unwrap();
    assert_eq!(routing.session_id, "thread-value");
    assert_eq!(
        routing.hash_key,
        Uuid::new_v5(&Uuid::NAMESPACE_URL, b"thread-id\0thread-value").to_string()
    );

    let override_routing =
        stable_oneapi_routing(&headers, &json!({"prompt_cache_key":"review-cache-value"}))
            .unwrap()
            .unwrap();
    assert_eq!(override_routing.session_id, "thread-value");
    assert_eq!(
        override_routing.hash_key,
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            b"thread-id\0thread-value\0prompt-cache-key\0review-cache-value",
        )
        .to_string()
    );

    headers.insert("x-openai-subagent", "review".parse().unwrap());
    let subagent_routing = stable_oneapi_routing(&headers, &body).unwrap().unwrap();
    assert_ne!(subagent_routing.hash_key, routing.hash_key);

    headers.remove("thread-id");
    headers.remove("x-openai-subagent");
    headers.insert("x-session-id", "x-session-value".parse().unwrap());
    assert_eq!(
        stable_oneapi_routing(&headers, &body)
            .unwrap()
            .unwrap()
            .session_id,
        "session-value"
    );

    let body = json!({});
    assert_eq!(
        stable_oneapi_routing(&headers, &body)
            .unwrap()
            .unwrap()
            .session_id,
        "x-session-value"
    );
    headers.remove("x-session-id");
    assert_eq!(
        stable_oneapi_routing(&headers, &body)
            .unwrap()
            .unwrap()
            .session_id,
        "session-value"
    );
    headers.remove("session-id");
    assert!(stable_oneapi_routing(&headers, &body).unwrap().is_none());

    headers.clear();
    assert_eq!(
        stable_oneapi_routing(&headers, &json!({"prompt_cache_key":"cache-value"}))
            .unwrap()
            .unwrap()
            .session_id,
        "cache-value"
    );
    assert!(
        stable_oneapi_routing(&headers, &json!({}))
            .unwrap()
            .is_none()
    );
    assert!(
        stable_oneapi_routing(&headers, &json!({"prompt_cache_key":null}))
            .unwrap()
            .is_none()
    );
    assert!(stable_oneapi_routing(&headers, &json!({"prompt_cache_key":1})).is_err());
}

#[tokio::test]
async fn official_auth_cache_refreshes_and_does_not_hide_invalid_files() {
    let directory = tempfile::tempdir().unwrap();
    let auth_path = directory.path().join("auth.json");
    let cache = tokio::sync::Mutex::new(None);
    tokio::fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"first","account_id":"account-one"}}"#,
    )
    .await
    .unwrap();

    let (authorization, account_id) = read_codex_official_auth(&auth_path, &cache).await.unwrap();
    assert_eq!(authorization, "Bearer first");
    assert_eq!(account_id, "account-one");

    tokio::fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"second-longer","account_id":"account-two"}}"#,
    )
    .await
    .unwrap();
    let (authorization, account_id) = read_codex_official_auth(&auth_path, &cache).await.unwrap();
    assert_eq!(authorization, "Bearer second-longer");
    assert_eq!(account_id, "account-two");

    tokio::fs::write(&auth_path, b"{").await.unwrap();
    assert!(read_codex_official_auth(&auth_path, &cache).await.is_err());
}

#[tokio::test]
async fn fetches_official_models_with_codex_auth_and_client_version() {
    let captured = Arc::new(Mutex::new(None));
    let captured_request = Arc::clone(&captured);
    let upstream =
        Router::new().route(
            "/backend-api/codex/models",
            get(
                move |headers: HeaderMap,
                      axum::extract::Query(query): axum::extract::Query<
                    HashMap<String, String>,
                >| {
                    let captured_request = Arc::clone(&captured_request);
                    async move {
                        *captured_request.lock().unwrap() = Some((
                            headers
                                .get(header::AUTHORIZATION)
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .to_owned(),
                            headers
                                .get("chatgpt-account-id")
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .to_owned(),
                            query.get("client_version").unwrap().to_owned(),
                        ));
                        Json(json!({
                            "models": [{
                                "slug": "gpt-5.6-sol",
                                "context_window": 272000,
                                "max_context_window": 272000
                            }]
                        }))
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let auth_path = directory.path().join("auth.json");
    tokio::fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"secret","account_id":"account-one"}}"#,
    )
    .await
    .unwrap();
    let state = AppState::new(GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: vec![test_provider(
            "https://example.invalid".to_owned(),
            "test-model",
        )],
        official_responses_url: format!("http://{address}/backend-api/codex/responses"),
        codex_auth_path: auth_path,
        gateway_api_key: None,
        accept_codex_oauth: true,
        official_selected_models: None,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(2),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    })
    .unwrap();

    let catalog = state
        .fetch_official_models_catalog("0.144.4")
        .await
        .unwrap();

    assert_eq!(catalog["models"][0]["context_window"], 272_000);
    assert_eq!(
        captured.lock().unwrap().as_ref().unwrap(),
        &(
            "Bearer secret".to_owned(),
            "account-one".to_owned(),
            "0.144.4".to_owned()
        )
    );
}

#[tokio::test]
async fn benchmark_api_runs_after_the_start_request_returns_and_persists_results() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured_requests = Arc::clone(&requests);
    let model_requests = Arc::new(AtomicUsize::new(0));
    let captured_model_requests = Arc::clone(&model_requests);
    let upstream = Router::new()
        .route(
            "/v1/models",
            get(move || {
                let captured_model_requests = Arc::clone(&captured_model_requests);
                async move {
                    captured_model_requests.fetch_add(1, Ordering::Relaxed);
                    Json(json!({
                        "object":"list",
                        "data":[{"id":"benchmark-model","object":"model"}]
                    }))
                }
            }),
        )
        .route(
            "/v1/messages",
            post(move |Json(body): Json<Value>| {
                let captured_requests = Arc::clone(&captured_requests);
                async move {
                    captured_requests.lock().unwrap().push(body);
                    let stream = async_stream::stream! {
                        yield Ok::<_, Infallible>(Bytes::from(concat!(
                            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\n",
                            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n"
                        )));
                        tokio::time::sleep(Duration::from_millis(15)).await;
                        yield Ok::<_, Infallible>(Bytes::from(
                            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"x\"}}\n\n"
                        ));
                        tokio::time::sleep(Duration::from_millis(15)).await;
                        yield Ok::<_, Infallible>(Bytes::from(
                            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"y\"}}\n\n"
                        ));
                        tokio::time::sleep(Duration::from_millis(15)).await;
                        yield Ok::<_, Infallible>(Bytes::from(concat!(
                            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},\"usage\":{\"output_tokens\":100}}\n\n",
                            "data: {\"type\":\"message_stop\"}\n\n"
                        )));
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from_stream(stream))
                        .unwrap()
                }
            }),
        );
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(upstream_listener, upstream).await.unwrap();
    });

    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_address = gateway_listener.local_addr().unwrap();
    let results_directory = tempfile::tempdir().unwrap();
    let results_path = results_directory.path().join("model-benchmarks.json");
    let mut state = AppState::new(GatewayConfig {
        bind: gateway_address,
        providers: vec![test_provider(
            format!("http://{upstream_address}"),
            "benchmark-model",
        )],
        official_responses_url: "https://example.invalid/responses".to_owned(),
        codex_auth_path: results_directory.path().join("auth.json"),
        gateway_api_key: Some("gateway-key".to_owned()),
        accept_codex_oauth: false,
        official_selected_models: None,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(2),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    })
    .unwrap();
    state.benchmarks = ModelBenchmarkManager::new(results_path.clone());
    tokio::spawn(async move {
        axum::serve(gateway_listener, router(state)).await.unwrap();
    });

    let client = Client::new();
    for _ in 0..2 {
        client
            .get(format!("http://{gateway_address}/v1/models"))
            .bearer_auth("gateway-key")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }
    assert_eq!(model_requests.load(Ordering::Relaxed), 0);
    let started: Value = client
        .post(format!("http://{gateway_address}/v1/model-benchmarks"))
        .bearer_auth("gateway-key")
        .json(&json!({"timeout_seconds":1}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(started["snapshot"]["status"], "running");

    for _ in 0..100 {
        let response: Value = client
            .get(format!("http://{gateway_address}/v1/model-benchmarks"))
            .bearer_auth("gateway-key")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        if response["snapshot"]["status"] == "completed" {
            assert_eq!(response["snapshot"]["results"][0]["output_tokens"], 100);
            assert!(response["snapshot"]["results"][0]["tps"].is_number());
            assert!(results_path.exists());
            let request = &requests.lock().unwrap()[0];
            assert_eq!(request["max_tokens"], 100);
            assert_eq!(
                request["messages"][0]["content"][0]["text"],
                crate::benchmark::BENCHMARK_PROMPT
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("benchmark API did not finish");
}
