use super::*;

#[tokio::test]
async fn routes_aws_bedrock_preset_through_mantle_messages() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url.clone());
    let mut provider = ProviderPreset::AwsBedrock.create("aws-bedrock", "upstream-key");
    provider.base_url = upstream_url;
    config.providers = vec![provider];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("anthropic.claude-sonnet-5-aws-bedrock");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["model"], "anthropic.claude-sonnet-5");
}

#[tokio::test]
async fn signs_aws_bedrock_requests_with_aksk() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url.clone());
    let mut provider = aws_bedrock_aksk_provider(
        "aws-bedrock",
        "AKIDEXAMPLE",
        "secret-example",
        Some("session-example".to_owned()),
        "us-east-1",
    );
    provider.base_url = upstream_url;
    config.providers = vec![provider];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("anthropic.claude-sonnet-5-aws-bedrock");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = requests.lock().unwrap();
    let authorization = requests[0]["__authorization"].as_str().unwrap();
    assert!(authorization.starts_with("AWS4-HMAC-SHA256 "));
    assert!(authorization.contains("Credential=AKIDEXAMPLE/"));
    assert!(!authorization.contains("secret-example"));
    assert_eq!(requests[0]["__aws_session_token"], "session-example");
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
    assert_eq!(models["data"][0]["id"], "DeepSeek-V4-Flash-custom");

    let catalog: Value = client
        .get(format!("{gateway_url}/v1/codex-model-catalog"))
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog["models"][1]["slug"], "Claude Sonnet 5-custom");
}

#[tokio::test]
async fn uses_baidu_available_models_as_authoritative_catalog_source() {
    let upstream_url = spawn_baidu_metadata_upstream().await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    config.providers[0].model_source = ProviderModelSource::BaiduOneApi;
    config.providers[0].cached_models.clear();
    config.providers[0].selected_models.clear();
    let discovered = discover_provider_models(&reqwest::Client::new(), &config.providers[0])
        .await
        .unwrap();
    apply_discovered_models(&mut config.providers[0], discovered).unwrap();
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
    let models = models["data"].as_array().unwrap();
    assert_eq!(models.len(), 3);
    let deepseek = models
        .iter()
        .find(|model| model["id"] == "DeepSeek-V4-Flash-custom")
        .unwrap();
    assert_eq!(deepseek["ratio"], "0.2x");
    assert_eq!(
        deepseek["display_name"],
        "DeepSeek V4 Flash (0.2x) \u{b7} oneapi"
    );
    assert_eq!(deepseek["description"], "Fast coding model");
    assert_eq!(deepseek["context_window"], 1_024_000);
    let kimi = models
        .iter()
        .find(|model| model["id"] == "Kimi-K2.7-Code-custom")
        .unwrap();
    assert_eq!(kimi["ratio"], "1.0x");
    let auto = models
        .iter()
        .find(|model| model["id"] == "auto-custom")
        .unwrap();
    assert_eq!(auto["description"], "Default model");
    assert_eq!(auto["context_window"], 200_000);

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
        .find(|model| model["slug"] == "DeepSeek-V4-Flash-custom")
        .unwrap();
    assert_eq!(
        model["description"],
        "Fast coding model | 0.2x | Value model"
    );
    assert_eq!(
        model["display_name"],
        "DeepSeek V4 Flash (0.2x) \u{b7} oneapi"
    );
    assert_eq!(model["context_window"], 1_024_000);
    assert_eq!(model["input_modalities"], json!(["text"]));
    let auto = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == "auto-custom")
        .unwrap();
    assert_eq!(auto["description"], "Default model | 1.0x | Value model");
    assert_eq!(auto["context_window"], 200_000);
}

#[tokio::test]
async fn isolates_http_and_websocket_routes_across_two_providers() {
    let alpha_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured_alpha = alpha_requests.clone();
    let alpha_url = spawn_router(Router::new().route(
        "/v1/messages",
        post(move |headers: HeaderMap, Json(mut body): Json<Value>| {
            let captured_alpha = captured_alpha.clone();
            async move {
                body["__authorization"] = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map_or(Value::Null, |value| json!(value));
                let failing = body["model"] == "broken";
                captured_alpha.lock().unwrap().push(body);
                if failing {
                    return (
                        StatusCode::TOO_MANY_REQUESTS,
                        axum::Json(json!({
                            "error": {
                                "message": "provider overloaded",
                                "type": "429001"
                            }
                        })),
                    )
                        .into_response();
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(text_sse()))
                    .unwrap()
            }
        }),
    ))
    .await;
    let beta_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured_beta = beta_requests.clone();
    let beta_url = spawn_router(Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, Json(mut body): Json<Value>| {
            let captured_beta = captured_beta.clone();
            async move {
                body["__authorization"] = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map_or(Value::Null, |value| json!(value));
                captured_beta.lock().unwrap().push(body);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(openai_chat_sse()))
                    .unwrap()
            }
        }),
    ))
    .await;

    let mut alpha = ProviderPreset::Custom.create("alpha", "alpha-key");
    alpha.base_url = alpha_url;
    alpha.protocol = ProviderProtocol::AnthropicMessages;
    alpha.api_path = "/v1/messages".to_owned();
    alpha.anthropic_version = Some("2023-06-01".to_owned());
    alpha.cached_models = ["shared", "hidden", "broken"]
        .into_iter()
        .map(|id| ProviderModel {
            id: id.to_owned(),
            ..ProviderModel::default()
        })
        .collect();
    alpha.selected_models = vec!["shared".to_owned(), "broken".to_owned()];
    let mut beta = ProviderPreset::Custom.create("beta", "beta-key");
    beta.base_url = beta_url;
    beta.protocol = ProviderProtocol::OpenAiChat;
    beta.api_path = "/v1/chat/completions".to_owned();
    beta.cached_models = vec![ProviderModel {
        id: "shared".to_owned(),
        ..ProviderModel::default()
    }];
    beta.selected_models = vec!["shared".to_owned()];
    let mut config = test_config("http://unused.invalid".to_owned());
    config.providers = vec![alpha, beta];
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
    let model_ids = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(model_ids, ["shared-alpha", "broken-alpha", "shared-beta"]);
    assert!(!model_ids.contains(&"hidden-alpha"));

    for model in ["shared-alpha", "shared-beta"] {
        let mut request = responses_request();
        request["model"] = json!(model);
        let response = client
            .post(format!("{gateway_url}/v1/responses"))
            .bearer_auth("gateway-key")
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{model}");
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("response.completed")
        );
    }

    let mut hidden = responses_request();
    hidden["model"] = json!("hidden-alpha");
    assert_eq!(
        client
            .post(format!("{gateway_url}/v1/responses"))
            .bearer_auth("gateway-key")
            .json(&hidden)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let mut broken = responses_request();
    broken["model"] = json!("broken-alpha");
    let broken_response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&broken)
        .send()
        .await
        .unwrap();
    assert_eq!(broken_response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        broken_response.json::<Value>().await.unwrap(),
        json!({"error":{"message":"provider overloaded","type":"429001"}})
    );
    let mut beta_after_failure = responses_request();
    beta_after_failure["model"] = json!("shared-beta");
    assert_eq!(
        client
            .post(format!("{gateway_url}/v1/responses"))
            .bearer_auth("gateway-key")
            .json(&beta_after_failure)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut websocket_request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    websocket_request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(websocket_request).await.unwrap();
    socket
        .send(WsMessage::Text(
            json!({
                "type": "response.create",
                "model": "shared-alpha",
                "generate": false,
                "input": []
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let alpha_frames = websocket_response_frames(&mut socket).await;
    let previous_response_id = alpha_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap()["response"]["id"]
        .clone();
    socket
        .send(WsMessage::Text(
            json!({
                "type": "response.create",
                "model": "shared-beta",
                "previous_response_id": previous_response_id,
                "generate": false,
                "input": []
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let cross_provider = websocket_response_frames(&mut socket).await.join("\n");
    assert!(cross_provider.contains("\"type\":\"response.failed\""));
    assert!(cross_provider.contains("belongs to model shared-alpha"));

    let alpha_requests = alpha_requests.lock().unwrap();
    assert!(
        alpha_requests
            .iter()
            .all(|body| body["__authorization"] == "Bearer alpha-key")
    );
    assert!(
        alpha_requests
            .iter()
            .all(|body| matches!(body["model"].as_str(), Some("shared" | "broken")))
    );
    let beta_requests = beta_requests.lock().unwrap();
    assert!(
        beta_requests
            .iter()
            .all(|body| body["__authorization"] == "Bearer beta-key")
    );
    assert!(beta_requests.iter().all(|body| body["model"] == "shared"));
}

#[tokio::test]
async fn opencode_go_sends_stable_session_headers() {
    let captured = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
    let requests = captured.clone();
    let upstream = spawn_router(Router::new().fallback(post(move |headers: HeaderMap, Json(body): Json<Value>| {
        let requests = requests.clone();
        async move {
            // Drain the streamed request before responding so Windows cannot
            // close the connection while the gateway is still writing it.
            assert_eq!(body["model"], "glm-5.2");
            requests.lock().unwrap().push(headers);
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"model\":\"glm-5.2\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0,\"total_tokens\":1}}}\n\n",
            )
        }
    }))).await;
    let mut config = test_config(upstream.clone());
    let mut provider =
        codex_mixin::provider::ProviderPreset::OpenCodeGo.create("opencode-go", "upstream-key");
    provider.base_url = upstream;
    provider.cached_models = vec![ProviderModel {
        id: "glm-5.2".to_owned(),
        ..Default::default()
    }];
    provider.selected_models = vec!["glm-5.2".to_owned()];
    config.providers = vec![provider];
    let gateway = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    for (path, body) in [
        (
            "/v1/responses",
            json!({"model":"glm-5.2","input":"hello","stream":true}),
        ),
        (
            "/v1/chat/completions",
            json!({"model":"glm-5.2","messages":[{"role":"user","content":"hello"}],"stream":true}),
        ),
        (
            "/v1/messages",
            json!({"model":"glm-5.2","messages":[{"role":"user","content":"hello"}],"max_tokens":16,"stream":true}),
        ),
    ] {
        let mut body = body;
        body["model"] = json!(codex_mixin::provider::catalog_model_slug(
            "glm-5.2",
            "opencode-go"
        ));
        for (name, session) in [
            ("x-opencode-session", "session-a"),
            ("x-opencode-session", "session-a"),
            ("x-opencode-session", "session-b"),
            ("thread-id", "thread-c"),
            ("session-id", "session-d"),
        ] {
            let response = client
                .post(format!("{gateway}{path}"))
                .bearer_auth("gateway-key")
                .header(name, session)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{path}: {text}");
            assert!(!text.contains("response.failed"), "{text}");
        }
    }
    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 15);
    for (index, headers) in requests.iter().enumerate() {
        let session = [
            "session-a",
            "session-a",
            "session-b",
            "thread-id\0thread-c",
            "session-d",
        ][index % 5];
        let expected =
            uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, session.as_bytes()).to_string();
        assert_eq!(
            headers
                .get("x-opencode-session")
                .and_then(|value| value.to_str().ok()),
            Some(expected.as_str())
        );
        assert!(
            headers[header::USER_AGENT]
                .to_str()
                .unwrap()
                .starts_with("codex-mixin/")
        );
        assert!(!headers.contains_key("x-hash-key"));
    }
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
async fn unsigned_thinking_is_a_normal_baidu_fallback() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::UnsignedThinking).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["model"] = json!("Claude Sonnet 5-custom");
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("response.reasoning_summary_text.delta"));
    assert!(!body.contains("codex-mixin:baidu-unsigned-thinking:v1:"));
    assert!(body.contains("response.completed"));
    assert!(!body.contains("response.failed"));
}

#[tokio::test]
async fn anthropic_signed_thinking_round_trips_across_http_turns() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Thinking).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model":"Claude Sonnet 5-custom",
            "stream":true,
            "input":"first turn"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let mut encoded = first.bytes().await.unwrap().to_vec();
    let events = drain_events(&mut encoded);
    let reasoning: Value = serde_json::from_str(
        &events
            .iter()
            .find(|event| {
                event.event.as_deref() == Some("response.output_item.done")
                    && event.data.contains("\"type\":\"reasoning\"")
            })
            .expect("first turn should return a reasoning item")
            .data,
    )
    .unwrap();

    let second = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model":"Claude Sonnet 5-custom",
            "stream":true,
            "input":[
                reasoning["item"].clone(),
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"second turn"}]}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let _ = second.bytes().await.unwrap();

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        recorded[1]["messages"][0]["content"][0],
        json!({
            "type":"thinking",
            "thinking":"checked constraints",
            "signature":"signed-state"
        })
    );
}
