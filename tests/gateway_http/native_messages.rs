use super::*;

#[tokio::test]
async fn anthropic_messages_ignores_legacy_model_level_endpoint() {
    let hits = Arc::new(Mutex::new(Vec::<String>::new()));
    let hits2 = Arc::clone(&hits);
    let upstream = Router::new().route(
        "/{*path}",
        // Read the request body before replying. Windows resets a socket that
        // closes with unread data, and the gateway then sees a send error.
        post(move |uri: OriginalUri, _body: bytes::Bytes| async move {
            hits2.lock().unwrap().push(uri.path().to_owned());
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(text_sse()))
                .unwrap()
        }),
    );
    let upstream_url = spawn_router(upstream).await;
    let mut config = test_config(upstream_url);
    config.providers[0].cached_models[0].api_path =
        Some("/v1/models/deepseek-v4/messages".to_owned());
    config.providers[0].selected_models = vec!["DeepSeek-V4-Flash".to_owned()];
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&responses_request())
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "gateway response: {body}");
    assert!(body.contains("response.completed"));
    let hits = hits.lock().unwrap();
    assert_eq!(hits.as_slice(), ["/v1/messages"]);
}

#[tokio::test]
async fn native_messages_runs_context_tools_with_session() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextStatusThenText).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway}/v1/messages"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "native-context")
        .header("anthropic-beta", "context-1m-2025-08-07")
        .json(&json!({"model":"DeepSeek-V4-Flash-custom","max_tokens":1024,"stream":true,"messages":[{"role":"user","content":"hello"}]}))
        .send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = response.text().await.unwrap();
    assert!(text.contains("hello") && text.contains(" codex"), "{text}");
    assert!(!text.contains("mixin_context"), "{text}");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0]["tools"]
            .to_string()
            .contains("mixin_context_status")
    );
    assert_eq!(requests[0]["__anthropic_beta"], "context-1m-2025-08-07");
}

#[tokio::test]
async fn native_messages_compression_keeps_summary_without_original_metadata() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextCompressThenText).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    config.context.protected_messages = 1;
    config.context.protected_tokens = 1;
    let database = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    let body = json!({"model":"DeepSeek-V4-Flash-custom","max_tokens":1024,"stream":true,
    "system":[{"type":"text","text":"rules","cache_control":{"type":"ephemeral"}}],
    "messages":[
        {"role":"user","content":"dev rules"},
        {"role":"user","content":"explore two ideas"},
        {"role":"assistant","content":[{"type":"text","text":format!("alpha {}", "word ".repeat(400)),"cache_control":{"type":"ephemeral"}}]},
        {"role":"assistant","content":[{"type":"text","text":format!("beta {}", "word ".repeat(400)),"cache_control":{"type":"ephemeral"}}]},
        {"role":"user","content":"continue"}
    ]});
    let response = reqwest::Client::new()
        .post(format!("{gateway}/v1/messages"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "native-compress")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = response.text().await.unwrap();
    assert!(
        text.contains("hello") && !text.contains("mixin_context"),
        "{text}"
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["system"][0], body["system"][0]);
    let blocks: Vec<_> = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["content"].as_array().unwrap())
        .collect();
    let summary = blocks
        .iter()
        .find(|block| {
            block["text"]
                .as_str()
                .is_some_and(|text| text.contains("<context_summary"))
        })
        .unwrap();
    assert!(
        summary["text"]
            .as_str()
            .unwrap()
            .contains("alpha and beta explored two ideas")
    );
    assert!(summary.get("cache_control").is_none());
    assert!(!requests[1]["messages"].to_string().contains("alpha word"));
}

#[tokio::test]
async fn native_messages_preserves_thinking_and_options_with_context() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Thinking).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    let body = json!({
        "model":"DeepSeek-V4-Flash-custom", "max_tokens":2048, "stream":false,
        "system":[{"type":"text","text":"rules","cache_control":{"type":"ephemeral"}}],
        "thinking":{"type":"enabled","budget_tokens":1024}, "output_config":{"effort":"high"},
        "metadata":{"user_id":"person"},
        "temperature":0.5, "stop_sequences":["STOP"],
        "tools":[{"name":"tool.with.punctuation","description":"test","input_schema":{"type":"object"},"cache_control":{"type":"ephemeral"}}],
        "messages":[
            {"role":"user","content":[{"type":"text","text":"hello","cache_control":{"type":"ephemeral"}}]},
            {"role":"assistant","content":[{"type":"thinking","thinking":"signed","signature":"sig"},{"type":"thinking","thinking":"unsigned"},{"type":"redacted_thinking","data":"opaque"},{"type":"tool_use","id":"call_1","name":"tool.with.punctuation","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"failed","is_error":true}]}
        ]
    });
    let response = reqwest::Client::new()
        .post(format!("{gateway}/v1/messages"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "native-thinking")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = response.json().await.unwrap();
    assert_eq!(
        result["content"][0],
        json!({"type":"thinking","thinking":"checked constraints","signature":"signed-state"})
    );
    assert_eq!(
        result["usage"],
        json!({"input_tokens":10,"output_tokens":8})
    );
    assert_eq!(result["stop_reason"], "end_turn");
    let captured = requests.lock().unwrap();
    let sent = &captured[0];
    assert_eq!(sent["system"][0], body["system"][0]);
    assert_eq!(sent["thinking"], body["thinking"]);
    assert_eq!(sent["output_config"], body["output_config"]);
    assert_eq!(sent["metadata"], body["metadata"]);
    assert_eq!(sent["temperature"], 0.5);
    assert_eq!(sent["stop_sequences"], json!(["STOP"]));
    assert_eq!(sent["tools"][0], body["tools"][0]);
    let blocks: Vec<_> = sent["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["content"].as_array().unwrap())
        .collect();
    for expected in body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["content"].as_array().unwrap())
    {
        assert!(blocks.contains(&expected), "missing {expected}: {blocks:?}");
    }
    assert!(!sent.to_string().contains("__mixin_native_block"));
}

#[tokio::test]
async fn native_messages_preserves_streaming_thinking_and_usage() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Thinking).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    let response = reqwest::Client::new().post(format!("{gateway}/v1/messages"))
        .bearer_auth("gateway-key").header("x-session-id", "native-stream")
        .json(&json!({"model":"DeepSeek-V4-Flash-custom","max_tokens":1024,"stream":true,"messages":[{"role":"user","content":"hello"}]})).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = response.text().await.unwrap();
    assert!(
        text.contains("thinking_delta") && text.contains("checked constraints"),
        "{text}"
    );
    assert!(
        text.contains("signature_delta") && text.contains("signed-state"),
        "{text}"
    );
    assert!(
        text.contains("\"input_tokens\":10") && text.contains("\"output_tokens\":8"),
        "{text}"
    );
}

#[tokio::test]
async fn native_messages_thinking_normalization_preserves_block_metadata() {
    let (upstream, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    let first = json!({"type":"thinking","thinking":"first","signature":"first-signature"});
    let last = json!({"type":"thinking","thinking":"last","signature":"last-signature"});
    let redacted = json!({"type":"redacted_thinking","data":"opaque"});
    let unsigned = json!({"type":"thinking","thinking":"unsigned"});
    let text = json!({"type":"text","text":"before compress","cache_control":{"type":"ephemeral"}});
    let tool = json!({"type":"tool_use","id":"client-call","name":"tool.with.punctuation","input":{"key":"value"}});
    let response = reqwest::Client::new().post(format!("{gateway}/v1/messages"))
        .bearer_auth("gateway-key").header("x-session-id", "native-normalization")
        .json(&json!({
            "model":"DeepSeek-V4-Flash-custom", "max_tokens":1024, "stream":true,
            "messages":[
                {"role":"user","content":"start"},
                {"role":"assistant","content":[first, text, last, redacted, unsigned, tool]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"client-call","content":"done"}]}
            ],
            "tools":[{"name":"tool.with.punctuation","description":"test tool","input_schema":{"type":"object"}}]
        })).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = response.text().await.unwrap();
    assert!(stream.contains("message_stop"), "{stream}");
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let assistant = captured[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "assistant")
        .unwrap();
    assert_eq!(
        assistant["content"],
        json!([first, last, redacted, unsigned, text, tool])
    );
    assert!(!captured[0].to_string().contains("__mixin_native_block"));
}

#[tokio::test]
async fn native_messages_bypasses_context_without_a_session_or_when_disabled() {
    for enabled in [false, true] {
        let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
        let mut config = test_config(upstream_url);
        config.context.enabled = enabled;
        let database = tempfile::tempdir().unwrap();
        let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
        let client = reqwest::Client::new();
        let mut request = client.post(format!("{gateway}/v1/messages")).bearer_auth("gateway-key")
            .json(&json!({"model":"DeepSeek-V4-Flash-custom","max_tokens":1024,"stream":true,"messages":[{"role":"user","content":"hello"}]}));
        if !enabled {
            request = request.header("x-session-id", "disabled");
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.text().await.unwrap();
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(!captured[0].to_string().contains("mixin_context"));
    }
}

#[tokio::test]
async fn native_messages_keeps_sessions_isolated() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    for (session, message) in [
        ("native-alpha", "private alpha"),
        ("native-beta", "private beta"),
    ] {
        let response = reqwest::Client::new().post(format!("{gateway}/v1/messages"))
            .bearer_auth("gateway-key").header("x-session-id", session)
            .json(&json!({"model":"DeepSeek-V4-Flash-custom","max_tokens":1024,"stream":true,"messages":[{"role":"user","content":message}]})).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.text().await.unwrap();
    }
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert!(
        captured[0]["messages"]
            .to_string()
            .contains("private alpha")
    );
    assert!(
        !captured[1]["messages"]
            .to_string()
            .contains("private alpha")
    );
    assert!(captured[1]["messages"].to_string().contains("private beta"));
}

#[tokio::test]
async fn native_messages_context_retries_image_413() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let upstream = spawn_router(Router::new().route(
        "/v1/messages",
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured);
            async move {
                let first = {
                    let mut requests = captured.lock().unwrap();
                    requests.push(body);
                    requests.len() == 1
                };
                if first {
                    return Response::builder()
                        .status(StatusCode::PAYLOAD_TOO_LARGE)
                        .body(Body::from("too large"))
                        .unwrap();
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
    let mut config = test_config(upstream);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    let image_url = png_data_url(3000, 1500);
    let data = image_url.split_once(";base64,").unwrap().1;
    let response = reqwest::Client::new().post(format!("{gateway}/v1/messages"))
        .bearer_auth("gateway-key").header("x-session-id", "native-413")
        .json(&json!({"model":"DeepSeek-V4-Flash-custom","max_tokens":1024,"stream":true,"messages":[{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":data}}]}]})).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.text().await.unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let source = &requests[1]["messages"][0]["content"][0]["source"];
    assert_eq!(source["media_type"], "image/jpeg");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(source["data"].as_str().unwrap())
        .unwrap();
    let image = image::load_from_memory(&decoded).unwrap();
    assert_eq!((image.width(), image.height()), (768, 384));
}

#[tokio::test]
async fn native_messages_hosted_tools_remain_passthrough() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let upstream = spawn_router(Router::new().route(
        "/v1/messages",
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured);
            async move {
                captured.lock().unwrap().push(body);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(web_search_sse()))
                    .unwrap()
            }
        }),
    ))
    .await;
    let mut config = test_config(upstream);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    let response = reqwest::Client::new().post(format!("{gateway}/v1/messages"))
        .bearer_auth("gateway-key").header("x-session-id", "native-hosted")
        .json(&json!({"model":"DeepSeek-V4-Flash-custom","max_tokens":1024,"stream":true,"tools":[{"type":"web_search_20250305","name":"web_search"}],"messages":[{"role":"user","content":"search"}]})).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = response.text().await.unwrap();
    assert!(
        stream.contains("server_tool_use") && stream.contains("web_search_tool_result"),
        "{stream}"
    );
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert!(!captured[0].to_string().contains("mixin_context"));
}

#[tokio::test]
async fn native_messages_opaque_history_and_server_tools_remain_passthrough() {
    for with_tool in [false, true] {
        let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
        let mut config = test_config(upstream_url);
        config.context.enabled = true;
        let database = tempfile::tempdir().unwrap();
        let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
        let mut body = json!({"model":"DeepSeek-V4-Flash-custom","max_tokens":1024,"stream":true,
            "messages":[{"role":"assistant","content":[{"type":"server_tool_use","id":"srv1","name":"code_execution","input":{"code":"1"}},{"type":"code_execution_tool_result","tool_use_id":"srv1","content":{"type":"code_execution_result","stdout":"1"}}]},{"role":"user","content":"next"}]});
        if with_tool {
            body["tools"] = json!([{"type":"code_execution_20250522","name":"code_execution"}]);
        }
        let response = reqwest::Client::new()
            .post(format!("{gateway}/v1/messages"))
            .bearer_auth("gateway-key")
            .header("x-session-id", "native-opaque")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.text().await.unwrap();
        let captured = requests.lock().unwrap();
        assert_eq!(captured[0]["messages"], body["messages"]);
        assert!(!captured[0].to_string().contains("mixin_context"));
    }
}

#[tokio::test]
async fn forwards_client_anthropic_beta_to_provider_messages() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.providers[0].anthropic_beta = Some("existing-beta".to_owned());
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let request = json!({
        "model": "DeepSeek-V4-Flash-custom",
        "max_tokens": 1024,
        "stream": true,
        "messages": [{"role": "user", "content": "say hi"}]
    });

    let enabled = client
        .post(format!("{gateway_url}/v1/messages"))
        .bearer_auth("gateway-key")
        .header("anthropic-beta", "context-1m-2025-08-07")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(enabled.status(), StatusCode::OK);
    let _ = enabled.text().await.unwrap();

    let plain = client
        .post(format!("{gateway_url}/v1/messages"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(plain.status(), StatusCode::OK);
    let _ = plain.text().await.unwrap();

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(
        captured[0]["__anthropic_beta"],
        "existing-beta,context-1m-2025-08-07"
    );
    assert_eq!(captured[1]["__anthropic_beta"], "existing-beta");
}
