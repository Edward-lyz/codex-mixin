use super::*;

#[tokio::test]
async fn preserves_native_collaboration_over_official_http() {
    let (gateway_url, requests, _, _codex_home) = spawn_gateway_with_mock_official(
        OfficialWebSocketBehavior::Persistent,
        Duration::from_secs(5),
    )
    .await;
    let body = json!({"model":"gpt-5.5", "stream":true,
        "tools":[{"type":"namespace","name":"collaboration","tools":[
            {"type":"function","name":"spawn_agent","parameters":{"properties":{
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
    assert_eq!(
        response.text().await.unwrap(),
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\"}}\n\n"
    );
    let sent = &requests.lock().unwrap()[0];
    assert_eq!(sent["tools"], body["tools"]);
    assert_eq!(sent["input"], body["input"]);
}

#[tokio::test]
async fn preserves_native_collaboration_schema_and_calls_after_reconnect() {
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
    assert_eq!(item["namespace"], "collaboration");
    assert_eq!(item["name"], "spawn_agent");
    assert_eq!(item["arguments"], "{\"message\":\"read probe\"}");
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
        assert_eq!(request["tools"], first["tools"]);
    }
    assert_eq!(requests[0]["input"], first["input"]);
    assert_eq!(requests[1]["input"][0], first["input"][0]);
    assert!(requests[1].get("previous_response_id").is_none());
    assert_eq!(requests[1]["input"].as_array().unwrap().len(), 3);
    assert_eq!(requests[1]["input"][1]["namespace"], "collaboration");
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
            {"type":"encrypted_content","encrypted_content":"local plaintext task"}
        ]
    }));
    body
}

#[tokio::test]
async fn retries_official_http_with_materialized_agent_messages() {
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
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("response.completed"));

    let captured = official_requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(
        captured[0]["input"][2]["content"][1]["type"],
        "encrypted_content"
    );
    assert_eq!(captured[1]["input"][2]["content"][1]["type"], "input_text");
    assert_eq!(
        captured[1]["input"][2]["content"][1]["text"],
        "local plaintext task"
    );
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
async fn retries_official_websocket_with_materialized_agent_messages() {
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
    assert!(frames.contains("\"type\":\"response.completed\""));
    assert!(!frames.contains("\"type\":\"response.failed\""));

    let captured = official_requests.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(
        captured[0]["input"][2]["content"][1]["type"],
        "encrypted_content"
    );
    assert_eq!(captured[1]["input"][2]["content"][1]["type"], "input_text");
    assert_eq!(
        captured[1]["input"][2]["content"][1]["text"],
        "local plaintext task"
    );
    assert_eq!(connections.load(Ordering::SeqCst), 2);
}
