use super::*;

#[tokio::test]
async fn maps_custom_websocket_to_responses_frames() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
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
    assert_eq!(
        requests[0]["metadata"]["user_id"],
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, b"websocket-session").to_string()
    );
}

#[tokio::test]
async fn custom_websocket_close_before_first_progress_releases_session_for_resume() {
    let upstream_state = HangingWsUpstreamState {
        requests: Arc::new(AtomicUsize::new(0)),
        started: Arc::new(tokio::sync::Notify::new()),
        cancelled: Arc::new(tokio::sync::Notify::new()),
    };
    let upstream = spawn_router(
        Router::new()
            .route("/v1/messages", post(hanging_ws_messages))
            .with_state(upstream_state.clone()),
    )
    .await;
    let mut config = test_config(upstream);
    configure_baidu_policy(&mut config);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    let websocket_url = gateway.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    request
        .headers_mut()
        .insert("x-session-id", "cancel-before-progress".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = responses_request();
    body.as_object_mut().unwrap().remove("stream");
    body["type"] = json!("response.create");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), upstream_state.started.notified())
        .await
        .expect("the custom upstream did not receive the first request");
    socket.send(WsMessage::Close(None)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), upstream_state.cancelled.notified())
        .await
        .expect("closing the client websocket did not cancel its pending upstream stream");

    let mut resume_request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    resume_request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    resume_request
        .headers_mut()
        .insert("x-session-id", "cancel-before-progress".parse().unwrap());
    let (mut resumed_socket, _) = connect_async(resume_request).await.unwrap();
    let mut resume_body = responses_request();
    resume_body.as_object_mut().unwrap().remove("stream");
    resume_body["type"] = json!("response.create");
    resumed_socket
        .send(WsMessage::Text(resume_body.to_string().into()))
        .await
        .unwrap();
    let frames = tokio::time::timeout(
        WS_RESPONSE_TIMEOUT,
        websocket_response_frames(&mut resumed_socket),
    )
    .await
    .expect("the same-session websocket resume did not complete");
    assert!(
        frames
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );
    assert_eq!(upstream_state.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn locally_handles_v2_compaction_trigger_on_custom_websocket() {
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
    let compact = json!({
        "type": "response.create",
        "model": "Claude Sonnet 5-custom",
        "input": [
            {"type":"message","role":"user","content":[
                {"type":"input_text","text":"fix the parser"}
            ]},
            {"type":"message","role":"assistant","content":[
                {"type":"output_text","text":"The parser fix is complete."}
            ]},
            {"type":"compaction_trigger"}
        ]
    });
    socket
        .send(WsMessage::Text(compact.to_string().into()))
        .await
        .unwrap();

    let frames = websocket_response_frames(&mut socket).await;
    let completed_items = frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .filter(|event| event["type"] == "response.output_item.done")
        .map(|event| event["item"].clone())
        .collect::<Vec<_>>();
    assert_eq!(completed_items.len(), 1);
    assert_eq!(completed_items[0]["type"], "compaction");
    assert!(requests.lock().unwrap().is_empty());

    let previous_response_id = frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap()["response"]["id"]
        .clone();
    let follow_up = json!({
        "type": "response.create",
        "model": "Claude Sonnet 5-custom",
        "previous_response_id": previous_response_id,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type":"input_text","text":"continue"}]
        }],
        "tools": []
    });
    socket
        .send(WsMessage::Text(follow_up.to_string().into()))
        .await
        .unwrap();
    let follow_up_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(follow_up_frames.contains("\"type\":\"response.completed\""));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["messages"][0]["role"], "user");
    let compacted = requests[0]["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(compacted.contains("fix the parser"));
    assert!(compacted.contains("The parser fix is complete."));
}

#[tokio::test]
async fn strips_websocket_envelope_before_baidu_responses_request() {
    let (upstream_url, requests) = spawn_baidu_protocol_upstream().await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    configure_custom_headers_from_env(&mut config);
    config.providers[0].model_source = ProviderModelSource::BaiduOneApi;
    let gateway_url = spawn_gateway_with_config(config).await;
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
    first["model"] = json!("gpt-5.6-sol-custom");
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

    let follow_up = json!({
        "type": "response.create",
        "model": "gpt-5.6-sol-custom",
        "previous_response_id": completed["response"]["id"],
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type":"input_text","text":"second turn"}]
        }],
        "tools": []
    });
    socket
        .send(WsMessage::Text(follow_up.to_string().into()))
        .await
        .unwrap();
    let follow_up_frames = websocket_response_frames(&mut socket).await;
    assert!(
        follow_up_frames
            .iter()
            .any(|frame| frame.contains("\"type\":\"response.completed\""))
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| {
        request["body"].get("type").is_none()
            && request["body"].get("previous_response_id").is_none()
    }));
}

#[tokio::test]
async fn fusion_uses_same_pipeline_on_custom_websocket() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![fusion_profile()];
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = fusion_request();
    body.as_object_mut().unwrap().remove("stream");
    body["type"] = json!("response.create");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let frames = websocket_response_frames(&mut socket).await;
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("response.reasoning_summary_text.delta"))
    );
    let completed: Value = frames
        .iter()
        .filter_map(|frame| serde_json::from_str(frame).ok())
        .find(|event: &Value| event["type"] == "response.completed")
        .unwrap();
    assert_eq!(completed["response"]["model"], "auto/default");

    let follow_up = json!({
        "type":"response.create",
        "model":"auto/default",
        "previous_response_id":completed["response"]["id"],
        "instructions":"You are Codex.\n<collaboration_mode># Collaboration Mode: Default\nExecute the requested work.</collaboration_mode>",
        "input":[{
            "type":"message",
            "role":"user",
            "content":[{"type":"input_text","text":"follow up"}]
        }]
    });
    socket
        .send(WsMessage::Text(follow_up.to_string().into()))
        .await
        .unwrap();
    let follow_up_frames = websocket_response_frames(&mut socket).await;
    assert!(
        follow_up_frames
            .iter()
            .any(|frame| frame.contains("response.reasoning_summary_text.delta"))
    );
    assert_eq!(requests.lock().unwrap().len(), 8);
    assert_eq!(requests.lock().unwrap().last().unwrap()["model"], "final");
    let follow_up_completed: Value = follow_up_frames
        .iter()
        .filter_map(|frame| serde_json::from_str(frame).ok())
        .find(|event: &Value| event["type"] == "response.completed")
        .unwrap();

    let tool_continuation = json!({
        "type":"response.create",
        "model":"auto/default",
        "previous_response_id":follow_up_completed["response"]["id"],
        "input":[
            {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
            {"type":"function_call_output","call_id":"call_1","output":"/tmp"}
        ]
    });
    socket
        .send(WsMessage::Text(tool_continuation.to_string().into()))
        .await
        .unwrap();
    let continuation_frames = websocket_response_frames(&mut socket).await;
    assert!(
        continuation_frames
            .iter()
            .all(|frame| !frame.contains("response.reasoning_summary_text.delta"))
    );
    assert_eq!(requests.lock().unwrap().len(), 9);
    assert_eq!(requests.lock().unwrap().last().unwrap()["model"], "final");
}

#[tokio::test]
async fn retries_demoted_web_search_on_custom_websocket() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::WebSearchRetry).await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    config.enable_web_search_tool = true;
    let capability_dir = tempfile::tempdir().unwrap();
    let capabilities = WebSearchCapabilities::load(
        capability_dir.path().join("web-search-capabilities.json"),
        &config,
    )
    .unwrap();
    // The Baidu catalog declares hosted search; it is recorded without probes.
    let mut models = vec![ModelInfo {
        id: "Claude Sonnet 5-custom".to_owned(),
        supports_web_search: Some(true),
        ..ModelInfo::default()
    }];
    let registry = ProviderRegistry::new(config.providers.clone()).unwrap();
    let summary = capabilities
        .probe_models(&mut models, &config, &registry, true)
        .await
        .unwrap();
    assert_eq!(summary.attempted, 0);
    assert!(requests.lock().unwrap().is_empty(), "no probe requests");

    let state = AppState::with_web_search_capabilities(config, capabilities).unwrap();
    let gateway_url = spawn_router(router(state)).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    request
        .headers_mut()
        .insert("session-id", "web-search-session".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = responses_request();
    body.as_object_mut().unwrap().remove("stream");
    body["type"] = json!("response.create");
    body["model"] = json!("Claude Sonnet 5-custom");
    body["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"web_search","external_web_access":true}));
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(frames.contains("\"type\":\"web_search_call\""));
    assert!(frames.contains("\"type\":\"response.completed\""));
    assert!(!frames.contains("\"name\":\"web_search\",\"type\":\"function_call\""));
    let upstream_requests = requests.lock().unwrap();
    assert_eq!(upstream_requests.len(), 2);
    assert_eq!(
        upstream_requests[0]["metadata"]["user_id"],
        upstream_requests[0]["__x_hash_key"]
    );
    assert_eq!(
        upstream_requests[1]["metadata"]["user_id"],
        upstream_requests[1]["__x_hash_key"]
    );
    assert_ne!(
        upstream_requests[0]["metadata"]["user_id"],
        upstream_requests[1]["metadata"]["user_id"]
    );
    assert_eq!(upstream_requests[1]["tool_choice"]["name"], "web_search");
    assert!(
        upstream_requests
            .iter()
            .all(
                |request| uuid::Uuid::parse_str(request["__x_hash_key"].as_str().unwrap()).is_ok()
            )
    );
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
        "model": "DeepSeek-V4-Flash-custom",
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
async fn websocket_history_replay_only_rewrites_the_previous_tail() {
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
    first["input"].as_array_mut().unwrap().extend([
        json!({
            "type":"function_call",
            "call_id":"call_image",
            "name":"view_image",
            "arguments":"{}"
        }),
        json!({
            "type":"function_call_output",
            "call_id":"call_image",
            "output":[
                {"type":"input_text","text":"screenshot captured"},
                {"type":"input_image","image_url":png_data_url(16, 16)}
            ]
        }),
    ]);
    socket
        .send(WsMessage::Text(first.to_string().into()))
        .await
        .unwrap();
    let completed = websocket_response_frames(&mut socket)
        .await
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap();

    // Same model, instructions and tools, with only a new user turn appended.
    let mut second = responses_request();
    second.as_object_mut().unwrap().remove("stream");
    second["type"] = json!("response.create");
    second["previous_response_id"] = completed["response"]["id"].clone();
    second["input"] = json!([{
        "type":"message",
        "role":"user",
        "content":[{"type":"input_text","text":"second turn"}]
    }]);
    socket
        .send(WsMessage::Text(second.to_string().into()))
        .await
        .unwrap();
    let second_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(second_frames.contains("\"type\":\"response.completed\""));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["system"], requests[1]["system"]);
    assert_eq!(requests[0]["tools"], requests[1]["tools"]);
    let earlier = requests[0]["messages"].as_array().unwrap();
    let current = requests[1]["messages"].as_array().unwrap();
    assert!(
        current.len() > earlier.len(),
        "second turn did not append messages: {current:?}"
    );
    // Everything the provider cached before the previous tail is replayed
    // byte-for-byte.
    let stable = earlier.len() - 1;
    assert_eq!(&current[..stable], &earlier[..stable]);
    // The one rewritten message is what used to be the tail: the fresh tool
    // result the model has now seen, replayed as the marker. That is the entire
    // cost of not carrying screenshot bytes forever.
    assert_ne!(current[stable], earlier[stable]);
    assert!(requests[0].to_string().contains("\"type\":\"image\""));
    assert!(!requests[1].to_string().contains("\"type\":\"image\""));
    assert!(requests[1].to_string().contains(TOOL_IMAGE_PLACEHOLDER));
    assert!(!requests[0].to_string().contains(TOOL_IMAGE_PLACEHOLDER));
}

#[tokio::test]
async fn preserves_steered_user_input_after_custom_tool_call() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Tool).await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    let gateway_url = spawn_gateway_with_config(config).await;
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
    first["model"] = json!("gpt-5.6-sol-custom");
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

    let follow_up = json!({
        "type": "response.create",
        "model": "gpt-5.6-sol-custom",
        "previous_response_id": completed["response"]["id"],
        "input": [
            {
                "type": "function_call_output",
                "call_id": "toolu_1",
                "output": "command interrupted by new input"
            },
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "steer: inspect the logs instead"}]
            }
        ],
        "tools": first["tools"]
    });
    socket
        .send(WsMessage::Text(follow_up.to_string().into()))
        .await
        .unwrap();
    let follow_up_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(follow_up_frames.contains("\"type\":\"response.completed\""));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["model"], "gpt-5.6-sol");
    let messages = requests[1]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"][0]["type"], "tool_use");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(
        messages[2]["content"][1]["text"],
        "steer: inspect the logs instead"
    );
}

#[tokio::test]
async fn websocket_converts_historical_tool_schema_before_oneapi_request() {
    let (upstream_url, upstream_requests) = spawn_mock_openai_chat().await;
    let mut config = test_config(upstream_url);
    configure_openai_chat(&mut config, "/chat/completions");
    let gateway_url = spawn_gateway_with_config(config).await;
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
    body["tools"] = json!([]);
    body["input"].as_array_mut().unwrap().push(json!({
        "type":"tool_search_output",
        "call_id":"search_1",
        "status":"completed",
        "execution":"client",
        "tools":[{"type":"function","name":"lookup","parameters":{
            "anyOf":[{"type":"object","properties":{"id":{"type":"string"}}}]
        }}]
    }));
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();
    assert!(
        websocket_response_frames(&mut socket)
            .await
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );

    let requests = upstream_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["tools"][0]["function"]["name"], "lookup");
    assert_eq!(
        requests[0]["tools"][0]["function"]["parameters"]["type"],
        "object"
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
        official_forwarded_headers,
    ) = spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
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
    request
        .headers_mut()
        .insert("originator", "codex_cli_rs".parse().unwrap());
    request
        .headers_mut()
        .insert("x-codex-originator", "legacy-codex".parse().unwrap());
    request
        .headers_mut()
        .insert("x-openai-subagent", "review".parse().unwrap());
    request
        .headers_mut()
        .insert("x-openai-memgen-request", "true".parse().unwrap());
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
    assert!(
        official_forwarded_headers
            .lock()
            .unwrap()
            .iter()
            .all(|headers| {
                headers
                    .get("originator")
                    .and_then(|value| value.to_str().ok())
                    == Some("codex_cli_rs")
                    && headers
                        .get("x-codex-originator")
                        .and_then(|value| value.to_str().ok())
                        == Some("legacy-codex")
                    && headers
                        .get("x-openai-subagent")
                        .and_then(|value| value.to_str().ok())
                        == Some("review")
                    && headers
                        .get("x-openai-memgen-request")
                        .and_then(|value| value.to_str().ok())
                        == Some("true")
            })
    );
}

#[tokio::test]
async fn routes_auto_review_to_official_websocket() {
    let (gateway_url, official_requests, official_websocket_connections, _codex_home) =
        spawn_gateway_with_mock_official(
            OfficialWebSocketBehavior::Persistent,
            Duration::from_secs(2),
        )
        .await;
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
    body["model"] = json!("codex-auto-review");
    body["max_output_tokens"] = json!(8192);

    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    assert!(
        websocket_response_frames(&mut socket)
            .await
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );
    let official_requests = official_requests.lock().unwrap();
    assert_eq!(official_requests.len(), 1);
    assert_eq!(official_requests[0]["model"], "codex-auto-review");
    assert_eq!(official_requests[0]["stream"], true);
    assert!(official_requests[0].get("max_output_tokens").is_none());
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn proxies_realtime_voice_websocket_to_official_backend() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let (
        official_url,
        official_requests,
        official_auth_headers,
        official_account_headers,
        official_websocket_connections,
        official_forwarded_headers,
    ) = spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
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
    let mut request =
        format!("{websocket_url}/v1/realtime?intent=quicksilver&model=gpt-realtime-1.5")
            .into_client_request()
            .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    request
        .headers_mut()
        .insert("originator", "codex-app".parse().unwrap());
    request
        .headers_mut()
        .insert("x-session-id", "voice-session".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();

    let session_update = json!({
        "type": "session.update",
        "session": {"voice": "verse"}
    });
    socket
        .send(WsMessage::Text(session_update.to_string().into()))
        .await
        .unwrap();
    let response = socket.next().await.unwrap().unwrap();
    let response: Value = match response {
        WsMessage::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text response, got {other:?}"),
    };

    assert_eq!(response["type"], "session.updated");
    assert_eq!(response["upstream_path"], "/v1/realtime");
    assert_eq!(
        response["upstream_query"],
        "intent=quicksilver&model=gpt-realtime-1.5"
    );
    assert_eq!(
        official_requests.lock().unwrap().as_slice(),
        &[session_update]
    );
    assert_eq!(
        official_auth_headers.lock().unwrap().as_slice(),
        &[Some("Bearer codex-oauth-token".to_owned())]
    );
    assert_eq!(
        official_account_headers.lock().unwrap().as_slice(),
        &[Some("account-1".to_owned())]
    );
    assert_eq!(
        official_forwarded_headers.lock().unwrap()[0]
            .get("x-session-id")
            .unwrap(),
        "voice-session"
    );
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_realtime_voice_when_official_oauth_routing_is_disabled() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.accept_codex_oauth = false;
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request =
        format!("{websocket_url}/v1/realtime?intent=quicksilver&model=gpt-realtime-1.5")
            .into_client_request()
            .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());

    let error = connect_async(request).await.unwrap_err();
    assert!(
        error.to_string().contains("400"),
        "expected HTTP 400, got {error}"
    );
}

#[tokio::test]
async fn maps_realtime_webrtc_call_to_official_backend_shape() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, official_auth_headers, official_account_headers, _, _) =
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
    let boundary = "codex-realtime-call-boundary";
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\nContent-Type: application/sdp\r\n\r\noffer-sdp\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\nContent-Type: application/json\r\n\r\n{{\"model\":\"gpt-realtime-1.5\",\"voice\":\"verse\"}}\r\n--{boundary}--\r\n"
    );

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/v1/realtime/calls?intent=quicksilver&architecture=avas"
        ))
        .bearer_auth("gateway-key")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(multipart)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "/v1/realtime/calls/call-456"
    );
    assert_eq!(response.text().await.unwrap(), "answer-sdp");
    assert_eq!(
        official_requests.lock().unwrap().as_slice(),
        &[json!({
            "body": {
                "sdp": "offer-sdp",
                "session": {"model": "gpt-realtime-1.5", "voice": "verse"}
            },
            "path": "/v1/realtime/calls",
            "query": "intent=quicksilver&architecture=avas"
        })]
    );
    assert_eq!(
        official_auth_headers.lock().unwrap().as_slice(),
        &[Some("Bearer codex-oauth-token".to_owned())]
    );
    assert_eq!(
        official_account_headers.lock().unwrap().as_slice(),
        &[Some("account-1".to_owned())]
    );
}

#[tokio::test]
async fn maps_official_live_call_to_official_backend_shape() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, _, _, _, _) =
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
    let boundary = "codex-realtime-call-boundary";
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\nContent-Type: application/sdp\r\n\r\noffer-sdp\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\nContent-Type: application/json\r\n\r\n{{\"model\":\"gpt-live-1-boulder-alpha\",\"voice\":\"verse\"}}\r\n--{boundary}--\r\n"
    );

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/live"))
        .bearer_auth("gateway-key")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(multipart)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        official_requests.lock().unwrap().as_slice(),
        &[json!({
            "body": {
                "sdp": "offer-sdp",
                "session": {"model": "gpt-live-1-boulder-alpha", "voice": "verse"}
            },
            "path": "/v1/realtime/calls",
            "query": "intent=quicksilver&architecture=avas"
        })]
    );
}

#[tokio::test]
async fn custom_only_routes_available_realtime_model_to_provider_websocket() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.providers[0].cached_models.push(ProviderModel {
        id: "gpt-realtime-1.5".to_owned(),
        ..ProviderModel::default()
    });
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"auth_mode":"bedrockApiKey","bedrock_api_key":{"api_key":"codex-mixin-local-test","region":"us-east-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request =
        format!("{websocket_url}/v1/realtime?intent=quicksilver&model=gpt-realtime-1.5")
            .into_client_request()
            .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let session_update = json!({"type": "session.update", "session": {"voice": "verse"}});
    socket
        .send(WsMessage::Text(session_update.to_string().into()))
        .await
        .unwrap();
    let response = socket.next().await.unwrap().unwrap();
    let response: Value = match response {
        WsMessage::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text response, got {other:?}"),
    };

    assert_eq!(response["authorization"], "Bearer upstream-key");
    assert_eq!(response["upstream_path"], "/v1/realtime");
    assert_eq!(
        response["upstream_query"],
        "intent=quicksilver&model=gpt-realtime-1.5"
    );
    assert_eq!(
        upstream_requests.lock().unwrap().as_slice(),
        &[session_update]
    );
}

#[tokio::test]
async fn custom_only_routes_available_realtime_model_to_provider_webrtc() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.providers[0].cached_models.push(ProviderModel {
        id: "gpt-realtime-1.5".to_owned(),
        ..ProviderModel::default()
    });
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"auth_mode":"bedrockApiKey","bedrock_api_key":{"api_key":"codex-mixin-local-test","region":"us-east-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let boundary = "codex-realtime-call-boundary";
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\nContent-Type: application/sdp\r\n\r\noffer-sdp\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\nContent-Type: application/json\r\n\r\n{{\"model\":\"gpt-realtime-1.5\",\"voice\":\"verse\"}}\r\n--{boundary}--\r\n"
    );

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/v1/realtime/calls?intent=quicksilver&architecture=avas"
        ))
        .bearer_auth("gateway-key")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(multipart)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        location,
        "/v1/realtime/calls/codex-mixin~custom~custom-call"
    );
    assert_eq!(response.text().await.unwrap(), "custom-answer-sdp");
    {
        let requests = upstream_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["authorization"], "Bearer upstream-key");
        assert_eq!(requests[0]["path"], "/v1/realtime/calls");
        assert_eq!(requests[0]["query"], "intent=quicksilver&architecture=avas");
        assert!(
            requests[0]["body"]
                .as_str()
                .unwrap()
                .contains(r#""model":"gpt-realtime-1.5""#)
        );
    }

    let call_id = location.rsplit('/').next().unwrap();
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/realtime?call_id={call_id}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let sideband_message = json!({"type": "session.update"});
    socket
        .send(WsMessage::Text(sideband_message.to_string().into()))
        .await
        .unwrap();
    let response = socket.next().await.unwrap().unwrap();
    let response: Value = match response {
        WsMessage::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text response, got {other:?}"),
    };
    assert_eq!(response["authorization"], "Bearer upstream-key");
    assert_eq!(response["upstream_path"], "/v1/realtime");
    assert_eq!(response["upstream_query"], "call_id=custom-call");
    assert_eq!(upstream_requests.lock().unwrap()[1], sideband_message);
}

#[tokio::test]
async fn custom_only_routes_available_live_model_and_sideband_to_provider() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.providers[0].cached_models.push(ProviderModel {
        id: "gpt-live-1-boulder-alpha".to_owned(),
        ..ProviderModel::default()
    });
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"auth_mode":"bedrockApiKey","bedrock_api_key":{"api_key":"codex-mixin-local-test","region":"us-east-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let boundary = "codex-live-call-boundary";
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\nContent-Type: application/sdp\r\n\r\noffer-sdp\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\nContent-Type: application/json\r\n\r\n{{\"model\":\"gpt-live-1-boulder-alpha\",\"voice\":\"verse\"}}\r\n--{boundary}--\r\n"
    );

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/live"))
        .bearer_auth("gateway-key")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(multipart)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(location, "/v1/live/codex-mixin~custom~custom-call");
    assert_eq!(response.text().await.unwrap(), "custom-answer-sdp");
    {
        let requests = upstream_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["path"], "/v1/live");
        assert!(
            requests[0]["body"]
                .as_str()
                .unwrap()
                .contains(r#""model":"gpt-live-1-boulder-alpha""#)
        );
    }

    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}{location}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    socket
        .send(WsMessage::Text(
            json!({"type": "session.update"}).to_string().into(),
        ))
        .await
        .unwrap();
    let response = socket.next().await.unwrap().unwrap();
    let response: Value = match response {
        WsMessage::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text response, got {other:?}"),
    };
    assert_eq!(response["authorization"], "Bearer upstream-key");
    assert_eq!(response["upstream_path"], "/v1/live/custom-call");
    assert_eq!(response["upstream_query"], "");
}

#[tokio::test]
async fn custom_only_rejects_unavailable_realtime_model() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"auth_mode":"bedrockApiKey","bedrock_api_key":{"api_key":"codex-mixin-local-test","region":"us-east-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/realtime?model=gpt-realtime-1.5")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());

    let error = connect_async(request).await.unwrap_err();
    assert!(
        error.to_string().contains("400"),
        "expected HTTP 400, got {error}"
    );
}

#[tokio::test]
async fn provider_qualified_realtime_model_overrides_available_official_oauth() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.providers[0].cached_models.push(ProviderModel {
        id: "gpt-realtime-1.5".to_owned(),
        ..ProviderModel::default()
    });
    config.providers[0]
        .selected_models
        .push("gpt-realtime-1.5".to_owned());
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/realtime?model=gpt-realtime-1.5-custom")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    socket
        .send(WsMessage::Text(
            json!({"type": "session.update"}).to_string().into(),
        ))
        .await
        .unwrap();
    let response = socket.next().await.unwrap().unwrap();
    let response: Value = match response {
        WsMessage::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text response, got {other:?}"),
    };

    assert_eq!(response["authorization"], "Bearer upstream-key");
    assert_eq!(response["upstream_query"], "model=gpt-realtime-1.5");
}

#[tokio::test]
async fn auxiliary_realtime_provider_overrides_available_official_oauth() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, _, _, _, official_websocket_connections, _) =
        spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    config.providers[0].auxiliary_model_upstream = true;
    config.providers[0].cached_models.push(ProviderModel {
        id: "gpt-realtime-1.5".to_owned(),
        ..ProviderModel::default()
    });
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/realtime?model=gpt-realtime-1.5")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let session_update = json!({"type": "session.update"});
    socket
        .send(WsMessage::Text(session_update.to_string().into()))
        .await
        .unwrap();
    let response = socket.next().await.unwrap().unwrap();
    let response: Value = match response {
        WsMessage::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected text response, got {other:?}"),
    };

    assert_eq!(response["authorization"], "Bearer upstream-key");
    assert_eq!(response["upstream_query"], "model=gpt-realtime-1.5");
    assert_eq!(
        upstream_requests.lock().unwrap().as_slice(),
        &[session_update]
    );
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn custom_only_uses_first_available_provider_for_shared_realtime_model() {
    let (primary_url, primary_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (secondary_url, secondary_requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(primary_url);
    config.providers[0].cached_models.push(ProviderModel {
        id: "gpt-realtime-1.5".to_owned(),
        ..ProviderModel::default()
    });
    let mut secondary = config.providers[0].clone();
    secondary.id = "secondary".to_owned();
    secondary.display_name = "Secondary".to_owned();
    secondary.base_url = secondary_url;
    config.providers.push(secondary);
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"auth_mode":"bedrockApiKey","bedrock_api_key":{"api_key":"codex-mixin-local-test","region":"us-east-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/realtime?model=gpt-realtime-1.5")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let session_update = json!({"type": "session.update"});
    socket
        .send(WsMessage::Text(session_update.to_string().into()))
        .await
        .unwrap();
    let _ = socket.next().await.unwrap().unwrap();

    assert_eq!(
        primary_requests.lock().unwrap().as_slice(),
        &[session_update]
    );
    assert!(secondary_requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn custom_only_prefers_provider_with_selected_shared_realtime_model() {
    let (primary_url, primary_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (secondary_url, secondary_requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(primary_url);
    config.providers[0].cached_models.push(ProviderModel {
        id: "gpt-realtime-1.5".to_owned(),
        ..ProviderModel::default()
    });
    let mut secondary = config.providers[0].clone();
    secondary.id = "secondary".to_owned();
    secondary.display_name = "Secondary".to_owned();
    secondary.base_url = secondary_url;
    secondary
        .selected_models
        .push("gpt-realtime-1.5".to_owned());
    config.providers.push(secondary);
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"auth_mode":"bedrockApiKey","bedrock_api_key":{"api_key":"codex-mixin-local-test","region":"us-east-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/realtime?model=gpt-realtime-1.5")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let session_update = json!({"type": "session.update"});
    socket
        .send(WsMessage::Text(session_update.to_string().into()))
        .await
        .unwrap();
    let _ = socket.next().await.unwrap().unwrap();

    assert!(primary_requests.lock().unwrap().is_empty());
    assert_eq!(
        secondary_requests.lock().unwrap().as_slice(),
        &[session_update]
    );
}

#[tokio::test]
async fn reconnects_official_upstream_without_resetting_client_websocket() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, _, _, official_websocket_connections, _) =
        spawn_mock_official(OfficialWebSocketBehavior::CloseAfterCompletedWithCustomTool).await;
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
    let first_frames = websocket_response_frames(&mut socket).await;
    let first_completed = first_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    let first_response_id = first_completed["response"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_output = first_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.output_item.done")
        .unwrap()["item"]
        .clone();
    assert_eq!(first_output["type"], "custom_tool_call");

    tokio::time::sleep(Duration::from_millis(25)).await;
    let incremental_input = json!({
        "type": "custom_tool_call_output",
        "call_id": first_output["call_id"],
        "output": "tool completed"
    });
    let mut follow_up = official.clone();
    follow_up["previous_response_id"] = json!(first_response_id);
    follow_up["input"] = json!([incremental_input.clone()]);
    socket
        .send(WsMessage::Text(follow_up.to_string().into()))
        .await
        .unwrap();
    let second_frames =
        tokio::time::timeout(WS_RESPONSE_TIMEOUT, websocket_response_frames(&mut socket))
            .await
            .expect("gateway did not finish the second official response");
    let second_joined = second_frames.join("\n");
    assert!(
        second_joined.contains("\"type\":\"response.completed\""),
        "unexpected second response frames: {second_joined}"
    );
    let second_completed = second_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    let second_response_id = second_completed["response"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second_output = second_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.output_item.done")
        .unwrap()["item"]
        .clone();

    tokio::time::sleep(Duration::from_millis(25)).await;
    let compaction_trigger = json!({"type":"compaction_trigger"});
    let mut compaction = official.clone();
    compaction["previous_response_id"] = json!(second_response_id);
    compaction["input"] = json!([compaction_trigger.clone()]);
    socket
        .send(WsMessage::Text(compaction.to_string().into()))
        .await
        .unwrap();
    let compaction_frames =
        tokio::time::timeout(WS_RESPONSE_TIMEOUT, websocket_response_frames(&mut socket))
            .await
            .expect("gateway did not finish compaction after reconnect");
    assert!(
        compaction_frames
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );

    let official_requests = official_requests.lock().unwrap();
    assert_eq!(official_requests.len(), 3);
    assert!(official_requests[1].get("previous_response_id").is_none());
    let mut expected_input = official["input"].as_array().unwrap().clone();
    expected_input.push(first_output);
    expected_input.push(incremental_input);
    assert_eq!(
        official_requests[1]["input"],
        Value::Array(expected_input.clone())
    );
    assert!(official_requests[2].get("previous_response_id").is_none());
    expected_input.push(second_output);
    expected_input.push(compaction_trigger);
    assert_eq!(official_requests[2]["input"], Value::Array(expected_input));
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn discards_official_connection_after_failed_and_incomplete_responses() {
    let (gateway_url, official_requests, official_websocket_connections, _codex_home) =
        spawn_gateway_with_mock_official(
            OfficialWebSocketBehavior::TerminalFailuresBeforeRecovery,
            Duration::from_secs(1),
        )
        .await;
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
    first["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(first.to_string().into()))
        .await
        .unwrap();
    let first_frames = websocket_response_frames(&mut socket).await;
    let first_response_id = first_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap()["response"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut failed = first.clone();
    failed["previous_response_id"] = json!(first_response_id);
    failed["input"] = json!([{
        "type": "message",
        "role": "user",
        "content": [{"type":"input_text","text":"trigger failed response"}]
    }]);
    socket
        .send(WsMessage::Text(failed.to_string().into()))
        .await
        .unwrap();
    let failed_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(failed_frames.contains("\"type\":\"response.failed\""));

    let mut incomplete = first.clone();
    incomplete["input"] = json!([{
        "type": "message",
        "role": "user",
        "content": [{"type":"input_text","text":"trigger incomplete response"}]
    }]);
    socket
        .send(WsMessage::Text(incomplete.to_string().into()))
        .await
        .unwrap();
    let incomplete_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(incomplete_frames.contains("\"type\":\"response.incomplete\""));

    let mut recovery = first.clone();
    recovery["input"] = json!([{
        "type": "message",
        "role": "user",
        "content": [{"type":"input_text","text":"recover on a fresh connection"}]
    }]);
    socket
        .send(WsMessage::Text(recovery.to_string().into()))
        .await
        .unwrap();
    let recovery_frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(recovery_frames.contains("\"type\":\"response.completed\""));

    let official_requests = official_requests.lock().unwrap();
    assert_eq!(official_requests.len(), 4);
    assert!(official_requests[2].get("previous_response_id").is_none());
    assert!(official_requests[3].get("previous_response_id").is_none());
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn maps_official_websocket_error_to_terminal_failure() {
    let (gateway_url, official_requests, official_websocket_connections, _codex_home) =
        spawn_gateway_with_mock_official(
            OfficialWebSocketBehavior::ConnectionLimitThenComplete,
            Duration::from_secs(1),
        )
        .await;
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
    body["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();
    let error_frames =
        tokio::time::timeout(WS_RESPONSE_TIMEOUT, websocket_response_frames(&mut socket))
            .await
            .expect("official websocket error was not mapped to response.failed")
            .join("\n");
    assert!(error_frames.contains("\"type\":\"response.failed\""));
    assert!(error_frames.contains("websocket_connection_limit_reached"));

    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();
    let recovery_frames =
        tokio::time::timeout(WS_RESPONSE_TIMEOUT, websocket_response_frames(&mut socket))
            .await
            .expect("gateway remained blocked after wrapped error")
            .join("\n");
    assert!(recovery_frames.contains("\"type\":\"response.completed\""));
    assert_eq!(official_requests.lock().unwrap().len(), 2);
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn times_out_silent_official_websocket() {
    let (gateway_url, _, official_websocket_connections, _codex_home) =
        spawn_gateway_with_mock_official(
            OfficialWebSocketBehavior::Silent,
            // Also bounds the mock's localhost connect, so leave room for a
            // busy runner instead of racing the TCP handshake.
            Duration::from_millis(250),
        )
        .await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = responses_request();
    body["type"] = json!("response.create");
    body["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let frames = tokio::time::timeout(WS_RESPONSE_TIMEOUT, websocket_response_frames(&mut socket))
        .await
        .expect("silent official websocket did not time out")
        .join("\n");
    assert!(frames.contains("\"type\":\"response.failed\""));
    assert!(frames.contains("idle timeout waiting for official websocket"));
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn times_out_official_websocket_handshake() {
    let (gateway_url, _, _, _codex_home) = spawn_gateway_with_mock_official(
        OfficialWebSocketBehavior::SlowHandshake,
        Duration::from_millis(250),
    )
    .await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = responses_request();
    body["type"] = json!("response.create");
    body["model"] = json!("gpt-5.5");
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();

    let frames = tokio::time::timeout(WS_RESPONSE_TIMEOUT, websocket_response_frames(&mut socket))
        .await
        .expect("official websocket handshake did not time out")
        .join("\n");
    assert!(frames.contains("\"type\":\"response.failed\""));
    assert!(frames.contains("official websocket connect timed out"));
}

#[tokio::test]
async fn preserves_official_prewarm_and_full_request_semantics() {
    let (gateway_url, official_requests, official_websocket_connections, _codex_home) =
        spawn_gateway_with_mock_official(
            OfficialWebSocketBehavior::Persistent,
            Duration::from_secs(1),
        )
        .await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();

    let mut prewarm = responses_request();
    prewarm.as_object_mut().unwrap().remove("stream");
    prewarm["type"] = json!("response.create");
    prewarm["model"] = json!("gpt-5.5");
    prewarm["generate"] = json!(false);
    socket
        .send(WsMessage::Text(prewarm.to_string().into()))
        .await
        .unwrap();
    let prewarm_frames = websocket_response_frames(&mut socket).await;
    assert!(
        prewarm_frames
            .iter()
            .all(|frame| !frame.contains("response.output_item.done"))
    );
    let prewarm_response_id = prewarm_frames
        .iter()
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|event| event["type"] == "response.completed")
        .unwrap()["response"]["id"]
        .clone();

    let mut incremental = prewarm.clone();
    incremental.as_object_mut().unwrap().remove("generate");
    incremental["previous_response_id"] = prewarm_response_id.clone();
    incremental["input"] = json!([]);
    socket
        .send(WsMessage::Text(incremental.to_string().into()))
        .await
        .unwrap();
    assert!(
        websocket_response_frames(&mut socket)
            .await
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );

    let mut full = prewarm.clone();
    full.as_object_mut().unwrap().remove("generate");
    full["input"] = json!([{
        "type": "message",
        "role": "user",
        "content": [{"type":"input_text","text":"non-prefix full request"}]
    }]);
    socket
        .send(WsMessage::Text(full.to_string().into()))
        .await
        .unwrap();
    assert!(
        websocket_response_frames(&mut socket)
            .await
            .join("\n")
            .contains("\"type\":\"response.completed\"")
    );

    let official_requests = official_requests.lock().unwrap();
    assert_eq!(official_requests.len(), 3);
    assert_eq!(official_requests[0]["generate"], false);
    assert_eq!(
        official_requests[1]["previous_response_id"],
        prewarm_response_id
    );
    assert_eq!(official_requests[1]["input"], json!([]));
    assert!(official_requests[2].get("previous_response_id").is_none());
    assert_eq!(official_requests[2]["input"], full["input"]);
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn keeps_client_websocket_open_after_partial_official_response_disconnect() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, _, _, official_websocket_connections, _) =
        spawn_mock_official(OfficialWebSocketBehavior::CloseAfterCreated).await;
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
    let failed_frames =
        tokio::time::timeout(WS_RESPONSE_TIMEOUT, websocket_response_frames(&mut socket))
            .await
            .expect("gateway did not report the interrupted official response");
    let failed_events = failed_frames
        .iter()
        .map(|frame| serde_json::from_str::<Value>(frame).unwrap())
        .collect::<Vec<_>>();
    let created_id = failed_events
        .iter()
        .find(|event| event["type"] == "response.created")
        .and_then(|event| event.pointer("/response/id"))
        .and_then(Value::as_str)
        .unwrap();
    let failed_id = failed_events
        .iter()
        .find(|event| event["type"] == "response.failed")
        .and_then(|event| event.pointer("/response/id"))
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(failed_id, created_id);

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
    assert_eq!(official_requests.lock().unwrap().len(), 1);
    assert_eq!(upstream_requests.lock().unwrap().len(), 1);
    assert_eq!(official_websocket_connections.load(Ordering::SeqCst), 1);
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
                "model": "DeepSeek-V4-Flash-custom",
                "generate": false,
                "input": []
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let warmup_frames =
        tokio::time::timeout(WS_RESPONSE_TIMEOUT, websocket_response_frames(&mut socket))
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

    let search_frames =
        tokio::time::timeout(WS_RESPONSE_TIMEOUT, websocket_response_frames(&mut socket))
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

pub(crate) async fn websocket_response_frames<S>(socket: &mut S) -> Vec<String>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let mut frames = Vec::new();
    while let Some(message) = socket.next().await {
        match message.unwrap() {
            WsMessage::Text(text) => {
                let text = text.to_string();
                let terminal = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|event| event.get("type").and_then(Value::as_str).map(str::to_owned))
                    .is_some_and(|event_type| {
                        matches!(
                            event_type.as_str(),
                            "response.completed" | "response.failed" | "response.incomplete"
                        )
                    });
                frames.push(text);
                if terminal {
                    break;
                }
            }
            WsMessage::Close(_) => break,
            _ => {}
        }
    }
    frames
}
