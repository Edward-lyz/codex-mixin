use super::*;

#[tokio::test]
async fn claude_desktop_converts_openai_chat_stream_and_response() {
    use codex_mixin::clients::claude_desktop::route_id;
    let (upstream_url, requests) = spawn_mock_openai_chat().await;
    let mut config = test_config(upstream_url);
    configure_openai_chat(&mut config, "/chat/completions");
    config.gateway_client_keys.claude_desktop = Some("desktop-key".to_owned());
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    for stream in [true, false] {
        let response = client
            .post(format!("{gateway_url}/claude-desktop/v1/messages"))
            .bearer_auth("desktop-key")
            .json(&json!({
                "model": format!("{}[1m]", route_id("gpt-5.6-sol-custom")),
                "max_tokens":1024, "stream":stream,
                "messages":[{"role":"user","content":"say hi"}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        if stream {
            let events = response.text().await.unwrap();
            assert!(events.contains("event: message_stop"), "{events}");
            assert!(events.contains("hello"), "{events}");
        } else {
            let message: Value = response.json().await.unwrap();
            assert_eq!(message["content"][0]["text"], "hello openai");
            assert_eq!(message["stop_reason"], "end_turn");
        }
    }
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request["model"] == "gpt-5.6-sol")
    );
}

#[tokio::test]
async fn api_base_probe_returns_real_models() {
    let gateway = spawn_gateway("http://127.0.0.1:1".to_owned()).await;
    let client = reqwest::Client::new();
    let response = client
        .get(format!("{gateway}/v1"))
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["object"], "list");
    assert!(!value["data"].as_array().unwrap().is_empty());
    let cross_origin = client
        .get(format!("{gateway}/v1"))
        .header("Origin", "https://any-app.example")
        .bearer_auth("gateway-key")
        .send()
        .await
        .unwrap();
    assert_eq!(cross_origin.status(), StatusCode::OK);
    assert_eq!(
        cross_origin.headers()["access-control-allow-origin"],
        "https://any-app.example"
    );
}

#[tokio::test]
async fn flat_model_aliases_preserve_upstream_ids_across_http_apis() {
    let (upstream, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream);
    config.providers[0].cached_models = vec![ProviderModel {
        id: "org/model".to_owned(),
        ..Default::default()
    }];
    config.providers[0].selected_models = vec!["org/model".to_owned()];
    let gateway = spawn_gateway_with_config(config).await;
    let alias = format!(
        "mixin-flat-v1-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("org/model-custom")
    );
    let client = reqwest::Client::new();
    for (path, body) in [
        (
            "chat/completions",
            json!({"model":alias,"messages":[{"role":"user","content":"hello"}],"stream":false}),
        ),
        (
            "responses",
            json!({"model":alias,"input":"hello","stream":true}),
        ),
        (
            "messages",
            json!({"model":alias,"messages":[{"role":"user","content":"hello"}],"max_tokens":32,"stream":true}),
        ),
    ] {
        let response = client
            .post(format!("{gateway}/v1/{path}"))
            .bearer_auth("gateway-key")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{path}: {text}");
    }
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 3);
    assert!(sent.iter().all(|body| body["model"] == "org/model"));
}

#[tokio::test]
async fn chat_endpoint_returns_json_or_sse_and_real_usage() {
    let (upstream, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway = spawn_gateway(upstream).await;
    for streaming in [false, true] {
        let response = reqwest::Client::new()
            .post(format!("{gateway}/v1/chat/completions"))
            .bearer_auth("gateway-key")
            .json(&json!({"model":"DeepSeek-V4-Flash-custom","messages":[{"role":"system","content":"policy"},{"role":"user","content":"hello"}],"stream":streaming,"stream_options":{"include_usage":true}}))
            .send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .to_owned();
        let text = response.text().await.unwrap();
        if streaming {
            assert!(content_type.starts_with("text/event-stream"));
            let mut buffer = text.as_bytes().to_vec();
            let content = drain_events(&mut buffer)
                .into_iter()
                .filter(|event| event.data != "[DONE]")
                .map(|event| serde_json::from_str::<Value>(&event.data).unwrap())
                .filter_map(|value| {
                    value
                        .pointer("/choices/0/delta/content")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect::<String>();
            assert_eq!(content, "hello codex");
            assert!(text.contains("\"finish_reason\":\"stop\""));
            assert!(text.contains("\"usage\""));
            assert!(text.ends_with("data: [DONE]\n\n"));
        } else {
            assert!(content_type.starts_with("application/json"));
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["object"], "chat.completion");
            assert_eq!(value["choices"][0]["message"]["content"], "hello codex");
            assert_eq!(value["choices"][0]["finish_reason"], "stop");
            assert!(value["usage"]["total_tokens"].as_u64().unwrap() > 0);
        }
    }
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn chat_endpoint_maps_real_function_calls() {
    let (upstream, _) = spawn_mock_upstream(MockMode::Tool).await;
    let gateway = spawn_gateway(upstream).await;
    let response = reqwest::Client::new().post(format!("{gateway}/v1/chat/completions"))
        .bearer_auth("gateway-key")
        .json(&json!({"model":"DeepSeek-V4-Flash-custom","messages":[{"role":"user","content":"read file"}],"tools":[{"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}}]}))
        .send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["choices"][0]["finish_reason"], "tool_calls");
    assert!(
        !value["choices"][0]["message"]["tool_calls"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn chat_replays_deepseek_reasoning_through_responses_and_chat_upstreams() {
    for protocol in [
        ProviderProtocol::OpenAiResponses,
        ProviderProtocol::OpenAiChat,
    ] {
        let (upstream, requests) = if protocol == ProviderProtocol::OpenAiChat {
            spawn_mock_openai_chat().await
        } else {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let app = Router::new()
                .route(
                    "/responses",
                    post(
                        |State(requests): State<Arc<Mutex<Vec<Value>>>>,
                         Json(body): Json<Value>| async move {
                            requests.lock().unwrap().push(body);
                            let response = json!({"type":"response.completed","response":{
                                "id":"resp_replay","status":"completed",
                                "output":[{"id":"msg1","type":"message","role":"assistant",
                                    "content":[{"type":"output_text","text":"hello"}]}]
                            }});
                            (
                                [(header::CONTENT_TYPE, "text/event-stream")],
                                format!("event: response.completed\ndata: {response}\n\n"),
                            )
                        },
                    ),
                )
                .with_state(requests.clone());
            (spawn_router(app).await, requests)
        };
        let mut config = test_config(upstream);
        config.providers[0].protocol = protocol;
        config.providers[0].api_path = if protocol == ProviderProtocol::OpenAiChat {
            "/chat/completions"
        } else {
            "/responses"
        }
        .to_owned();
        let gateway = spawn_gateway_with_config(config).await;
        for streaming in [false, true] {
            let response = reqwest::Client::new()
                .post(format!("{gateway}/v1/chat/completions"))
                .bearer_auth("gateway-key")
                .json(&json!({
                    "model":"DeepSeek-V4-Flash-custom","stream":streaming,
                    "messages":[
                        {"role":"user","content":"Read a file."},
                        {"role":"assistant","content":null,"reasoning_content":"Inspect the file first.",
                            "tool_calls":[{"id":"cindy_call","type":"function","function":{"name":"read_file","arguments":"{}"}}]},
                        {"role":"tool","tool_call_id":"cindy_call","content":"synthetic file"}
                    ],
                    "tools":[{"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}}]
                }))
                .send()
                .await
                .unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{protocol:?}: {text}");
            assert!(text.contains("hello"), "{text}");
        }
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 2);
        for request in sent.iter() {
            if protocol == ProviderProtocol::OpenAiChat {
                let messages = request["messages"].as_array().unwrap();
                assert_eq!(messages.len(), 3);
                assert_eq!(messages[1]["reasoning_content"], "Inspect the file first.");
                assert_eq!(messages[1]["tool_calls"][0]["id"], "cindy_call");
                assert_eq!(messages[2]["tool_call_id"], "cindy_call");
            } else {
                assert_eq!(request["input"][1]["type"], "reasoning");
                assert_eq!(
                    request["input"][1]["content"][0]["text"],
                    "Inspect the file first."
                );
                assert_eq!(request["input"][2]["call_id"], "cindy_call");
                assert_eq!(request["input"][3]["call_id"], "cindy_call");
            }
        }
    }
}

#[tokio::test]
async fn chat_rejects_unsupported_semantics_before_upstream_call() {
    let (upstream, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway = spawn_gateway(upstream).await;
    let response = reqwest::Client::new().post(format!("{gateway}/v1/chat/completions"))
        .bearer_auth("gateway-key")
        .json(&json!({"model":"DeepSeek-V4-Flash-custom","messages":[{"role":"user","content":"hello"}],"n":2}))
        .send().await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(requests.lock().unwrap().is_empty());
}

pub(crate) async fn spawn_mock_gemini() -> (String, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .fallback(post(mock_gemini_generate_content))
        .with_state(requests.clone());
    (spawn_router(app).await, requests)
}

async fn mock_gemini_generate_content(
    axum::extract::State(requests): axum::extract::State<Arc<Mutex<Vec<Value>>>>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    assert_eq!(query.get("alt").map(String::as_str), Some("sse"));
    requests.lock().unwrap().push(body);
    let sse_body = "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello from Gemini!\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":4,\"totalTokenCount\":9}}}\n\n";
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(sse_body))
        .unwrap()
}

#[tokio::test]
async fn routes_gemini_models_through_responses_provider() {
    let (upstream_url, requests) = spawn_mock_gemini().await;
    let mut provider = codex_mixin::provider::ProviderPreset::Agy.create("gemini-test", "test-key");
    // A routing fixture supplies the discovered model explicitly; presets have no seed catalog.
    provider.cached_models = vec![codex_mixin::provider::ProviderModel {
        id: "gemini-3.8-flash-high".to_owned(),
        ..Default::default()
    }];
    provider.selected_models = vec!["gemini-3.8-flash-high".to_owned()];
    provider.base_url = upstream_url;
    provider.api_path = "/v1beta/models/{model}:streamGenerateContent".to_owned();
    let config = GatewayConfig {
        providers: vec![provider],
        ..test_config("http://127.0.0.1:1".to_owned())
    };
    let gateway = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "gemini-3.8-flash-high-gemini-test",
            "stream": true,
            "input": "say hi"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let stream = response.text().await.unwrap();
    assert!(stream.contains("response.created"), "{stream}");
    assert!(stream.contains("Hello from Gemini!"), "{stream}");
    assert!(stream.contains("response.completed"), "{stream}");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["contents"][0]["parts"][0]["text"], "say hi");
}

async fn mock_gemini_tool_generate_content(
    axum::extract::State(requests): axum::extract::State<Arc<Mutex<Vec<Value>>>>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    assert_eq!(query.get("alt").map(String::as_str), Some("sse"));
    let mut reqs = requests.lock().unwrap();
    let turn = reqs.len();
    reqs.push(body);
    let sse_body = if turn == 0 {
        let payload = json!({
            "response": {
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": {
                                "name": "default_api__exec_command",
                                "args": { "cmd": "pwd" }
                            },
                            "thoughtSignature": "AQJyZWFsLXNpZ25hdHVyZS1ieXRlcwM="
                        }],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }]
            }
        });
        format!("data: {payload}\n\n")
    } else {
        let payload = json!({
            "response": {
                "candidates": [{
                    "content": {
                        "parts": [{
                            "text": "Done!"
                        }],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }]
            }
        });
        format!("data: {payload}\n\n")
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(sse_body))
        .unwrap()
}

#[tokio::test]
async fn gemini_preserves_thought_signature_and_tool_names_across_turns() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .fallback(post(mock_gemini_tool_generate_content))
        .with_state(requests.clone());
    let upstream_url = spawn_router(app).await;

    let mut provider =
        codex_mixin::provider::ProviderPreset::Agy.create("gemini-tool-test", "test-key");
    // A routing fixture supplies the discovered model explicitly; presets have no seed catalog.
    provider.cached_models = vec![codex_mixin::provider::ProviderModel {
        id: "gemini-3.8-flash-high".to_owned(),
        ..Default::default()
    }];
    provider.selected_models = vec!["gemini-3.8-flash-high".to_owned()];
    provider.base_url = upstream_url;
    provider.api_path = "/v1beta/models/{model}:streamGenerateContent".to_owned();
    let config = GatewayConfig {
        providers: vec![provider],
        ..test_config("http://127.0.0.1:1".to_owned())
    };
    let gateway = spawn_gateway_with_config(config).await;

    // Turn 1: request that produces a tool call
    let response = reqwest::Client::new()
        .post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "gemini-3.8-flash-high-gemini-tool-test",
            "stream": true,
            "input": "run pwd",
            "tools": [
                {
                    "type": "function",
                    "name": "exec_command",
                    "namespace": "default_api",
                    "description": "run shell command",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "cmd": { "type": "string" }
                        }
                    }
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let stream_text = response.text().await.unwrap();
    let mut call_id = String::new();
    for line in stream_text.lines() {
        if let Some(data) = line.strip_prefix("data: ")
            && let Ok(val) = serde_json::from_str::<Value>(data)
            && val.get("type").and_then(Value::as_str) == Some("response.output_item.added")
            && val["item"]["type"] == "function_call"
        {
            call_id = val["item"]["call_id"].as_str().unwrap().to_owned();
        }
    }
    assert!(
        call_id.starts_with("call_default_api__exec_command_ts_"),
        "call_id was: {call_id}"
    );

    // Turn 2: send function call and output back
    let response = reqwest::Client::new()
        .post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "gemini-3.8-flash-high-gemini-tool-test",
            "stream": true,
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": "run pwd"
                },
                {
                    "type": "function_call",
                    "call_id": call_id,
                    "name": "exec_command",
                    "namespace": "default_api",
                    "arguments": "{\"cmd\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": "/home/clf"
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let stream_text = response.text().await.unwrap();
    assert!(stream_text.contains("Done!"), "{stream_text}");

    let reqs = requests.lock().unwrap();
    assert_eq!(reqs.len(), 2);

    // Verify Turn 2 upstream request structure
    let turn2 = &reqs[1];
    let model_part = &turn2["contents"][1]["parts"][0];
    assert_eq!(
        model_part["functionCall"]["id"],
        "call_default_api__exec_command"
    );
    assert_eq!(
        model_part["functionCall"]["name"],
        "default_api__exec_command"
    );
    assert_eq!(
        model_part["thoughtSignature"],
        "AQJyZWFsLXNpZ25hdHVyZS1ieXRlcwM="
    );

    let user_part = &turn2["contents"][2]["parts"][0];
    assert_eq!(
        user_part["functionResponse"]["id"],
        "call_default_api__exec_command"
    );
    assert_eq!(
        user_part["functionResponse"]["name"],
        "default_api__exec_command"
    );
    assert_eq!(
        user_part["functionResponse"]["response"]["output"],
        "/home/clf"
    );
}

async fn mock_gemini_corrupted_sig_content(
    axum::extract::State(requests): axum::extract::State<Arc<Mutex<Vec<Value>>>>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    assert_eq!(query.get("alt").map(String::as_str), Some("sse"));
    let mut reqs = requests.lock().unwrap();
    let attempt = reqs.len();
    reqs.push(body.clone());
    if attempt == 0 {
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"error":{"code":400,"message":"Corrupted thought signature.","status":"INVALID_ARGUMENT"}}"#,
            ))
            .unwrap()
    } else {
        let parts = body["request"]["contents"][1]["parts"].as_array().unwrap();
        assert_eq!(
            parts[0]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
        let payload = json!({
            "response": {
                "candidates": [{
                    "content": {
                        "parts": [{
                            "text": "Self-healed successfully!"
                        }],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }]
            }
        });
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!("data: {payload}\n\n")))
            .unwrap()
    }
}

#[tokio::test]
async fn gemini_recovers_from_corrupted_thought_signature() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .fallback(post(mock_gemini_corrupted_sig_content))
        .with_state(requests.clone());
    let upstream_url = spawn_router(app).await;

    let mut provider =
        codex_mixin::provider::ProviderPreset::Agy.create("gemini-corrupted-test", "test-key");
    // A routing fixture supplies the discovered model explicitly; presets have no seed catalog.
    provider.cached_models = vec![codex_mixin::provider::ProviderModel {
        id: "gemini-3.8-flash-high".to_owned(),
        ..Default::default()
    }];
    provider.selected_models = vec!["gemini-3.8-flash-high".to_owned()];
    provider.base_url = upstream_url;
    provider.api_path = "/v1internal:streamGenerateContent".to_owned();
    let config = GatewayConfig {
        providers: vec![provider],
        ..test_config("http://127.0.0.1:1".to_owned())
    };
    let gateway = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "gemini-3.8-flash-high-gemini-corrupted-test",
            "stream": true,
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": "run pwd"
                },
                {
                    "type": "function_call",
                    "call_id": "call_1_ts_corrupted",
                    "name": "exec_command",
                    "namespace": "default_api",
                    "arguments": "{\"cmd\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1_ts_corrupted",
                    "output": "/home/clf"
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let stream_text = response.text().await.unwrap();
    assert!(
        stream_text.contains("Self-healed successfully!"),
        "{stream_text}"
    );

    let reqs = requests.lock().unwrap();
    assert_eq!(reqs.len(), 2, "must have retried once after 400");
}

#[tokio::test]
async fn agy_mixed_tools_complete() {
    let app = Router::new().route("/v1internal:streamGenerateContent", post(
        |axum::Json(body): axum::Json<Value>| async move {
            let request = &body["request"];
            let tools = request["tools"].as_array().unwrap();
            assert!(tools.iter().any(|tool| tool.get("functionDeclarations").is_some()));
            // The native endpoint rejects the combination even with the flag.
            if tools.iter().any(|tool| tool.get("googleSearch").is_some()) {
                return (StatusCode::BAD_REQUEST, axum::Json(json!({"error":{
                    "code":400,
                    "message":"Please enable tool_config.include_server_side_tool_invocations to use Built-in tools with Function calling.",
                    "status":"INVALID_ARGUMENT"
                }}))).into_response();
            }
            assert!(request.get("toolConfig").is_none());
            Response::builder().header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from("data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"OK\"}]},\"finishReason\":\"STOP\"}]}}\n\n"))
                .unwrap()
        }
    ));
    let mut provider = codex_mixin::provider::ProviderPreset::Agy.create("agy", "fixture-key");
    provider.base_url = spawn_router(app).await;
    provider.cached_models = vec![codex_mixin::provider::ProviderModel {
        id: "gemini-3.8-flash-high".to_owned(),
        supports_web_search: Some(true),
        supports_function_tools: Some(true),
        ..Default::default()
    }];
    provider.selected_models = vec!["gemini-3.8-flash-high".to_owned()];
    let gateway = spawn_gateway_with_config(GatewayConfig {
        providers: vec![provider],
        ..test_config("http://127.0.0.1:1".to_owned())
    })
    .await;
    let response = reqwest::Client::new().post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({"model":"gemini-3.8-flash-high-agy", "stream":true,
            "input":"hello", "tools":[{"type":"web_search"},
                {"type":"function", "name":"read_file", "parameters":{"type":"object","properties":{}}}]}))
        .send().await.unwrap();
    let output = response.text().await.unwrap();
    assert!(output.contains("response.completed"), "{output}");
    assert!(!output.contains("response.failed"), "{output}");
}
