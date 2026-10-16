use axum::body::Body;
use axum::http::{Request, StatusCode};
use codex_mixin::config::{GatewayConfig, ThinkingMode};
use codex_mixin::provider::custom_provider;
use codex_mixin::server::{AppState, router};
use std::time::Duration;
use tower::ServiceExt;
const ORIGIN: &str = "http://10.252.145.171:8723";
fn config() -> GatewayConfig {
    let mut provider = custom_provider("custom", "upstream-key");
    provider.base_url = "http://127.0.0.1:9".to_owned();
    GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        official_ech_proxy: false,
        official_ech_fallback_reason: None,
        codex_auth_path: std::path::PathBuf::from("/nonexistent/cors-test-auth.json"),
        gateway_api_key: Some("gateway-key".to_owned()),
        cors_allowed_origins: codex_mixin::config::StoredGatewayConfig::default()
            .cors_allowed_origins,
        gateway_client_keys: Default::default(),
        accept_codex_oauth: false,
        official_selected_models: None,
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
#[tokio::test]
async fn cors_preflight_precedes_auth() {
    let app = router(AppState::new(config()).unwrap());
    for path in [
        "/v1/models",
        "/v1/messages",
        "/v1/chat/completions",
        "/missing",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri(path)
                    .header("origin", ORIGIN)
                    .header("access-control-request-method", "POST")
                    .header(
                        "access-control-request-headers",
                        "authorization,content-type,x-custom-header",
                    )
                    .header("access-control-request-private-network", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{path}");
        assert_eq!(response.headers()["access-control-allow-origin"], ORIGIN);
        assert_eq!(
            response.headers()["access-control-allow-private-network"],
            "true"
        );
        assert_eq!(
            response.headers()["access-control-allow-headers"],
            "authorization,content-type,x-custom-header"
        );
    }
}

#[tokio::test]
async fn cors_covers_success_and_errors() {
    let app = router(AppState::new(config()).unwrap());
    for (method, path, expected) in [
        ("GET", "/healthz", StatusCode::OK),
        ("GET", "/missing", StatusCode::NOT_FOUND),
        ("GET", "/v1/messages", StatusCode::METHOD_NOT_ALLOWED),
        ("POST", "/v1/messages", StatusCode::BAD_REQUEST),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("origin", ORIGIN)
                    .header("authorization", "Bearer gateway-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()["access-control-allow-origin"], ORIGIN);
        assert_eq!(
            response.headers()["access-control-allow-methods"],
            "GET, POST, OPTIONS"
        );
        assert_eq!(response.headers()["access-control-max-age"], "86400");
        assert!(
            response.headers()["access-control-allow-headers"]
                .to_str()
                .unwrap()
                .contains("anthropic-beta")
        );
        assert!(
            response.headers()["access-control-allow-headers"]
                .to_str()
                .unwrap()
                .contains("x-goog-api-key")
        );
        assert!(
            !response
                .headers()
                .contains_key("access-control-allow-credentials")
        );
        assert!(
            !response
                .headers()
                .contains_key("access-control-allow-private-network")
        );
        assert!(
            response
                .headers()
                .get_all("vary")
                .iter()
                .any(|value| value == "Origin")
        );
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/usage")
                .header("origin", ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        if cfg!(feature = "client-auth") {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::OK
        }
    );
    assert_eq!(response.headers()["access-control-allow-origin"], ORIGIN);
}

#[tokio::test]
async fn cors_rejects_untrusted_origins() {
    let app = router(AppState::new(config()).unwrap());
    for origin in [
        "http://evil.example",
        "null",
        "*",
        "",
        "http://10.252.145.171:8723/",
        "http://10.252.145.171:8724",
        "http://user@10.252.145.171:8723",
        "http://10.252.145.171:8723 http://evil.example",
    ] {
        for method in ["GET", "POST", "OPTIONS"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/healthz")
                        .header("origin", origin)
                        .header("access-control-request-private-network", "true")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{origin:?}: {method}"
            );
            assert!(
                !response
                    .headers()
                    .contains_key("access-control-allow-origin")
            );
            assert!(
                !response
                    .headers()
                    .contains_key("access-control-allow-private-network")
            );
        }
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .header("origin", ORIGIN)
                .header("origin", "http://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn cors_native_and_local_same_origin() {
    let mut config = config();
    config.bind = "0.0.0.0:8787".parse().unwrap();
    let app = router(AppState::new(config).unwrap());
    for host in ["127.0.0.1:8787", "localhost:8787", "[::1]:8787"] {
        for origin in [None, Some(format!("http://{host}"))] {
            let mut request = Request::builder().uri("/healthz").header("host", host);
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                !response
                    .headers()
                    .contains_key("access-control-allow-origin")
            );
        }
    }
    for host in ["attacker.example:8787", "127.0.0.1:9999", "0.0.0.0:8787"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .header("host", host)
                    .header("origin", format!("http://{host}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{host}");
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
}

#[tokio::test]
async fn cors_allowlist_replace_disable() {
    for origins in [Vec::new(), vec!["https://other.example".to_owned()]] {
        let mut config = config();
        config.cors_allowed_origins.clone_from(&origins);
        let app = router(AppState::new(config).unwrap());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .header("origin", ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        if let Some(origin) = origins.first() {
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/healthz")
                        .header("origin", origin)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["access-control-allow-origin"], origin);
        }
    }
}

#[tokio::test]
async fn cors_methods_and_pna_scope() {
    let app = router(AppState::new(config()).unwrap());
    for method in ["PUT", "DELETE", "PATCH", "HEAD"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/healthz")
                    .header("origin", ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers()["access-control-allow-origin"], ORIGIN);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/healthz")
                    .header("origin", ORIGIN)
                    .header("access-control-request-method", method)
                    .header("access-control-request-private-network", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers()["access-control-allow-origin"], ORIGIN);
        assert!(
            !response
                .headers()
                .contains_key("access-control-allow-private-network")
        );
    }
    for pna in [None, Some("false"), Some("true")] {
        let mut request = Request::builder()
            .method("OPTIONS")
            .uri("/v1/models")
            .header("origin", ORIGIN)
            .header("access-control-request-method", "GET");
        if let Some(value) = pna {
            request = request.header("access-control-request-private-network", value);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response
                .headers()
                .contains_key("access-control-allow-private-network"),
            pna == Some("true")
        );
    }
}

#[tokio::test]
async fn cors_streams_without_buffering() {
    use axum::response::Response;
    use axum::routing::post;
    use bytes::Bytes;
    use codex_mixin::provider::{ProviderModel, ProviderModelSource, ProviderProtocol};
    use http_body_util::BodyExt;
    use serde_json::json;
    use std::convert::Infallible;
    use std::sync::Arc;

    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let upstream_release = Arc::clone(&release);
    let upstream = axum::Router::new().route("/v1/messages", post(move || {
        let release = Arc::clone(&upstream_release);
        async move {
            let events = async_stream::stream! {
                yield Ok::<_, Infallible>(Bytes::from_static(concat!(
                    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_cors\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"test\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                ).as_bytes()));
                // The test only releases completion after receiving the first frame.
                release.acquire().await.unwrap().forget();
                yield Ok::<_, Infallible>(Bytes::from_static(concat!(
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"cors streamed\"}}\n\n",
                    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
                    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                ).as_bytes()));
            };
            Response::builder().header("content-type", "text/event-stream")
                .body(Body::from_stream(events)).unwrap()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });
    let mut config = config();
    let provider = &mut config.providers[0];
    provider.base_url = format!("http://{address}");
    provider.protocol = ProviderProtocol::AnthropicMessages;
    provider.api_path = "/v1/messages".to_owned();
    provider.model_source = ProviderModelSource::Static;
    provider.cached_models = vec![ProviderModel {
        id: "test".to_owned(),
        ..Default::default()
    }];
    provider.selected_models = vec!["test".to_owned()];
    let app = router(AppState::new(config).unwrap());
    for path in ["/v1/messages", "/v1/chat/completions", "/v1/responses"] {
        let request = if path == "/v1/responses" {
            json!({"model":"test-custom", "stream":true, "input":"hello"})
        } else {
            json!({"model":"test-custom", "stream":true, "max_tokens":32, "messages":[{"role":"user", "content":"hello"}]})
        };
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            app.clone().oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("origin", ORIGIN)
                    .header("authorization", "Bearer gateway-key")
                    .header("content-type", "application/json")
                    .body(Body::from(request.to_string()))
                    .unwrap(),
            ),
        )
        .await
        .expect("SSE headers must not wait for completion")
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()["access-control-allow-origin"], ORIGIN);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );
        let mut body = response.into_body();
        let first = tokio::time::timeout(Duration::from_secs(3), body.frame())
            .await
            .expect("first SSE frame must arrive before upstream completion")
            .unwrap()
            .unwrap();
        assert!(!first.into_data().unwrap().is_empty());
        release.add_permits(1);
        let rest = tokio::time::timeout(Duration::from_secs(3), body.collect())
            .await
            .unwrap()
            .unwrap()
            .to_bytes();
        let rest = String::from_utf8(rest.to_vec()).unwrap();
        assert!(rest.contains("cors streamed"), "{path}: {rest}");
        let terminal = match path {
            "/v1/messages" => "message_stop",
            "/v1/chat/completions" => "[DONE]",
            _ => "response.completed",
        };
        assert!(rest.contains(terminal), "{path}: {rest}");
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
