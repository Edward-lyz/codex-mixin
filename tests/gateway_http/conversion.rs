use super::*;

#[tokio::test]
async fn maps_fast_service_tier_to_anthropic_request_and_beta() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.providers[0].anthropic_beta = Some("existing-beta".to_owned());
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

/// A host that caps output at 262144 tokens rejects larger budgets with a
/// message naming its ceiling, as Baidu OneAPI does for GLM-5.3.
async fn spawn_output_limited_upstream() -> (String, Arc<Mutex<Vec<u64>>>) {
    let budgets = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&budgets);
    let app = Router::new().route(
        "/v1/messages",
        post(move |Json(body): Json<Value>| {
            let seen = Arc::clone(&seen);
            async move {
                let max_tokens = body["max_tokens"].as_u64().unwrap();
                seen.lock().unwrap().push(max_tokens);
                if max_tokens > 262_144 {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error":{"type":"invalid_request_error",
                            "message": format!("Range of max_tokens should be [1, 262144], got {max_tokens}")}})),
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
    );
    (spawn_router(app).await, budgets)
}

#[tokio::test]
async fn learns_the_provider_output_limit_when_the_default_budget_is_rejected() {
    let (upstream_url, budgets) = spawn_output_limited_upstream().await;
    let mut config = test_config(upstream_url);
    config.default_max_tokens = 512_000;
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    for _ in 0..2 {
        let response = client
            .post(format!("{gateway_url}/v1/responses"))
            .bearer_auth("gateway-key")
            .json(&responses_request())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.text().await.unwrap();
    }
    assert_eq!(*budgets.lock().unwrap(), [512_000, 262_144, 262_144]);

    let mut explicit = responses_request();
    explicit["max_output_tokens"] = json!(300_000);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&explicit)
        .send()
        .await
        .unwrap();
    assert!(
        !response.status().is_success(),
        "explicit client limits are never rewritten"
    );
    assert_eq!(budgets.lock().unwrap().last(), Some(&300_000));
}

#[tokio::test]
async fn uses_provider_model_output_limit_before_sending() {
    let (upstream_url, budgets) = spawn_output_limited_upstream().await;
    let mut config = test_config(upstream_url);
    config.default_max_tokens = codex_mixin::config::DEFAULT_MAX_OUTPUT_TOKENS;
    config.providers[0].cached_models[0].max_output_tokens = Some(131_072);
    let gateway_url = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&responses_request())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(*budgets.lock().unwrap(), [131_072]);
}

#[tokio::test]
async fn maps_stable_session_to_anthropic_metadata() {
    let upstream_url = spawn_session_required_upstream().await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
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
async fn configuration_updates_apply_before_translating_custom_requests() {
    for protocol in [
        ProviderProtocol::AnthropicMessages,
        ProviderProtocol::OpenAiChat,
        ProviderProtocol::GoogleGemini,
    ] {
        let (upstream_url, requests) = match protocol {
            ProviderProtocol::AnthropicMessages => spawn_mock_upstream(MockMode::Text).await,
            ProviderProtocol::OpenAiChat => spawn_mock_openai_chat().await,
            ProviderProtocol::GoogleGemini => spawn_mock_gemini().await,
            _ => unreachable!(),
        };
        let mut config = test_config(upstream_url);
        configure_baidu_policy(&mut config);
        config.thinking_mode = ThinkingMode::Auto;
        config.providers[0].cached_models[0].supports_thinking = Some(true);
        match protocol {
            ProviderProtocol::AnthropicMessages => {}
            ProviderProtocol::OpenAiChat => {
                configure_openai_chat(&mut config, "/chat/completions");
                configure_model_protocol(
                    &mut config,
                    "DeepSeek-V4-Flash",
                    protocol,
                    "/chat/completions",
                );
            }
            ProviderProtocol::GoogleGemini => {
                config.providers[0].protocol = protocol;
                config.providers[0].api_path =
                    "/v1beta/models/{model}:streamGenerateContent".to_owned();
                configure_model_protocol(
                    &mut config,
                    "DeepSeek-V4-Flash",
                    protocol,
                    "/v1beta/models/{model}:streamGenerateContent",
                );
            }
            _ => unreachable!(),
        }
        let gateway = spawn_gateway_with_config(config).await;
        let format = json!({
            "type":"json_schema", "name":"answer", "strict":true,
            "schema":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}
        });
        let body = json!({
            "model":"DeepSeek-V4-Flash-custom", "stream":true,
            "reasoning":{"effort":"low","summary":"auto"},
            "text":{"format":{"type":"text"},"verbosity":"low"},
            "input":[
                {"type":"message","role":"user","content":"first turn"},
                {"type":"configuration_update","id":"cu-1","reasoning":{"effort":"high"},"text":{"format":format}},
                {"type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"working"}]},
                {"type":"configuration_update","id":"cu-2","reasoning":{"effort":"max"}},
                {"type":"message","role":"user","content":"return the answer"}
            ]
        });
        let response = reqwest::Client::new()
            .post(format!("{gateway}/v1/responses"))
            .bearer_auth("gateway-key")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let stream = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{protocol:?}: {stream}");
        assert!(
            stream.contains("response.completed"),
            "{protocol:?}: {stream}"
        );

        let mut request = format!("{}/v1/responses", gateway.replacen("http://", "ws://", 1))
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
        let (mut socket, _) = connect_async(request).await.unwrap();
        let mut websocket_body = body.clone();
        websocket_body["type"] = json!("response.create");
        socket
            .send(WsMessage::Text(websocket_body.to_string().into()))
            .await
            .unwrap();
        let frames = websocket_response_frames(&mut socket).await;
        assert!(
            frames
                .iter()
                .any(|frame| frame.contains("\"type\":\"response.completed\"")),
            "{protocol:?}: {frames:?}"
        );
        socket.close(None).await.unwrap();

        let recorded = requests.lock().unwrap().clone();
        assert_eq!(recorded.len(), 2, "{protocol:?}: {recorded:?}");
        for sent in recorded.iter() {
            assert!(!sent.to_string().contains("configuration_update"), "{sent}");
            match protocol {
                ProviderProtocol::AnthropicMessages => {
                    assert_eq!(sent["output_config"]["effort"], "max");
                    assert_eq!(sent["output_config"]["format"]["schema"], format["schema"]);
                }
                ProviderProtocol::OpenAiChat => {
                    assert_eq!(sent["reasoning_effort"], "max");
                    assert_eq!(
                        sent["response_format"]["json_schema"]["schema"],
                        format["schema"]
                    );
                }
                ProviderProtocol::GoogleGemini => {
                    assert_eq!(
                        sent["generationConfig"]["thinkingConfig"]["thinkingBudget"],
                        16384
                    );
                    assert_eq!(
                        sent["generationConfig"]["responseMimeType"],
                        "application/json"
                    );
                    assert_eq!(
                        sent["generationConfig"]["responseJsonSchema"],
                        format["schema"]
                    );
                }
                _ => unreachable!(),
            }
        }
        for effort in [Value::Null, json!("none"), json!("ultra")] {
            let mut expected = body.clone();
            expected["reasoning"]["effort"] = effort.clone();
            expected["text"]["format"] = json!({"type":"text"});
            expected["input"] = json!([{"type":"message","role":"user","content":"continue"}]);
            let mut updated = expected.clone();
            updated["reasoning"]["effort"] = json!("low");
            updated["text"]["format"] = format.clone();
            updated["input"].as_array_mut().unwrap().insert(
                0,
                json!({"type":"configuration_update","reasoning":{"effort":effort},"text":{"format":{"type":"text"}}}),
            );
            for request in [&expected, &updated] {
                let response = reqwest::Client::new()
                    .post(format!("{gateway}/v1/responses"))
                    .bearer_auth("gateway-key")
                    .json(request)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let stream = response.text().await.unwrap();
                assert!(stream.contains("response.completed"), "{stream}");
            }
            let sent = requests.lock().unwrap();
            for field in [
                "thinking",
                "output_config",
                "reasoning_effort",
                "response_format",
                "generationConfig",
            ] {
                assert_eq!(
                    sent[sent.len() - 1][field],
                    sent[sent.len() - 2][field],
                    "{protocol:?} reset {field}"
                );
            }
        }
        let before_invalid = requests.lock().unwrap().len();
        for item in [
            json!({"type":"configuration_update"}),
            json!({"type":"configuration_update","reasoning":"high"}),
            json!({"type":"configuration_update","reasoning":{"effort":"invalid"}}),
            json!({"type":"configuration_update","text":{"format":{"type":"json_schema"}}}),
            json!({"type":"configuration_update","text":{"format":{"type":"unknown"}}}),
            json!({"type":"unknown_new_item"}),
        ] {
            let mut invalid = body.clone();
            invalid["input"] = json!([item, {"type":"message","role":"user","content":"continue"}]);
            let response = reqwest::Client::new()
                .post(format!("{gateway}/v1/responses"))
                .bearer_auth("gateway-key")
                .json(&invalid)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{protocol:?}");
        }
        assert_eq!(requests.lock().unwrap().len(), before_invalid);
    }
}

#[tokio::test]
async fn configuration_updates_preserve_native_responses_history_and_prefix() {
    for official in [false, true] {
        let (mut config, requests, _, _home) = mock_official_gateway_config(
            OfficialWebSocketBehavior::Persistent,
            Duration::from_secs(5),
        )
        .await;
        let model = if official {
            "gpt-5.6-sol"
        } else {
            let (url, recorded) = spawn_baidu_protocol_upstream().await;
            config.providers[0].base_url = url;
            configure_baidu_policy(&mut config);
            configure_custom_headers_from_env(&mut config);
            config.providers[0].model_source = ProviderModelSource::BaiduOneApi;
            configure_model_protocol(
                &mut config,
                "gpt-5.6-sol",
                ProviderProtocol::OpenAiResponses,
                "/v1/responses",
            );
            // The same original item list must reach a native custom provider.
            let gateway = spawn_gateway_with_config(config).await;
            let body = json!({
                "model":"gpt-5.6-sol-custom","stream":true,
                "reasoning":{"effort":"low"},
                "input":[
                    {"type":"configuration_update","reasoning":{"effort":"max"}},
                    {"type":"message","role":"user","content":"hi"}
                ]
            });
            let response = reqwest::Client::new()
                .post(format!("{gateway}/v1/responses"))
                .bearer_auth("gateway-key")
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let stream = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{stream}");
            assert!(stream.contains("response.completed"), "{stream}");
            let sent = recorded.lock().unwrap();
            assert_eq!(sent[0]["body"]["input"], body["input"], "{sent:?}");
            assert_eq!(sent[0]["body"]["reasoning"], body["reasoning"]);
            continue;
        };
        let gateway = spawn_gateway_with_config(config).await;
        let body = json!({
            "model":model,"stream":true,"reasoning":{"effort":"low"},
            "input":[
                {"type":"configuration_update","reasoning":{"effort":"max"}},
                {"type":"message","role":"user","content":"hi"}
            ]
        });
        let response = reqwest::Client::new()
            .post(format!("{gateway}/v1/responses"))
            .bearer_auth("gateway-key")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let stream = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{stream}");
        assert!(stream.contains("response.completed"), "{stream}");
        let sent = requests.lock().unwrap();
        assert_eq!(sent[0]["input"], body["input"]);
        assert_eq!(sent[0]["reasoning"], body["reasoning"]);
    }
}

#[tokio::test]
async fn routes_baidu_models_with_per_model_reasoning_capabilities() {
    let (upstream_url, requests) = spawn_baidu_protocol_upstream().await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    configure_custom_headers_from_env(&mut config);
    config.providers[0].model_source = ProviderModelSource::BaiduOneApi;
    configure_model_protocol(
        &mut config,
        "gpt-5.6-sol",
        ProviderProtocol::OpenAiResponses,
        "/v1/responses",
    );
    config.thinking_mode = ThinkingMode::Auto;
    config.providers[0]
        .cached_models
        .iter_mut()
        .find(|model| model.id == "DeepSeek-V4-Flash")
        .unwrap()
        .supports_thinking = Some(true);
    config.providers[0].cached_models.push(ProviderModel {
        id: "Claude Opus 4.6".to_owned(),
        supports_thinking: Some(true),
        ..ProviderModel::default()
    });
    config.providers[0]
        .selected_models
        .push("Claude Opus 4.6".to_owned());
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();

    let mut gpt_request = responses_request();
    gpt_request["model"] = json!("gpt-5.6-sol-custom");
    gpt_request["reasoning"] = json!({"effort": "ultra"});
    let gpt_response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&gpt_request)
        .send()
        .await
        .unwrap();
    assert_eq!(gpt_response.status(), StatusCode::OK);
    assert!(
        gpt_response
            .text()
            .await
            .unwrap()
            .contains("response.completed")
    );

    let mut opus_request = responses_request();
    opus_request["model"] = json!("Claude Opus 4.6-custom");
    opus_request["reasoning"] = json!({"effort": "ultra"});
    let opus_response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&opus_request)
        .send()
        .await
        .unwrap();
    assert_eq!(opus_response.status(), StatusCode::OK);
    assert!(
        opus_response
            .text()
            .await
            .unwrap()
            .contains("response.completed")
    );

    let mut deepseek_request = responses_request();
    deepseek_request["reasoning"] = json!({"effort": "ultra"});
    let deepseek_response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&deepseek_request)
        .send()
        .await
        .unwrap();
    assert_eq!(deepseek_response.status(), StatusCode::OK);
    assert!(
        deepseek_response
            .text()
            .await
            .unwrap()
            .contains("response.completed")
    );

    let requests = requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    let responses_request = requests
        .iter()
        .find(|request| request["path"] == "/v1/responses")
        .unwrap();
    assert_eq!(responses_request["body"]["model"], "gpt-5.6-sol");
    assert_eq!(responses_request["body"]["reasoning"]["effort"], "max");
    assert!(responses_request["anthropic_version"].is_null());
    let messages_request = requests
        .iter()
        .find(|request| {
            request["path"] == "/v1/messages" && request["body"]["model"] == "Claude Opus 4.6"
        })
        .unwrap();
    assert_eq!(messages_request["body"]["model"], "Claude Opus 4.6");
    assert_eq!(messages_request["body"]["thinking"]["type"], "adaptive");
    assert_eq!(messages_request["body"]["output_config"]["effort"], "max");
    assert_eq!(messages_request["anthropic_version"], "2023-06-01");

    let deepseek_request = requests
        .iter()
        .find(|request| {
            request["path"] == "/v1/messages" && request["body"]["model"] == "DeepSeek-V4-Flash"
        })
        .unwrap();
    assert_eq!(deepseek_request["body"]["thinking"]["type"], "adaptive");
    assert_eq!(deepseek_request["body"]["output_config"]["effort"], "max");
}

#[tokio::test]
async fn forked_baidu_subagent_keeps_parent_affinity_and_replays_undecryptable_history() {
    let (upstream_url, requests) = spawn_baidu_protocol_upstream().await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    configure_custom_headers_from_env(&mut config);
    config.providers[0].model_source = ProviderModelSource::BaiduOneApi;
    configure_model_protocol(
        &mut config,
        "gpt-5.6-sol",
        ProviderProtocol::OpenAiResponses,
        "/v1/responses",
    );
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();

    let mut parent = responses_request();
    parent["model"] = json!("gpt-5.6-sol-custom");
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("thread-id", "parent-thread")
        .json(&parent)
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

    // The forked child replays the parent's reasoning, which this upstream
    // account cannot decrypt.
    let mut child = parent.clone();
    child["input"].as_array_mut().unwrap().extend([
        json!({"type":"reasoning","id":"rs_parent","summary":[],"encrypted_content":"gAAAAA-foreign-account"}),
        json!({"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"call_1","output":[{"type":"encrypted_content","encrypted_content":"gAAAAA-foreign-account"}]}),
    ]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("thread-id", "child-thread")
        .header("x-codex-parent-thread-id", "parent-thread")
        .header("x-openai-subagent", "collab_spawn")
        .json(&child)
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

    let requests = requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    let parent_key = requests[0]["x_hash_key"].as_str().unwrap();
    assert!(
        requests
            .iter()
            .all(|request| request["x_hash_key"] == parent_key)
    );
    let retried = requests[2]["body"]["input"].as_array().unwrap();
    assert!(retried.iter().all(|item| item["type"] != "reasoning"));
    let output = retried
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    assert!(output["output"].is_string());
}

#[tokio::test]
async fn rewrites_named_unpaired_function_output_for_responses_provider() {
    let (upstream_url, requests) = spawn_baidu_protocol_upstream().await;
    let mut config = test_config(upstream_url);
    configure_custom_headers_from_env(&mut config);
    config.providers[0].protocol = ProviderProtocol::OpenAiResponses;
    config.providers[0].api_path = "/v1/responses".to_owned();
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("gpt-5.6-sol-custom");
    request["input"] = json!([{
        "type": "function_call_output",
        "name": "send_message_to_thread",
        "namespace": "codex_app",
        "output": "Start a delegated turn."
    }]);

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    let status = response.status();
    let response_body = response.text().await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected response: {response_body}"
    );
    {
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0]["body"]["input"],
            json!([{
                "type": "message",
                "role": "user",
                "content": "Start a delegated turn."
            }])
        );
    }

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
                "model": "gpt-5.6-sol-custom",
                "input": [{
                    "type": "function_call_output",
                    "name": "send_message_to_thread",
                    "namespace": "codex_app",
                    "output": "Resume a delegated turn."
                }]
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(frames.contains("\"type\":\"response.completed\""));
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(
        captured[1]["body"]["input"],
        json!([{
            "type": "message",
            "role": "user",
            "content": "Resume a delegated turn."
        }])
    );
}

#[tokio::test]
async fn normalizes_foreign_reasoning_for_deepseek_responses_provider() {
    let (upstream_url, requests) = spawn_deepseek_reasoning_upstream().await;
    let mut config = test_config(upstream_url);
    config.providers[0].preset_id = Some("deepseek".to_owned());
    config.providers[0].protocol = ProviderProtocol::OpenAiResponses;
    config.providers[0].api_path = "/v1/responses".to_owned();
    let gateway_url = spawn_gateway_with_config(config).await;
    let foreign_reasoning = format!(
        "codex-mixin:anthropic-thinking:v1:{}",
        json!({
            "type": "thinking",
            "thinking": "I should inspect the failing build.",
            "signature": "signed-state"
        })
    );
    let mut request = responses_request();
    request["model"] = json!("DeepSeek-V4-Flash-custom");
    request["input"] = json!([
        {
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "summary"}],
            "encrypted_content": foreign_reasoning,
            "content": []
        },
        {
            "type": "function_call",
            "call_id": "call-status",
            "name": "get_status",
            "arguments": "{\"job_id\":42}"
        },
        {
            "type": "function_call_output",
            "call_id": "call-status",
            "output": "success"
        }
    ]);

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["input"][0],
        json!({
            "type": "reasoning",
            "content": [{
                "type": "reasoning_text",
                "text": "I should inspect the failing build."
            }]
        })
    );
}

#[tokio::test]
async fn rewrites_named_unpaired_function_output_before_anthropic_conversion() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let mut request = responses_request();
    request["input"] = json!([{
        "type": "function_call_output",
        "name": "send_message_to_thread",
        "namespace": "codex_app",
        "output": "Continue from another thread."
    }]);

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0]["messages"],
        json!([{
            "role": "user",
            "content": [{"type": "text", "text": "Continue from another thread."}]
        }])
    );
}

#[tokio::test]
async fn converts_anthropic_messages_for_openai_responses_models() {
    let (upstream_url, requests) = spawn_baidu_protocol_upstream().await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    configure_custom_headers_from_env(&mut config);
    config.providers[0].model_source = ProviderModelSource::BaiduOneApi;
    configure_model_protocol(
        &mut config,
        "gpt-5.6-sol",
        ProviderProtocol::OpenAiResponses,
        "/v1/responses",
    );
    let gateway_url = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "gpt-5.6-sol-custom",
            "max_tokens": 1024,
            "stream": true,
            "system": "You are Claude Code.",
            "messages": [{"role": "user", "content": "say hi"}],
            "tools": [{
                "name": "exec_command",
                "description": "run shell",
                "input_schema": {"type": "object", "properties": {"cmd": {"type": "string"}}}
            }]
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();

    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");
    let mut event_buffer = body.into_bytes();
    let events = drain_events(&mut event_buffer);
    let payloads = events
        .iter()
        .map(|event| serde_json::from_str::<Value>(&event.data).unwrap())
        .collect::<Vec<_>>();
    let tool_starts = payloads
        .iter()
        .filter(|payload| {
            payload["type"] == "content_block_start"
                && payload["content_block"]["type"] == "tool_use"
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_starts.len(), 1);
    assert_eq!(tool_starts[0]["content_block"]["id"], "call_claude");
    assert_eq!(tool_starts[0]["content_block"]["name"], "exec_command");
    let arguments = payloads
        .iter()
        .filter(|payload| payload["delta"]["type"] == "input_json_delta")
        .filter_map(|payload| payload["delta"]["partial_json"].as_str())
        .collect::<String>();
    assert_eq!(
        serde_json::from_str::<Value>(&arguments).unwrap(),
        json!({"cmd":"pwd"})
    );
    assert!(payloads.iter().any(|payload| {
        payload["type"] == "message_delta" && payload["delta"]["stop_reason"] == "tool_use"
    }));
    assert!(
        payloads
            .iter()
            .any(|payload| payload["type"] == "message_stop")
    );

    let continuation = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "gpt-5.6-sol-custom",
            "max_tokens": 1024,
            "stream": true,
            "system": "You are Claude Code.",
            "messages": [
                {"role":"assistant","content":[{
                    "type":"tool_use",
                    "id":"call_claude",
                    "name":"exec_command",
                    "input":{"cmd":"pwd"}
                }]},
                {"role":"user","content":[{
                    "type":"tool_result",
                    "tool_use_id":"call_claude",
                    "content":"/workspace"
                }]}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(continuation.status(), StatusCode::OK);

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["path"], "/v1/responses");
    assert_eq!(requests[0]["body"]["model"], "gpt-5.6-sol");
    assert_eq!(requests[0]["body"]["instructions"], "You are Claude Code.");
    assert_eq!(requests[0]["body"]["input"][0]["role"], "user");
    assert_eq!(requests[0]["body"]["tools"][0]["name"], "exec_command");
    assert_eq!(requests[1]["body"]["input"][0]["type"], "function_call");
    assert_eq!(
        requests[1]["body"]["input"][1]["type"],
        "function_call_output"
    );
    assert_eq!(requests[1]["body"]["input"][1]["output"], "/workspace");
}

#[tokio::test]
async fn converts_anthropic_messages_for_official_models() {
    let official_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = official_requests.clone();
    let official_url = spawn_router(Router::new().route(
        "/v1/responses",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let captured = captured.clone();
            async move {
                assert_eq!(
                    headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()),
                    Some("Bearer codex-oauth-token")
                );
                if body["store"] != false {
                    return (StatusCode::BAD_REQUEST, Json(json!({"detail":"Store must be set to false"}))).into_response();
                }
                captured.lock().unwrap().push(body.clone());
                let item = json!({
                    "id":"msg_official",
                    "type":"message",
                    "status":"completed",
                    "role":"assistant",
                    "content":[{"type":"output_text","text":"hello official","annotations":[]}]
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(format!(
                        "event: response.output_item.done\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
                        json!({"type":"response.output_item.done","output_index":0,"item":item}),
                        json!({
                            "type":"response.completed",
                            "response":{
                                "id":"resp_official",
                                "object":"response",
                                "status":"completed",
                                "model":body["model"],
                                "output":[],
                                "usage":{"input_tokens":4,"output_tokens":2,"total_tokens":6}
                            }
                        })
                    )))
                    .unwrap()
            }
        }),
    ))
    .await;
    let (custom_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(custom_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    config.gateway_client_keys.claude_desktop = Some("desktop-key".to_owned());
    config.official_selected_models = Some(vec!["gpt-5.5".to_owned()]);
    let gateway_url = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model":"gpt-5.5",
            "max_tokens":1024,
            "stream":true,
            "system":"You are Claude Code.",
            "messages":[{"role":"user","content":"say hi"}]
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();

    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");
    assert!(body.contains("hello official"), "{body}");
    for stream in [true, false] {
        let response = reqwest::Client::new()
            .post(format!("{gateway_url}/claude-desktop/v1/messages"))
            .bearer_auth("desktop-key")
            .json(&json!({
                "model": codex_mixin::clients::claude_desktop::route_id("gpt-5.5"),
                "max_tokens":1024,"stream":stream,
                "messages":[{"role":"user","content":"say hi"}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        if stream {
            assert!(
                response
                    .text()
                    .await
                    .unwrap()
                    .contains("event: message_stop")
            );
        } else {
            let message: Value = response.json().await.unwrap();
            assert_eq!(message["content"][0]["text"], "hello official");
        }
    }
    let alias = codex_mixin::clients::claude_desktop::route_id("gpt-5.5");
    let models: Value = reqwest::Client::new()
        .get(format!("{gateway_url}/claude-desktop/v1/models"))
        .bearer_auth("desktop-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        models["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["id"] == alias)
    );
    let detail = reqwest::Client::new()
        .get(format!("{gateway_url}/claude-desktop/v1/models/{alias}"))
        .bearer_auth("desktop-key")
        .send()
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(detail.json::<Value>().await.unwrap()["id"], alias);
    let requests = official_requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|body| body["store"] == false));
    assert_eq!(requests[0]["model"], "gpt-5.5");
    assert_eq!(requests[0]["instructions"], "You are Claude Code.");
}

#[tokio::test]
async fn claude_desktop_hides_official_models_without_oauth_tokens() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    // API-key mode: auth.json exists, but nothing can serve official models.
    std::fs::write(
        &config.codex_auth_path,
        r#"{"OPENAI_API_KEY":"sk-test","auth_mode":"apikey"}"#,
    )
    .unwrap();
    config.gateway_client_keys.claude_desktop = Some("desktop-key".to_owned());
    config.official_selected_models = Some(vec!["gpt-5.6-terra".to_owned()]);
    let gateway_url = spawn_gateway_with_config(config).await;

    let catalog: Value = reqwest::Client::new()
        .get(format!("{gateway_url}/claude-desktop/v1/models"))
        .bearer_auth("desktop-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids = catalog["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let official_route = codex_mixin::clients::claude_desktop::route_id("gpt-5.6-terra");
    assert!(
        !ids.contains(&official_route),
        "gateway advertised {official_route} without OAuth tokens: {ids:?}"
    );
    assert_eq!(
        ids.len(),
        10,
        "only provider models should stay listed: {ids:?}"
    );
    assert!(ids.iter().all(|id| id.starts_with("claude-sonnet-mixin-")));
}

#[tokio::test]
async fn converts_anthropic_messages_for_openai_chat_models() {
    let (upstream_url, requests) = spawn_mock_openai_chat().await;
    let mut config = test_config(upstream_url);
    configure_openai_chat(&mut config, "/chat/completions");
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let request = json!({
        "model": "gpt-5.6-sol-custom",
        "max_tokens": 1024,
        "stream": true,
        "system": "You are Claude Code.",
        "messages": [{"role": "user", "content": "say hi"}]
    });

    let streamed = client
        .post(format!("{gateway_url}/v1/messages"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(streamed.status(), StatusCode::OK);
    let streamed = streamed.text().await.unwrap();
    assert_eq!(streamed.matches("event: content_block_start").count(), 1);
    assert!(streamed.contains("hello"), "{streamed}");
    assert!(streamed.contains(" openai"), "{streamed}");
    assert!(streamed.contains("event: message_stop"), "{streamed}");

    let mut request = request;
    request["stream"] = Value::Bool(false);
    let buffered = client
        .post(format!("{gateway_url}/v1/messages"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(buffered.status(), StatusCode::OK);
    let buffered: Value = buffered.json().await.unwrap();
    assert_eq!(buffered["type"], "message");
    assert_eq!(buffered["content"][0]["text"], "hello openai");
    assert_eq!(buffered["stop_reason"], "end_turn");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["model"], "gpt-5.6-sol");
    assert_eq!(requests[0]["messages"][0]["role"], "system");
    assert_eq!(requests[0]["messages"][1]["role"], "user");
}

#[tokio::test]
async fn converts_openai_chat_tool_calls_to_anthropic_messages() {
    let (upstream_url, requests) = spawn_mock_openai_image_chat().await;
    let mut config = test_config(upstream_url);
    configure_openai_chat(&mut config, "/chat/completions");
    let gateway_url = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "gpt-5.6-sol-custom",
            "max_tokens": 1024,
            "stream": true,
            "messages": [{"role": "user", "content": "draw"}],
            "tools": [{
                "name": "image_gen__imagegen",
                "description": "draw image",
                "input_schema": {"type": "object", "properties": {"prompt": {"type": "string"}}}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert_eq!(body.matches("\"type\":\"tool_use\"").count(), 1, "{body}");
    assert!(body.contains("image_gen__imagegen"), "{body}");
    assert!(body.contains("input_json_delta"), "{body}");
    assert!(body.contains("\"stop_reason\":\"tool_use\""), "{body}");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["tools"][0]["function"]["name"],
        "image_gen__imagegen"
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
            "model": "Claude Sonnet 5-custom",
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
async fn ignores_compaction_trigger_for_custom_responses() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "Claude Sonnet 5-custom",
            "stream": true,
            "input": [
                {"type":"compaction_trigger","id":"ct_1"},
                {"type":"message","role":"user","content":[
                    {"type":"input_text","text":"continue after compaction"}
                ]}
            ]
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "unexpected response: {body}");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["messages"].as_array().unwrap().len(), 1);
    assert_eq!(requests[0]["messages"][0]["role"], "user");
}

#[tokio::test]
async fn locally_handles_v2_compaction_trigger_for_custom_responses() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "Claude Sonnet 5-custom",
            "stream": true,
            "input": [
                {"type":"message","role":"user","content":[
                    {"type":"input_text","text":"fix the parser"}
                ]},
                {"type":"message","role":"assistant","content":[
                    {"type":"output_text","text":"The parser fix is complete."}
                ]},
                {"type":"compaction_trigger"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut encoded = response.bytes().await.unwrap().to_vec();
    let completed_items = drain_events(&mut encoded)
        .into_iter()
        .filter(|event| event.event.as_deref() == Some("response.output_item.done"))
        .map(|event| serde_json::from_str::<Value>(&event.data).unwrap()["item"].clone())
        .collect::<Vec<_>>();
    assert_eq!(completed_items.len(), 1);
    assert_eq!(completed_items[0]["type"], "compaction");
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn locally_compacts_custom_provider_and_omits_tool_payloads() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/v1/responses/compact"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "Claude Sonnet 5-custom",
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"fix the parser"}]},
                {"type":"function_call","call_id":"call-1","name":"exec_command","arguments":"{\"cmd\":\"cargo test\"}"},
                {"type":"function_call_output","call_id":"call-1","output":"all tests passed"},
                {"type":"reasoning","encrypted_content":"sensitive reasoning"},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"The parser fix is complete."}]}
            ]
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body_text = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_str(&body_text).unwrap();
    assert_eq!(body["status"], "completed");
    assert_eq!(body["output"][0]["type"], "compaction");
    assert!(
        body["output"][0]["encrypted_content"]
            .as_str()
            .unwrap()
            .starts_with("codex-mixin:compaction:v1:")
    );
    assert!(requests.lock().unwrap().is_empty());

    let token = body["output"][0]["encrypted_content"].clone();
    let replay = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&json!({
            "model": "Claude Sonnet 5-custom",
            "stream": true,
            "input": [
                {"type":"compaction","encrypted_content":token},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let request = requests.lock().unwrap()[0].clone();
    let compacted = request["system"][0]["text"].as_str().unwrap();
    assert!(compacted.contains("fix the parser"));
    assert!(compacted.contains("The parser fix is complete."));
    assert!(!compacted.contains("cargo test"));
    assert!(!compacted.contains("all tests passed"));
    assert!(!compacted.contains("sensitive reasoning"));
}

#[tokio::test]
async fn compacts_openai_chat_provider_into_mixin_token() {
    let (upstream_url, requests) = spawn_mock_openai_chat().await;
    let mut config = test_config(upstream_url);
    configure_openai_chat(&mut config, "/chat/completions");
    let gateway_url = spawn_gateway_with_config(config).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses/compact"))
        .bearer_auth("gateway-key")
        .json(&json!({"model": "gpt-5.6-sol-custom", "input": "continue"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["output"][0]["type"], "compaction");
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn compacts_baidu_gpt_through_responses_provider() {
    let (upstream_url, requests) = spawn_baidu_protocol_upstream().await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    configure_custom_headers_from_env(&mut config);
    config.providers[0].model_source = ProviderModelSource::BaiduOneApi;
    configure_model_protocol(
        &mut config,
        "gpt-5.6-sol",
        ProviderProtocol::OpenAiResponses,
        "/v1/responses",
    );
    let gateway_url = spawn_gateway_with_config(config).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses/compact"))
        .bearer_auth("gateway-key")
        .json(&json!({"model": "gpt-5.6-sol-custom", "input": "continue"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["output"][0]["type"], "compaction");
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn forwards_official_compact_request_and_opaque_token() {
    let (official_url, requests, auth_headers, account_headers, _, forwarded_headers) =
        spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let codex_home = tempfile::tempdir().unwrap();
    let auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let mut config = test_config("http://127.0.0.1:1".to_owned());
    config.providers[0].cached_models.clear();
    config.providers[0].selected_models.clear();
    config.official_responses_url = format!("{official_url}/v1/responses");
    config.codex_auth_path = auth_path;
    let gateway_url = spawn_gateway_with_config(config).await;
    let request_body = json!({
        "model": "gpt-5.5",
        "input": "summarize",
        "stream": false,
        "previous_response_id": "resp_123",
        "unknown_future_field": {"preserve": true}
    });
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses/compact"))
        .bearer_auth("gateway-key")
        .header("x-request-id", "request-123")
        .json(&request_body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["output"][0]["encrypted_content"],
        "official-opaque-token"
    );
    assert_eq!(requests.lock().unwrap().as_slice(), &[request_body]);
    assert_eq!(
        auth_headers.lock().unwrap().as_slice(),
        &[Some("Bearer codex-oauth-token".to_owned())]
    );
    assert_eq!(
        account_headers.lock().unwrap().as_slice(),
        &[Some("account-1".to_owned())]
    );
    let forwarded = forwarded_headers.lock().unwrap();
    assert_eq!(
        forwarded[0]
            .get("x-request-id")
            .and_then(|value| value.to_str().ok()),
        Some("request-123")
    );
}

#[tokio::test]
async fn maps_openai_chat_stream_to_responses_sse() {
    let (upstream_url, requests) = spawn_mock_openai_chat().await;
    let mut config = test_config(upstream_url);
    configure_openai_chat(&mut config, "/chat/completions");
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["reasoning"] = json!({"effort": "high"});
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
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
    assert_eq!(upstream_request["reasoning_effort"], "high");
}

#[tokio::test]
async fn maps_oneapi_affinity_for_openai_chat() {
    let (upstream_url, requests) = spawn_mock_openai_chat().await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    configure_openai_chat(&mut config, "/chat/completions");
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
    configure_baidu_policy(&mut config);
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["model"] = json!("Fable 5-custom");

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
    config.providers[0].image_generation_path = Some("/v1/images/generations".to_owned());
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
    configure_openai_chat(&mut config, "/chat/completions");
    config.providers[0].image_generation_path = Some("/images/generations".to_owned());
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
async fn auxiliary_provider_receives_unmarked_image_generation_requests() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ImageTool).await;
    let mut config = test_config(upstream_url);
    config.providers[0].auxiliary_model_upstream = true;
    config.providers[0].image_generation_path = Some("/v1/images/generations".to_owned());
    let gateway_url = spawn_gateway_with_config(config).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .bearer_auth("gateway-key")
        .json(&json!({"prompt":"draw a green square","model":"gpt-image-2"}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["prompt"], "draw a green square");
    assert_eq!(requests[0]["model"], "gpt-image-2");
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
    config.providers[0].image_generation_path = Some("/v1/images/generations".to_owned());
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
    config.providers[0].image_generation_path = Some("/v1/images/generations".to_owned());
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
