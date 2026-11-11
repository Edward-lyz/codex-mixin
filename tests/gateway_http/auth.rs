use super::*;

#[tokio::test]
async fn public_peers_need_the_gateway_key() {
    use tower::ServiceExt;

    let (upstream_url, _requests) = spawn_mock_upstream(MockMode::Text).await;
    let database = tempfile::tempdir().unwrap();
    let public_peer: std::net::SocketAddr = "203.0.113.7:50000".parse().unwrap();
    let request = |key: Option<&str>| {
        let mut builder = axum::http::Request::post("/v1/responses")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(key) = key {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        let mut request = builder
            .body(Body::from(responses_request().to_string()))
            .unwrap();
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(public_peer));
        request
    };
    for configured_key in [Some("gateway-key"), None] {
        let mut config = test_config(upstream_url.clone());
        config.accept_codex_oauth = false;
        config.gateway_api_key = configured_key.map(str::to_owned);
        let state = AppState::with_env_lookup(config, |name| {
            (name == codex_mixin::context::DATABASE_ENV)
                .then(|| database.path().join("context.db").display().to_string())
        })
        .unwrap();
        let app = router(state);
        for key in [None, Some("copied-key"), Some("gateway-key")] {
            let response = app.clone().oneshot(request(key)).await.unwrap();
            let expected = if configured_key.is_some() && key == configured_key {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            };
            assert_eq!(
                response.status(),
                expected,
                "configured {configured_key:?}, sent {key:?}"
            );
        }
    }
}

#[tokio::test]
async fn responses_auth_obeys_build_feature() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.accept_codex_oauth = false;
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    for key in [None, Some("copied-key"), Some("gateway-key")] {
        let allowed = !cfg!(feature = "client-auth") || key == Some("gateway-key");
        let mut request = client.post(format!("{gateway_url}/v1/responses"));
        if let Some(key) = key {
            request = request.bearer_auth(key);
        }
        let response = request.json(&responses_request()).send().await.unwrap();
        if allowed {
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                response
                    .text()
                    .await
                    .unwrap()
                    .contains("response.completed")
            );
        } else {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let mut request = format!(
            "{}/v1/responses",
            gateway_url.replacen("http://", "ws://", 1)
        )
        .into_client_request()
        .unwrap();
        if let Some(key) = key {
            request.headers_mut().insert(
                header::AUTHORIZATION,
                format!("Bearer {key}").parse().unwrap(),
            );
        }
        let connection = connect_async(request).await;
        if allowed {
            let (mut socket, _) = connection.unwrap();
            let mut body = responses_request();
            body["type"] = json!("response.create");
            socket
                .send(WsMessage::Text(body.to_string().into()))
                .await
                .unwrap();
            let frames = tokio::time::timeout(
                Duration::from_secs(5),
                websocket_response_frames(&mut socket),
            )
            .await
            .unwrap()
            .join("\n");
            assert!(frames.contains("response.completed"), "{frames}");
        } else {
            match connection.unwrap_err() {
                tokio_tungstenite::tungstenite::Error::Http(response) => {
                    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                }
                error => panic!("unexpected WebSocket error: {error}"),
            }
        }
    }
    assert_eq!(
        requests.lock().unwrap().len(),
        if cfg!(feature = "client-auth") { 2 } else { 6 }
    );
}

#[tokio::test]
async fn claude_desktop_auth_obeys_build_feature() {
    use codex_mixin::clients::claude_desktop::route_id;
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.gateway_client_keys.claude_desktop = Some("desktop-key".to_owned());
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let request = json!({
        "model": route_id("DeepSeek-V4-Flash-custom"), "max_tokens": 1024,
        "stream": true, "messages": [{"role":"user", "content":"say hi"}]
    });
    let endpoint = format!("{gateway_url}/claude-desktop/v1/messages");
    for key in [None, Some("gateway-key"), Some("copied-key")] {
        let mut request_builder = client.post(&endpoint);
        if let Some(key) = key {
            request_builder = request_builder.bearer_auth(key);
        }
        let response = request_builder.json(&request).send().await.unwrap();
        if cfg!(feature = "client-auth") {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                response
                    .text()
                    .await
                    .unwrap()
                    .contains("event: message_stop")
            );
        }
    }
    assert_eq!(
        requests.lock().unwrap().len(),
        if cfg!(feature = "client-auth") { 0 } else { 3 }
    );
    requests.lock().unwrap().clear();
    let mut invalid = request.clone();
    invalid["model"] = json!("claude-sonnet-mixin-ff");
    let response = client
        .post(&endpoint)
        .bearer_auth("desktop-key")
        .json(&invalid)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(requests.lock().unwrap().is_empty());
    let response = client
        .post(&endpoint)
        .bearer_auth("desktop-key")
        .header("anthropic-beta", "context-1m-2025-08-07")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = response.text().await.unwrap();
    assert!(stream.contains("event: message_stop"), "{stream}");
    assert_eq!(requests.lock().unwrap()[0]["model"], "DeepSeek-V4-Flash");
    let response = client
        .get(format!("{gateway_url}/claude-desktop/v1/models"))
        .bearer_auth("desktop-key")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let models: Value = response.json().await.unwrap();
    assert!(models["data"].as_array().unwrap().iter().all(|model| {
        model["id"]
            .as_str()
            .unwrap()
            .starts_with("claude-sonnet-mixin-")
    }));
}
