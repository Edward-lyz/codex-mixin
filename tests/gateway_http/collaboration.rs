use super::*;

#[tokio::test]
async fn adapts_collaboration_over_official_http_without_relabeling_ciphertext() {
    for context_enabled in [false, true] {
        let (mut config, requests, _, _codex_home) = mock_official_gateway_config(
            OfficialWebSocketBehavior::CloseAfterCompletedWithCollaborationTool,
            Duration::from_secs(5),
        )
        .await;
        config.context.enabled = context_enabled;
        let database = tempfile::tempdir().unwrap();
        let gateway_url =
            spawn_gateway_with_context(config, &database.path().join("context.db")).await;
        for name in ["spawn_agent", "send_message", "followup_task"] {
            let body = json!({"model":"gpt-5.5", "stream":true,
                "tools":[{"type":"namespace","name":"collaboration","tools":[
                    {"type":"function","name":name,"parameters":{"properties":{
                        "message":{"type":"string","encrypted":true}
                    }}}
                ]}],
                "input":[{"type":"agent_message","author":"/root","recipient":"/root/worker",
                    "content":[{"type":"encrypted_content","encrypted_content":"unknown-opaque-format"}]}]
            });
            let response = reqwest::Client::new()
                .post(format!("{gateway_url}/v1/responses"))
                .bearer_auth("gateway-key")
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let events = drain_events(&mut response.bytes().await.unwrap().to_vec());
            assert_eq!(events.len(), 3);
            for event in &events {
                let payload: Value = serde_json::from_str(&event.data).unwrap();
                let item = payload
                    .get("item")
                    .unwrap_or(&payload["response"]["output"][0]);
                assert_eq!(item["namespace"], "collaboration");
                assert_eq!(item["name"], name);
                assert_eq!(item["encrypted_function_args"], json!([]));
                assert_eq!(item["arguments"], "{\"message\":\"read probe\"}");
            }
            let sent = requests.lock().unwrap().last().unwrap().clone();
            assert_eq!(sent["tools"][0]["name"], "codex_mixin_collaboration");
            assert!(
                sent["tools"][0]["tools"][0]["parameters"]["properties"]["message"]
                    .get("encrypted")
                    .is_none()
            );
            assert_eq!(sent["input"], body["input"]);

            // Codex delivers an explicitly plaintext tool argument as input_text.
            // The third-party child consumes it and replies to an official parent.
            for model in ["Claude Sonnet 5-custom", "gpt-5.5"] {
                let reply = json!({"model":model, "stream":true, "input":[{
                    "type":"agent_message", "author":"/root/worker", "recipient":"/root",
                    "content":[{"type":"input_text", "text":"read probe"}]
                }]});
                let response = reqwest::Client::new()
                    .post(format!("{gateway_url}/v1/responses"))
                    .bearer_auth("gateway-key")
                    .json(&reply)
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
            }
            let reply = requests.lock().unwrap().last().unwrap().clone();
            assert_eq!(reply["input"][0]["type"], "message");
            assert!(
                reply["input"][0]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("read probe")
            );
        }
    }
}

#[tokio::test]
async fn replays_legacy_collaboration_calls_for_declared_tools_only() {
    let (gateway_url, requests, _, _codex_home) = spawn_gateway_with_mock_official(
        OfficialWebSocketBehavior::CloseAfterCompletedWithCollaborationTool,
        Duration::from_secs(5),
    )
    .await;
    let client = reqwest::Client::new();
    let url = format!("{gateway_url}/v1/responses");
    // Simulate a saved upstream-format call followed by its client tool output.
    let body = json!({"model":"gpt-5.5", "stream":true,
        "tools":[{"type":"namespace", "name":"collaboration", "tools":[
            {"type":"function", "name":"send_message", "parameters":{"type":"object",
                "properties":{"message":{"type":"string", "encrypted":true}}}}
        ]}],
        "input":[
            {"type":"function_call", "id":"fc_legacy", "call_id":"call_legacy",
                "namespace":"codex_mixin_collaboration", "name":"send_message",
                "arguments":"{\"message\":\"legacy note\"}"},
            {"type":"function_call_output", "call_id":"call_legacy", "output":"delivered"},
            {"type":"additional_tools", "tools":[{"type":"namespace", "name":"collaboration",
                "tools":[{"type":"function", "name":"wait_agent", "parameters":{"type":"object"}}]}]},
            {"type":"function_call", "id":"fc_wait", "call_id":"call_wait",
                "namespace":"codex_mixin_collaboration", "name":"wait_agent",
                "arguments":"{}", "encrypted_function_args":null},
            {"type":"function_call_output", "call_id":"call_wait", "output":"done"},
            {"type":"function_call", "id":"fc_native", "call_id":"call_native",
                "namespace":"collaboration", "name":"send_message",
                "arguments":"{\"message\":\"opaque-native-payload\"}"}
        ]
    });
    let response = client
        .post(&url)
        .bearer_auth("gateway-key")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let mut wire = response.bytes().await.unwrap().to_vec();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&wire));
    let events = drain_events(&mut wire);
    assert_eq!(events.len(), 3);
    for event in events {
        let payload: Value = serde_json::from_str(&event.data).unwrap();
        let item = payload
            .get("item")
            .unwrap_or(&payload["response"]["output"][0]);
        assert_eq!(item["namespace"], "collaboration");
        assert_eq!(item["encrypted_function_args"], json!([]));
    }
    {
        let requests = requests.lock().unwrap();
        let sent = &requests[0];
        assert_eq!(sent["tools"][0]["name"], "codex_mixin_collaboration");
        let mut expected = body["input"].clone();
        expected[2]["tools"][0]["name"] = json!("codex_mixin_collaboration");
        assert_eq!(sent["input"], expected);
    }

    let mut unknown_tool = body.clone();
    unknown_tool["input"][0]["name"] = json!("unlisted_tool");
    let mut encrypted_call = body.clone();
    encrypted_call["input"][0]["encrypted_function_args"] = json!(["message"]);
    let mut malformed_metadata = body.clone();
    malformed_metadata["input"][0]["encrypted_function_args"] = json!("message");
    let mut reserved_declaration = body.clone();
    reserved_declaration["input"][2]["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"namespace", "name":"codex_mixin_collaboration", "tools":[]}));
    for (case, rejected) in [
        ("unlisted tool", unknown_tool),
        ("encrypted call", encrypted_call),
        ("malformed encryption metadata", malformed_metadata),
        ("reserved tool declaration", reserved_declaration),
    ] {
        let response = client
            .post(&url)
            .bearer_auth("gateway-key")
            .json(&rejected)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{case}");
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("collaboration compatibility namespace is already in use")
        );
    }
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn official_account_collaboration_replays_after_gateway_restart() {
    let (official_url, requests, _, _, _, _) =
        spawn_mock_official(OfficialWebSocketBehavior::CloseAfterCompletedWithCollaborationTool)
            .await;
    let root = tempfile::tempdir().unwrap();
    let mut provider = ProviderPreset::Official.create("official-fixture", "");
    provider.base_url = official_url;
    provider.api_path = "/v1/responses".to_owned();
    provider.model_source = ProviderModelSource::Static;
    provider.selected_models = vec!["gpt-5.5".to_owned()];
    provider.cached_models = vec![ProviderModel {
        id: "gpt-5.5".to_owned(),
        ..Default::default()
    }];
    let mut stored = codex_mixin::config::StoredGatewayConfig {
        providers: vec![provider],
        gateway_api_key: Some("gateway-key".to_owned()),
        ..Default::default()
    };
    stored.context.enabled = true;
    codex_mixin::config::save_stored_config_to_path(&root.path().join("config.json"), &stored)
        .unwrap();
    let credentials = root.path().join("oauth/accounts/official-fixture.json");
    std::fs::create_dir_all(credentials.parent().unwrap()).unwrap();
    std::fs::write(
        credentials,
        r#"{"tokens":{"access_token":"fixture-token","account_id":"fixture-account"}}"#,
    )
    .unwrap();
    let client = reqwest::Client::new();
    let mut body = json!({"model":"gpt-5.5-official-fixture", "stream":true,
        "tools":[{"type":"namespace","name":"collaboration","tools":[
            {"type":"function","name":"send_message","parameters":{"properties":{
                "message":{"type":"string","encrypted":true}
            }}}
        ]}], "input":[{"type":"agent_message", "author":"/root", "recipient":"/root/worker",
            "content":[{"type":"input_text","text":"read probe"}]}]
    });
    for turn in 0..2 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let mut gateway = common::tokio_cli(root.path())
            .args(["serve", "--bind", &address.to_string(), "--no-client-sync"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let url = format!("http://{address}");
        tokio::time::timeout(Duration::from_secs(15), async {
            while client.get(format!("{url}/healthz")).send().await.is_err() {
                assert!(
                    gateway.try_wait().unwrap().is_none(),
                    "gateway exited before listening"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("gateway did not listen");
        let response = client
            .post(format!("{url}/v1/responses"))
            .bearer_auth("gateway-key")
            .header("session-id", "collaboration-restart")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let events = drain_events(&mut response.bytes().await.unwrap().to_vec());
        let payload: Value = serde_json::from_str(
            &events
                .iter()
                .find(|event| event.data.contains("response.output_item.done"))
                .unwrap()
                .data,
        )
        .unwrap();
        let call = &payload["item"];
        assert_eq!(call["namespace"], "collaboration");
        assert_eq!(call["encrypted_function_args"], json!([]));
        let sent = requests.lock().unwrap().last().unwrap().clone();
        assert_eq!(sent["tools"][0]["name"], "codex_mixin_collaboration");
        assert!(
            sent["tools"][0]["tools"][0]["parameters"]["properties"]["message"]
                .get("encrypted")
                .is_none()
        );
        assert_eq!(sent["input"][0]["type"], "message");
        if turn == 1 {
            assert_eq!(sent["input"][1]["namespace"], "codex_mixin_collaboration");
            assert_eq!(sent["input"][1]["encrypted_function_args"], json!([]));
        } else {
            body["input"].as_array_mut().unwrap().extend([
                call.clone(), json!({"type":"function_call_output","call_id":"call_delegation","output":"delivered"})
            ]);
        }
        gateway.kill().await.unwrap();
    }
}

#[tokio::test]
async fn adapts_official_collaboration_schema_and_calls_after_reconnect() {
    let (gateway_url, requests, connections, _codex_home) = spawn_gateway_with_mock_official(
        OfficialWebSocketBehavior::CloseAfterCompletedWithCollaborationTool,
        Duration::from_secs(5),
    )
    .await;
    let mut request = format!(
        "{}/v1/responses",
        gateway_url.replacen("http://", "ws://", 1)
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let first = json!({
        "type":"response.create", "model":"gpt-5.5",
        "input":[{"type":"agent_message", "author":"/root", "recipient":"/root/worker",
            "content":[{"type":"encrypted_content", "encrypted_content":"unknown-opaque-format"}]}],
        "tools":[{"type":"namespace","name":"collaboration","tools":[
            {"type":"function","name":"spawn_agent","parameters":{"type":"object",
                "properties":{"message":{"type":"string","encrypted":true}}}}
        ]}]
    });
    socket
        .send(WsMessage::Text(first.to_string().into()))
        .await
        .unwrap();
    let frames = websocket_response_frames(&mut socket).await;
    let events: Vec<Value> = frames
        .iter()
        .map(|frame| serde_json::from_str(frame).unwrap())
        .collect();
    let item = &events
        .iter()
        .find(|event| event["type"] == "response.output_item.done")
        .unwrap()["item"];
    assert_eq!(
        events
            .iter()
            .find(|event| event["type"] == "response.output_item.added")
            .unwrap()["item"],
        *item
    );
    assert_eq!(item["namespace"], "collaboration");
    assert_eq!(item["name"], "spawn_agent");
    assert_eq!(item["arguments"], "{\"message\":\"read probe\"}");
    assert_eq!(item["encrypted_function_args"], json!([]));
    let completed = events
        .iter()
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    assert_eq!(completed["response"]["output"][0], *item);
    tokio::time::sleep(Duration::from_millis(25)).await;
    let mut follow = first.clone();
    follow["previous_response_id"] = completed["response"]["id"].clone();
    follow["input"] =
        json!([{"type":"function_call_output","call_id":"call_delegation","output":"child done"}]);
    socket
        .send(WsMessage::Text(follow.to_string().into()))
        .await
        .unwrap();
    let frames = tokio::time::timeout(
        Duration::from_secs(5),
        websocket_response_frames(&mut socket),
    )
    .await
    .unwrap();
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("response.completed"))
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert_eq!(request["tools"][0]["name"], "codex_mixin_collaboration");
        assert!(
            request["tools"][0]["tools"][0]["parameters"]["properties"]["message"]
                .get("encrypted")
                .is_none()
        );
    }
    assert_eq!(requests[0]["input"], first["input"]);
    assert_eq!(requests[1]["input"][0], first["input"][0]);
    assert!(requests[1].get("previous_response_id").is_none());
    assert_eq!(requests[1]["input"].as_array().unwrap().len(), 3);
    assert_eq!(
        requests[1]["input"][1]["namespace"],
        "codex_mixin_collaboration"
    );
    assert_eq!(requests[1]["input"][1]["call_id"], "call_delegation");
    assert_eq!(requests[1]["input"][2]["output"], "child done");
    assert_eq!(connections.load(Ordering::SeqCst), 2);
}

fn delegation_request() -> Value {
    let mut body = responses_request();
    body["model"] = json!("gpt-5.5");
    body["input"].as_array_mut().unwrap().push(json!({
        "type": "agent_message",
        "author": "/root/worker",
        "recipient": "/root",
        "content": [
            {"type":"input_text","text":"Message Type: MESSAGE\nPayload:\n"},
            {"type":"encrypted_content","encrypted_content":"opaque-agent-payload"}
        ]
    }));
    body
}

#[tokio::test]
async fn preserves_opaque_agent_messages_when_official_http_rejects_them() {
    let (gateway_url, official_requests, _, _codex_home) = spawn_gateway_with_mock_official(
        OfficialWebSocketBehavior::NativeAgentEnvelopeGate,
        Duration::from_secs(5),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&delegation_request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let captured = official_requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0]["input"][2]["content"][1]["type"],
        "encrypted_content"
    );
    assert_eq!(captured[0]["input"][2], delegation_request()["input"][2]);
}

#[tokio::test]
async fn keeps_official_http_envelopes_when_the_request_succeeds() {
    let (gateway_url, official_requests, _, _codex_home) = spawn_gateway_with_mock_official(
        OfficialWebSocketBehavior::Persistent,
        Duration::from_secs(5),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&delegation_request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.text().await.unwrap();

    let captured = official_requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0]["input"][2]["content"][1]["type"],
        "encrypted_content"
    );
}

#[tokio::test]
async fn official_route_replays_provider_custom_tool_calls_with_ctc_ids() {
    let (gateway_url, official_requests, _, _codex_home) = spawn_gateway_with_mock_official(
        OfficialWebSocketBehavior::Persistent,
        Duration::from_secs(5),
    )
    .await;
    let body = json!({"model":"gpt-5.5", "stream":true, "input":[
        {"type":"custom_tool_call", "id":"fc_provider", "call_id":"call_patch",
            "name":"apply_patch", "input":"*** Begin Patch", "status":"completed"},
        {"type":"custom_tool_call_output", "call_id":"call_patch", "output":"ok"},
        {"type":"custom_tool_call", "id":"ctc_native", "call_id":"call_native",
            "name":"apply_patch", "input":"*** Begin Patch", "status":"completed"},
        {"type":"function_call", "id":"fc_function", "call_id":"call_shell",
            "name":"shell", "arguments":"{}", "status":"completed"}
    ]});
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.text().await.unwrap();

    let captured = official_requests.lock().unwrap();
    let input = &captured[0]["input"];
    assert_eq!(input[0]["id"], "ctc_provider");
    assert_eq!(input[0]["call_id"], "call_patch");
    assert_eq!(input[2]["id"], "ctc_native");
    assert_eq!(input[3]["id"], "fc_function");
}

#[tokio::test]
async fn preserves_opaque_agent_messages_when_official_websocket_retries() {
    let (gateway_url, official_requests, connections, _codex_home) =
        spawn_gateway_with_mock_official(
            OfficialWebSocketBehavior::NativeAgentEnvelopeGate,
            Duration::from_millis(50),
        )
        .await;
    let mut request = format!(
        "{}/v1/responses",
        gateway_url.replacen("http://", "ws://", 1)
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = delegation_request();
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
    .expect("gateway did not retry the official websocket")
    .join("\n");
    assert!(frames.contains("\"type\":\"response.failed\""));
    assert!(!frames.contains("\"type\":\"response.completed\""));

    let captured = official_requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    for request in captured.iter() {
        assert_eq!(request["input"][2], delegation_request()["input"][2]);
    }
    assert_eq!(connections.load(Ordering::SeqCst), 2);
}
