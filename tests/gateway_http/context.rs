use super::*;

#[tokio::test]
async fn keeps_the_provider_prompt_prefix_append_only_across_turns() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let mut first = responses_request();
    first["prompt_cache_key"] = json!("prefix-cache-session");

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&first)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.unwrap();

    let mut second = first.clone();
    second["input"].as_array_mut().unwrap().extend([
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello openai"}]}),
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":"and again"}]}),
    ]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&second)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.unwrap();

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    // The provider only reuses its cache while the earlier prompt is an exact
    // prefix of the next one: same system and tool preamble, and every earlier
    // message byte-identical in place.
    assert_eq!(recorded[0]["system"], recorded[1]["system"]);
    assert_eq!(recorded[0]["tools"], recorded[1]["tools"]);
    let earlier = recorded[0]["messages"].as_array().unwrap();
    let current = recorded[1]["messages"].as_array().unwrap();
    assert!(
        current.len() > earlier.len(),
        "second turn did not append messages: {current:?}"
    );
    assert_eq!(&current[..earlier.len()], earlier.as_slice());
}

#[tokio::test]
async fn custom_tool_loop_marks_commentary_and_final_answer() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ToolThenText).await;
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
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let events = drain_events(&mut encoded);
    let done_items = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("response.output_item.done"))
        .map(|event| serde_json::from_str::<Value>(&event.data).unwrap()["item"].clone())
        .collect::<Vec<_>>();
    let commentary = done_items
        .iter()
        .find(|item| item["type"] == "message")
        .unwrap();
    assert_eq!(commentary["phase"], "commentary");
    let tool_call = done_items
        .iter()
        .find(|item| item["type"] == "function_call")
        .unwrap();
    let call_id = tool_call["call_id"].as_str().unwrap();

    let mut follow_up = responses_request();
    follow_up["input"] = json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"check the directory"}]},
        {
            "type":"function_call",
            "call_id":call_id,
            "name":tool_call["name"],
            "arguments":tool_call["arguments"]
        },
        {"type":"function_call_output","call_id":call_id,"output":"/tmp"}
    ]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&follow_up)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let events = drain_events(&mut encoded);
    let final_answer: Value = serde_json::from_str(
        &events
            .iter()
            .find(|event| {
                event.event.as_deref() == Some("response.output_item.done")
                    && serde_json::from_str::<Value>(&event.data)
                        .ok()
                        .is_some_and(|event| event["item"]["type"] == "message")
            })
            .unwrap()
            .data,
    )
    .unwrap();
    assert_eq!(final_answer["item"]["phase"], "final_answer");
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn context_status_round_is_hidden_from_the_client() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextStatusThenText).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-status")
        .json(&responses_request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap();
    assert!(stream.contains("hello codex"), "missing answer: {stream}");
    assert!(
        !stream.contains("mixin_context"),
        "internal tool leaked to the client: {stream}"
    );

    let recorded = requests.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "context round count: {recorded:?}");
    let tools = recorded[0]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 5);
    assert!(tools.iter().any(|tool| tool["name"] == "exec_command"));
    for name in [
        "mixin_context_compress",
        "mixin_context_decompress",
        "mixin_context_search",
        "mixin_context_status",
    ] {
        assert!(
            tools.iter().any(|tool| tool["name"] == name),
            "missing internal tool {name}"
        );
    }
    assert!(recorded[0].to_string().contains("<context_management>"));
    let follow_up = recorded[1]["messages"].to_string();
    assert!(follow_up.contains("mixin_context_status"), "{follow_up}");
    assert!(follow_up.contains("revision"), "{follow_up}");
    assert!(database_path.exists(), "context database was not created");
}

/// Guidance-only is the ablation arm that keeps the compression advice while
/// leaving the model without the compression tools, so the archived history
/// stays exactly as the client sent it.
#[tokio::test]
async fn context_guidance_only_round_leaves_the_archive_alone() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    config.context.guidance_only = true;
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-guidance")
        .json(&responses_request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap();
    assert!(stream.contains("hello codex"), "missing answer: {stream}");

    let recorded = requests.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1, "guidance-only round count: {recorded:?}");
    let tools = recorded[0]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "exec_command");
    assert!(recorded[0].to_string().contains("<context_management>"));
}

#[tokio::test]
async fn context_compression_round_rewrites_the_upstream_view() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextCompressThenText).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    config.context.protected_messages = 1;
    config.context.protected_tokens = 1;
    configure_baidu_policy(&mut config);
    config.thinking_mode = ThinkingMode::Auto;
    config.providers[0].cached_models[0].supports_thinking = Some(true);
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;

    let mut body = responses_request();
    body["input"] = json!([
        {"type":"message","role":"developer","content":[{"type":"input_text","text":"dev rules"}]},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"explore two ideas"}]},
        {
            "type":"message",
            "role":"assistant",
            "content":[{"type":"output_text","text":format!("alpha {}", "word ".repeat(400))}]
        },
        {
            "type":"message",
            "role":"assistant",
            "content":[{"type":"output_text","text":format!("beta {}", "word ".repeat(400))}]
        },
        {"type":"configuration_update","reasoning":{"effort":"high"}},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}
    ]);
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-compress")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap();
    assert!(stream.contains("hello codex"), "missing answer: {stream}");
    assert!(
        !stream.contains("<context_summary"),
        "summary leaked to the client: {stream}"
    );

    let recorded = requests.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "context round count: {recorded:?}");
    for request in &recorded {
        assert_eq!(request["output_config"]["effort"], "high");
    }
    let compressed = recorded[1]["messages"].to_string();
    assert!(compressed.contains("<context_summary"), "{compressed}");
    assert!(
        compressed.contains("alpha and beta explored two ideas"),
        "{compressed}"
    );
    assert!(
        !compressed.contains("alpha word"),
        "original messages were not replaced: {compressed}"
    );
    let blocks = recorded[1]["messages"][0]["content"]
        .as_array()
        .expect("merged user turn");
    assert_eq!(blocks.len(), 3);
    assert!(blocks[1]["text"].as_str().unwrap().contains("tier=\"1\""));
    let tool_result = recorded[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["content"].as_array().cloned().unwrap_or_default())
        .find(|block| block["type"] == "tool_result")
        .expect("hidden tool result");
    let result: Value =
        serde_json::from_str(tool_result["content"].as_str().unwrap()).expect("tool result json");
    assert_eq!(result["ok"], true);
    assert_eq!(result["block_id"], "b1");
    assert_eq!(result["revision"], 1);
}

#[tokio::test]
async fn context_repeated_compress_failure_stops_early() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextInvalidCompress).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let gateway_url = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-repeated-failure")
        .json(&responses_request())
        .send()
        .await
        .unwrap();
    let stream = response.text().await.unwrap();
    assert!(stream.contains("response.completed"), "{stream}");
    assert!(!stream.contains("response.failed"), "{stream}");
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn context_thinking_normalization_survives_compress_and_client_tool_replay() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = Arc::clone(&requests);
    let mut payloads = Vec::new();
    for (round, name, arguments) in [
        (
            0,
            "mixin_context_compress",
            json!({
                "ranges":[{"start":"m000003","end":"m000004"}],
                "topic":"exploration", "summary":"alpha and beta explored two ideas"
            }),
        ),
        (1, "exec_command", json!({"cmd":"pwd"})),
    ] {
        let mut encoded = text_then_tool_sse(name, arguments).into_bytes();
        let mut events: Vec<Value> = drain_events(&mut encoded)
            .into_iter()
            .map(|event| serde_json::from_str(&event.data).unwrap())
            .collect();
        for event in &mut events {
            if let Some(index) = event.get_mut("index") {
                *index = json!(index.as_u64().unwrap() + 1);
            }
            if event["content_block"]["type"] == "tool_use" {
                event["content_block"]["id"] = json!(format!("call_{round}"));
            }
        }
        events.splice(1..1, [
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":format!("thought {round}")}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":format!("signature-{round}")}}),
            json!({"type":"content_block_stop","index":0}),
        ]);
        payloads.push(
            events
                .into_iter()
                .map(|event| {
                    format!(
                        "event: {}\ndata: {event}\n\n",
                        event["type"].as_str().unwrap()
                    )
                })
                .collect::<String>(),
        );
    }
    let upstream = spawn_router(Router::new().route(
        "/v1/messages",
        post(move |Json(body): Json<Value>| {
            let captured = Arc::clone(&captured);
            let payloads = payloads.clone();
            async move {
                let index = {
                    let mut requests = captured.lock().unwrap();
                    let index = requests.len();
                    requests.push(body);
                    index
                };
                Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(
                        payloads.get(index).cloned().unwrap_or_else(text_sse),
                    ))
                    .unwrap()
            }
        }),
    ))
    .await;
    let mut config = test_config(upstream);
    config.context.enabled = true;
    config.context.protected_messages = 1;
    config.context.protected_tokens = 1;
    let database = tempfile::tempdir().unwrap();
    let path = database.path().join("context.db");
    let gateway = spawn_gateway_with_context(config, &path).await;
    let client = reqwest::Client::new();
    let mut body = responses_request();
    body["input"] = json!([
        {"type":"message","role":"developer","content":"dev rules"},
        {"type":"message","role":"user","content":"explore two ideas"},
        {"type":"message","role":"assistant","content":format!("alpha {}", "word ".repeat(400))},
        {"type":"message","role":"assistant","content":format!("beta {}", "word ".repeat(400))},
        {"type":"message","role":"user","content":"continue"}
    ]);
    let response = client
        .post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thinking-compress")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let completed: Value = drain_events(&mut encoded)
        .into_iter()
        .find(|event| event.event.as_deref() == Some("response.completed"))
        .map(|event| serde_json::from_str(&event.data).unwrap())
        .unwrap();
    assert!(!completed.to_string().contains("mixin_context_compress"));
    let output = completed["response"]["output"].as_array().unwrap();
    assert_eq!(
        output
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .count(),
        2
    );
    body["previous_response_id"] = completed["response"]["id"].clone();
    body["input"] =
        json!([{"type":"function_call_output","call_id":"call_1","output":"/workspace"}]);
    let response = client
        .post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thinking-compress")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = response.text().await.unwrap();
    assert!(
        stream.contains("response.completed") && !stream.contains("response.failed"),
        "{stream}"
    );

    let recorded = requests.lock().unwrap().clone();
    assert_eq!(recorded.len(), 3);
    assert!(
        recorded[1]
            .to_string()
            .contains("alpha and beta explored two ideas")
    );
    let messages = recorded[2]["messages"].as_array().unwrap();
    let assistant = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .unwrap();
    assert_eq!(
        assistant["content"],
        json!([
            {"type":"thinking","thinking":"thought 0","signature":"signature-0"},
            {"type":"thinking","thinking":"thought 1","signature":"signature-1"},
            {"type":"text","text":"checking"}, {"type":"text","text":"checking"},
            {"type":"tool_use","id":"call_1","name":"exec_command","input":{"cmd":"pwd"}}
        ])
    );
    assert!(recorded[2].to_string().contains("/workspace"));
    assert!(
        !recorded[2]["messages"]
            .to_string()
            .contains("mixin_context_compress")
    );
}

#[tokio::test]
async fn context_thinking_normalization_repairs_reopened_legacy_archives() {
    for enabled in [true, false] {
        let (upstream, requests) = spawn_mock_upstream(MockMode::Text).await;
        let mut config = test_config(upstream.clone());
        config.context.enabled = true;
        let database = tempfile::tempdir().unwrap();
        let path = database.path().join("context.db");
        let gateway = spawn_gateway_with_context(config, &path).await;
        let first = json!({"type":"thinking","thinking":"old first","signature":"old-signature-1"});
        let last = json!({"type":"thinking","thinking":"old last","signature":"old-signature-2"});
        // Persist the pre-fix archive shape without calling the converter.
        let legacy = json!([
            {"type":"message","role":"user","content":"old request"},
            {"type":"reasoning","encrypted_content":format!("codex-mixin:anthropic-thinking:v1:{first}")},
            {"type":"message","role":"assistant","content":"before hidden compress"},
            {"type":"reasoning","encrypted_content":format!("codex-mixin:anthropic-thinking:v1:{last}")},
            {"type":"message","role":"assistant","content":"old answer"}
        ]);
        let response = reqwest::Client::new()
            .post(format!("{gateway}/v1/responses/compact"))
            .bearer_auth("gateway-key")
            .header("x-session-id", "legacy-thinking")
            .json(&json!({"model":"DeepSeek-V4-Flash-custom","input":legacy}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let checkpoint: Value = response.json().await.unwrap();
        assert!(requests.lock().unwrap().is_empty());
        let mut config = test_config(upstream);
        config.context.enabled = enabled;
        let reopened = spawn_gateway_with_context(config, &path).await;
        let response = replay_context_checkpoint(
            &reopened,
            "legacy-thinking",
            checkpoint["output"][0]["encrypted_content"]
                .as_str()
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let stream = response.text().await.unwrap();
        assert!(
            stream.contains("response.completed") && !stream.contains("response.failed"),
            "{stream}"
        );
        let captured = requests.lock().unwrap().clone();
        assert_eq!(captured.len(), 1);
        let assistant = captured[0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "assistant")
            .unwrap();
        assert_eq!(
            assistant["content"],
            json!([
                first, last,
                {"type":"text","text":"before hidden compress"},
                {"type":"text","text":"old answer"}
            ])
        );
        let db = rusqlite::Connection::open(&path).unwrap();
        let archived: Vec<Value> = db.prepare("SELECT m.item_json FROM history h JOIN messages m ON m.session_key=h.session_key AND m.reference=h.reference ORDER BY h.position")
            .unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap()).collect();
        assert_eq!(
            &archived[..legacy.as_array().unwrap().len()],
            legacy.as_array().unwrap()
        );
    }
}

#[tokio::test]
async fn context_internal_rounds_are_bounded() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextStatusOnly).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    config.context.max_internal_rounds = 1;
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-bounded")
        .json(&responses_request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let events = drain_events(&mut encoded);
    assert!(
        !events
            .iter()
            .any(|event| event.event.as_deref() == Some("response.failed"))
    );
    let completed: Vec<_> = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("response.completed"))
        .collect();
    assert_eq!(completed.len(), 1, "{events:?}");
    let payload: Value = serde_json::from_str(&completed[0].data).unwrap();
    assert_eq!(payload["response"]["status"], "completed");
    assert_eq!(payload["response"]["output"], json!([]));
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn context_mixed_tools_hand_back_client_calls_only() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextMixedToolsThenText).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;
    let client = reqwest::Client::new();
    let user = json!({"type":"message","role":"user","content":[{"type":"input_text","text":"check the status and list files"}]});
    let mut request = responses_request();
    request["input"] = json!([user.clone()]);

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-mixed")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let first = String::from_utf8_lossy(&encoded).to_string();
    let events = drain_events(&mut encoded);
    let call = events
        .iter()
        .find_map(|event| {
            let payload: Value = serde_json::from_str(&event.data).ok()?;
            payload
                .get("item")
                .filter(|item| item["name"] == "exec_command")
                .cloned()
        })
        .expect("client call item");
    assert_eq!(call["type"], "function_call");
    assert!(
        !first.contains("mixin_context"),
        "internal tool leaked: {first}"
    );
    assert!(
        first.contains("exec_command"),
        "client call missing: {first}"
    );
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "model must stop at the hand-back"
    );

    let mut follow_up = request.clone();
    follow_up["input"] = json!([
        user,
        call.clone(),
        {"type":"function_call_output","call_id": call["call_id"], "output": "/tmp/workspace"},
    ]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-mixed")
        .json(&follow_up)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap();
    assert!(stream.contains("hello codex"), "missing answer: {stream}");
    assert!(
        !stream.contains("mixin_context"),
        "internal tool leaked: {stream}"
    );

    let recorded = requests.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "round count: {recorded:?}");
    let resumed = recorded[1]["messages"].to_string();
    assert!(
        resumed.contains("/tmp/workspace"),
        "client result was lost: {resumed}"
    );
}

#[tokio::test]
async fn context_expands_a_chained_request_from_the_archive() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextStatusThenText).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;
    let client = reqwest::Client::new();

    let mut first = responses_request();
    first["input"] = json!([{
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": "distinctive opening request"}]
    }]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-chained")
        .json(&first)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let events = drain_events(&mut encoded);
    let response_id = events
        .iter()
        .find(|event| event.event.as_deref() == Some("response.completed"))
        .and_then(|event| serde_json::from_str::<Value>(&event.data).ok())
        .and_then(|payload| payload["response"]["id"].as_str().map(str::to_owned))
        .expect("completed response id");
    assert!(!response_id.is_empty());

    // The follow-up carries only the delta and the response id, so the gateway
    // has to rebuild the full input from its own archive.
    let mut chained = responses_request();
    chained["previous_response_id"] = json!(response_id);
    chained["input"] = json!([{
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": "next question"}]
    }]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-chained")
        .json(&chained)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap();
    assert!(stream.contains("hello codex"), "missing answer: {stream}");

    let recorded = requests.lock().unwrap().clone();
    assert_eq!(recorded.len(), 4, "round count: {recorded:?}");
    let expanded = recorded[2]["messages"].to_string();
    assert!(
        expanded.contains("distinctive opening request"),
        "archived history was lost: {expanded}"
    );
    assert!(expanded.contains("next question"), "{expanded}");

    // Without an archive the gateway must fail loudly instead of silently
    // dropping the conversation.
    let mut unknown = responses_request();
    unknown["previous_response_id"] = json!("resp_unknown");
    unknown["input"] = json!([]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-chained")
        .json(&unknown)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.text().await.unwrap();
    assert!(body.contains("unknown response id"), "{body}");
}

#[tokio::test]
async fn context_chained_request_continues_after_expired_compressed_bodies() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextCompressThenText).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    config.context.protected_messages = 1;
    config.context.protected_tokens = 1;
    configure_baidu_policy(&mut config);
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;
    let client = reqwest::Client::new();

    let mut first = responses_request();
    first["input"] = json!([
        {"type":"message","role":"developer","content":[{"type":"input_text","text":"dev rules"}]},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"explore two ideas"}]},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":format!("alpha {}", "word ".repeat(400))}]},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":format!("beta {}", "word ".repeat(400))}]},
        {"type":"configuration_update","reasoning":{"effort":"high"}},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}
    ]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-retention")
        .json(&first)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let completed = drain_events(&mut encoded)
        .into_iter()
        .find(|event| event.event.as_deref() == Some("response.completed"))
        .and_then(|event| serde_json::from_str::<Value>(&event.data).ok())
        .expect("completed response event");
    let response_id = completed["response"]["id"].as_str().expect("response id");

    // Lossless maintenance must keep even idle histories and preserve exact
    // JSON bodies while moving their storage to compressed SQLite BLOBs.
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let legacy_body = serde_json::to_string(&first["input"][2]).unwrap();
    connection
        .execute(
            "UPDATE messages SET item_json = ?1, body_codec = 0, body_bytes = 0,
        body_checksum = NULL, body_kind = NULL WHERE reference = 'm000003'",
            [&legacy_body],
        )
        .unwrap();
    let legacy_reply = format!("{}{}", " ".repeat(512), completed["response"]["output"]);
    connection
        .execute(
            "UPDATE responses SET output_json = ?1, body_codec = 0, body_bytes = 0,
        body_checksum = NULL",
            [&legacy_reply],
        )
        .unwrap();
    // A fork can leave old messages without live references. They remain history.
    connection.execute("INSERT INTO messages(session_key, reference, fingerprint, item_json, created_at)
        SELECT session_key, 'm999999', fingerprint, item_json, created_at FROM messages WHERE reference = 'm000003'", []).unwrap();
    connection
        .execute(
            "UPDATE sessions SET updated_at = unixepoch() - 40 * 86400",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE messages SET created_at = unixepoch() - 40 * 86400",
            [],
        )
        .unwrap();
    let original_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    connection.execute_batch("BEGIN").unwrap();
    let _: i64 = connection
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    let pinned_reader = connection;
    let maintained = common::cli(database.path())
        .env("CODEX_MIXIN_CONTEXT_DB", &database_path)
        .args([
            "context",
            "maintain",
            "--batches",
            "100",
            "--compact",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        maintained.status.success(),
        "lossless maintenance failed: {}",
        String::from_utf8_lossy(&maintained.stderr)
    );
    let pinned_report: Value = serde_json::from_slice(&maintained.stdout).unwrap();
    assert!(
        pinned_report["wal"]["log_pages"].as_i64().unwrap()
            > pinned_report["wal"]["checkpointed_pages"].as_i64().unwrap(),
        "maintenance must report a reader-pinned WAL without deleting it"
    );
    pinned_reader.execute_batch("COMMIT").unwrap();
    drop(pinned_reader);
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let after_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        after_count, original_count,
        "maintenance must retain idle history"
    );
    let compressed: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE body_codec = 1 AND typeof(item_json) = 'blob'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        compressed > 0,
        "large original bodies must use lossless compression"
    );
    let replies: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM responses WHERE body_codec = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        replies > 0,
        "completed replies also need lossless compression"
    );
    assert!(
        std::fs::read_dir(database.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".bak")
        }),
        "in-place migration must not create a whole-database backup"
    );
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    let maintenance: Value = serde_json::from_slice(&maintained.stdout).unwrap();
    assert_eq!(maintenance["missing_messages"], 0);
    let preserved_session: String = connection
        .query_row("SELECT session_key FROM sessions LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    drop(connection);
    let exported = common::cli(database.path())
        .env("CODEX_MIXIN_CONTEXT_DB", &database_path)
        .args(["context", "export", "--session", &preserved_session])
        .output()
        .unwrap();
    assert!(
        exported.status.success(),
        "compressed export failed: {}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let exported: Value = serde_json::from_slice(&exported.stdout).unwrap();
    for (actual, expected) in exported["history"]
        .as_array()
        .unwrap()
        .iter()
        .zip(first["input"].as_array().unwrap())
    {
        assert_eq!(
            &actual["item"], expected,
            "lossless export must match original input"
        );
    }

    let engine = codex_mixin::context::ContextEngine::open(
        codex_mixin::config::ContextSettings {
            enabled: true,
            protected_messages: 1,
            protected_tokens: 1,
            ..Default::default()
        },
        &database_path,
    )
    .unwrap();
    let revision = engine
        .status(&preserved_session, 100_000, true)
        .unwrap()
        .revision;
    let search = engine
        .execute_tool(
            &preserved_session,
            revision,
            "mixin_context_search",
            &json!({"query":"alpha word","limit":20}),
            100_000,
        )
        .unwrap();
    let search: Value = serde_json::from_str(&search.content).unwrap();
    assert_eq!(search["complete"], true);
    assert!(
        search["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hit| hit["reference"] == "m000003"),
        "search must decode compressed originals"
    );
    let block = &exported["blocks"][0];
    let compressed_tokens = engine
        .status(&preserved_session, 100_000, true)
        .unwrap()
        .estimated_view_tokens;
    let restored = engine
        .execute_tool(
            &preserved_session,
            revision,
            "mixin_context_decompress",
            &json!({"block_id":block["id"],"apply":true}),
            100_000,
        )
        .unwrap();
    let restored_status = engine.status(&preserved_session, 100_000, true).unwrap();
    assert!(
        restored_status.estimated_view_tokens > compressed_tokens,
        "decompression must restore original text from compressed SQLite bodies"
    );
    let refs: Value = serde_json::from_str(block["direct_refs"].as_str().unwrap()).unwrap();
    engine.execute_tool(&preserved_session, restored.revision, "mixin_context_compress",
        &json!({"ranges":[{"start":refs[0]["reference"],"end":refs.as_array().unwrap().last().unwrap()["reference"]}],
            "topic":block["topic"],"summary":block["summary"]}), 100_000).unwrap();
    drop(engine);
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let checksum: Vec<u8> = connection
        .query_row(
            "SELECT body_checksum FROM messages WHERE reference = 'm000003'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute(
            "UPDATE messages SET body_checksum = X'00' WHERE reference = 'm000003'",
            [],
        )
        .unwrap();
    let corrupt = common::cli(database.path())
        .env("CODEX_MIXIN_CONTEXT_DB", &database_path)
        .args(["context", "export", "--session", &preserved_session])
        .output()
        .unwrap();
    assert!(
        !corrupt.status.success(),
        "corrupt stored body must not be presented as valid history"
    );
    assert!(String::from_utf8_lossy(&corrupt.stderr).contains("checksum mismatch"));
    connection
        .execute(
            "UPDATE messages SET body_checksum = ?1 WHERE reference = 'm000003'",
            [checksum],
        )
        .unwrap();
    drop(connection);

    // The test database is isolated. Tombstone only bodies covered by the
    // current compressed view to reproduce data lost by the old retention worker.
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let session_key: String = connection
        .query_row(
            "SELECT session_key FROM sessions ORDER BY updated_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let expired = connection
        .execute(
            "UPDATE messages SET item_json = '', expired_at = unixepoch()
             WHERE session_key = ?1 AND item_json <> ''
               AND EXISTS (
                   SELECT 1 FROM segments s JOIN blocks b
                     ON b.session_key = s.session_key AND b.block_id = s.reference
                   WHERE s.session_key = messages.session_key
                     AND s.revision = (SELECT revision FROM sessions WHERE session_key = ?1)
                     AND s.kind = 'block'
                     AND EXISTS (SELECT 1 FROM json_each(b.full_refs) r WHERE r.value = messages.reference)
               )
               AND NOT EXISTS (
                   SELECT 1 FROM segments s WHERE s.session_key = messages.session_key
                     AND s.revision = (SELECT revision FROM sessions WHERE session_key = ?1)
                     AND s.kind = 'message' AND s.reference = messages.reference
               )",
            [&session_key],
        )
        .unwrap();
    assert!(
        expired > 0,
        "the current compressed block should cover old bodies"
    );
    drop(connection);

    let metadata = common::cli(database.path())
        .env("CODEX_MIXIN_CONTEXT_DB", &database_path)
        .args(["context", "export", "--compact", "--session", &session_key])
        .output()
        .unwrap();
    assert!(
        metadata.status.success(),
        "console metadata must survive retention: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: Value = serde_json::from_slice(&metadata.stdout).unwrap();
    assert!(
        metadata["history"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["expired"] == true)
    );

    let mut chained = responses_request();
    chained["previous_response_id"] = json!(response_id);
    chained["input"] = json!([{
        "type":"message",
        "role":"user",
        "content":[{"type":"input_text","text":"question after expiration"}]
    }]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-retention")
        .json(&chained)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap();
    assert!(stream.contains("hello codex"), "missing answer: {stream}");

    let recorded = requests.lock().unwrap().clone();
    assert!(
        recorded.len() >= 3,
        "continuation did not reach upstream: {recorded:?}"
    );
    let continued = recorded.last().unwrap()["messages"].to_string();
    assert!(
        continued.contains("alpha and beta explored two ideas"),
        "{continued}"
    );
    assert!(
        continued.contains("question after expiration"),
        "{continued}"
    );
    assert!(
        !continued.contains("alpha word"),
        "expired raw body leaked: {continued}"
    );

    // Editing the second half of a compressed block restores its unchanged
    // prefix from the full replay, including a previously expired original.
    let mut forked = first;
    forked["input"][3]["content"][0]["text"] = json!("revised beta answer");
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-retention")
        .json(&forked)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = response.text().await.unwrap();
    assert!(
        stream.contains("hello codex"),
        "full replay failed: {stream}"
    );

    // Reproduce the persisted state left by the old fork path, including an
    // expired first user message used by the console's title fallback.
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    connection
        .execute(
            "UPDATE messages SET item_json = '', expired_at = unixepoch()
         WHERE session_key = ?1 AND reference IN ('m000002', 'm000003')",
            [&session_key],
        )
        .unwrap();
    drop(connection);
    let stats = common::cli(database.path())
        .env("CODEX_MIXIN_CONTEXT_DB", &database_path)
        .args(["context", "stats", "--json"])
        .output()
        .unwrap();
    assert!(
        stats.status.success(),
        "console statistics must survive expired originals: {}",
        String::from_utf8_lossy(&stats.stderr)
    );
    let stats: Value = serde_json::from_slice(&stats.stdout).unwrap();
    assert_eq!(stats["sessions"][0]["expired_messages"], 2);
    let status = common::cli(database.path())
        .env("CODEX_MIXIN_CONTEXT_DB", &database_path)
        .args(["context", "status", "--session", &session_key, "--json"])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "session status must survive expired originals: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["expired_messages"], 2);

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-context-retention")
        .json(&forked)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stream = response.text().await.unwrap();
    assert!(
        stream.contains("hello codex"),
        "unchanged replay failed: {stream}"
    );
}

async fn request_context_checkpoint(gateway_url: &str, session: &str) -> String {
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses/compact"))
        .bearer_auth("gateway-key")
        .header("x-session-id", session)
        .json(&json!({
            "model": "DeepSeek-V4-Flash-custom",
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"distinctive opening request"}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"the first answer"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"second question"}]}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["output"][0]["type"], "compaction");
    body["output"][0]["encrypted_content"]
        .as_str()
        .expect("checkpoint token")
        .to_owned()
}

async fn replay_context_checkpoint(
    gateway_url: &str,
    session: &str,
    token: &str,
) -> reqwest::Response {
    let mut follow_up = responses_request();
    follow_up["input"] = json!([
        {"type": "compaction", "encrypted_content": token},
        {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "next question"}]}
    ]);
    reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", session)
        .json(&follow_up)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn context_checkpoint_replaces_local_compaction() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextStatusThenText).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;
    let session = "thread-context-checkpoint";

    let token = request_context_checkpoint(&gateway_url, session).await;
    assert!(token.starts_with("codex-mixin:context:v2:"), "{token}");
    assert!(
        requests.lock().unwrap().is_empty(),
        "a local checkpoint must not call a model"
    );

    let response = replay_context_checkpoint(&gateway_url, session, &token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let stream = String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap();
    assert!(stream.contains("hello codex"), "missing answer: {stream}");

    let recorded = requests.lock().unwrap().clone();
    assert!(
        !recorded.is_empty(),
        "the follow-up must reach the provider"
    );
    let replay = Value::Array(
        recorded
            .iter()
            .map(|request| request["messages"].clone())
            .collect(),
    )
    .to_string();
    assert!(
        replay.contains("distinctive opening request"),
        "archived history was lost: {replay}"
    );
    assert!(replay.contains("second question"), "{replay}");
    assert!(replay.contains("next question"), "{replay}");
    assert!(
        !replay.contains("codex-mixin:context:v2:"),
        "a local checkpoint reached the provider: {replay}"
    );
}

#[tokio::test]
async fn context_rejects_a_checkpoint_from_another_session() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::ContextStatusThenText).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;

    let token = request_context_checkpoint(&gateway_url, "thread-context-owner").await;
    let response = replay_context_checkpoint(&gateway_url, "thread-context-other", &token).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.text().await.unwrap();
    assert!(body.contains("belongs to context session"), "{body}");
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn context_expands_a_checkpoint_after_the_feature_is_disabled() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url.clone());
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;
    let session = "thread-context-disabled";
    let token = request_context_checkpoint(&gateway_url, session).await;

    // An archived session stays readable after the feature is switched off.
    let mut config = test_config(upstream_url);
    config.context.enabled = false;
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;
    let response = replay_context_checkpoint(&gateway_url, session, &token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let stream = String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap();
    assert!(stream.contains("hello codex"), "missing answer: {stream}");

    let recorded = requests.lock().unwrap().clone();
    assert_eq!(
        recorded.len(),
        1,
        "the read-only path makes one round: {recorded:?}"
    );
    let replay = recorded[0].to_string();
    assert!(
        replay.contains("distinctive opening request"),
        "archived history was lost: {replay}"
    );
    assert!(replay.contains("next question"), "{replay}");
    assert!(
        !replay.contains("context_management"),
        "guidance leaked while the feature is off: {replay}"
    );
    assert!(
        !replay.contains("mixin_context_status"),
        "internal tools leaked while the feature is off: {replay}"
    );
}

#[tokio::test]
async fn context_official_route_keeps_the_upstream_status() {
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let (gateway_url, requests, _, _codex_home) = spawn_gateway_with_mock_official_context(
        OfficialWebSocketBehavior::RateLimitedHttp,
        Duration::from_secs(30),
        &database_path,
    )
    .await;
    let mut request = responses_request();
    request["model"] = json!("gpt-5.5-codex");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "body: {body}");
    assert!(body.contains("slow down"), "upstream body lost: {body}");
    assert_eq!(requests.lock().unwrap().len(), 1);
}

// Regression coverage for context pressure, persistence and transport boundaries.
#[tokio::test]
async fn context_growth_uses_a_baseline() {
    let (upstream, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream);
    config.context.enabled = true;
    config.context.growth_threshold_tokens = 500;
    config.context.protected_messages = 1;
    config.context.protected_tokens = 1;
    let db = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &db.path().join("context.db")).await;
    reqwest::Client::new().post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key").header("x-session-id", "review-growth")
        .json(&json!({"model":"DeepSeek-V4-Flash-custom", "stream":true,
            "input":[
                {"type":"message","role":"user","content":[{"type":"input_text","text":"opening"}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"word ".repeat(500)}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"word ".repeat(500)}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}
            ]})).send().await.unwrap().bytes().await.unwrap();
    let request = requests.lock().unwrap()[0].to_string();
    assert!(
        request.contains("keep working"),
        "fresh low-pressure history needs a growth baseline: {request}"
    );
    assert!(
        !request.contains("oldest unprotected range"),
        "fresh low-pressure history must not demand compression: {request}"
    );
}

#[tokio::test]
async fn context_model_window() {
    let (upstream, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream);
    config.context.enabled = true;
    config.providers[0].cached_models[0].context_window = Some(16_000);
    let db = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &db.path().join("context.db")).await;
    reqwest::Client::new()
        .post(format!("{gateway}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "review-window")
        .json(&responses_request())
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let request = requests.lock().unwrap()[0].to_string();
    assert!(
        request.contains("of 16000"),
        "model window ignored: {request}"
    );
}

#[tokio::test]
async fn context_official_ws() {
    let db = tempfile::tempdir().unwrap();
    let (gateway, requests, _, _auth) = spawn_gateway_with_mock_official_context(
        OfficialWebSocketBehavior::Persistent,
        Duration::from_secs(5),
        &db.path().join("context.db"),
    )
    .await;
    let ws = gateway.replacen("http://", "ws://", 1);
    let mut request = format!("{ws}/v1/responses").into_client_request().unwrap();
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    request
        .headers_mut()
        .insert("x-session-id", "review-ws".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let mut body = responses_request();
    body["model"] = json!("gpt-5.5-codex");
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
    .unwrap();
    let completed: Value = frames
        .iter()
        .map(|frame| serde_json::from_str::<Value>(frame).unwrap())
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    body["previous_response_id"] = completed["response"]["id"].clone();
    body["input"] = json!([{"type":"message","role":"user","content":[{"type":"input_text","text":"distinct WS continuation"}]}]);
    socket
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();
    let second_frames = tokio::time::timeout(
        Duration::from_secs(5),
        websocket_response_frames(&mut socket),
    )
    .await
    .unwrap();
    assert_eq!(
        requests.lock().unwrap().len(),
        2,
        "second WS response: {second_frames:?}"
    );
    let request = requests.lock().unwrap()[0].to_string();
    let continuation = requests.lock().unwrap()[1].to_string();
    assert!(
        continuation.contains("say hi"),
        "WS history lost: {continuation}"
    );
    assert_eq!(continuation.matches("distinct WS continuation").count(), 1);
    assert!(
        request.contains("mixin_context_compress"),
        "enabled engine bypassed by official WS: {request}"
    );
}

#[tokio::test]
async fn context_reader_keeps_lease() {
    use codex_mixin::config::ContextSettings;
    use codex_mixin::context::ContextEngine;
    let db = tempfile::tempdir().unwrap();
    let path = db.path().join("context.db");
    let settings = ContextSettings {
        enabled: true,
        ..Default::default()
    };
    let engine = Arc::new(ContextEngine::open(settings.clone(), &path).unwrap());
    let _lease = engine.acquire_lease("review-lease").await.unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(300),
            engine.acquire_lease("review-lease")
        )
        .await
        .is_err(),
        "a held session must not hand out a second lease"
    );
    let reader = ContextEngine::open(settings, &path).unwrap();
    reader.stats(1_000_000, true).unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(300),
            engine.acquire_lease("review-lease")
        )
        .await
        .is_err(),
        "opening the stats reader deleted a live turn lease"
    );
}

#[tokio::test]
async fn context_second_checkpoint() {
    let (upstream, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream);
    config.context.enabled = true;
    let db = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &db.path().join("context.db")).await;
    let session = "review-checkpoint";
    let token = request_context_checkpoint(&gateway, session).await;
    let response = reqwest::Client::new().post(format!("{gateway}/v1/responses/compact"))
        .bearer_auth("gateway-key").header("x-session-id", session)
        .json(&json!({"model":"DeepSeek-V4-Flash-custom", "input":[
            {"type":"compaction","encrypted_content":token},
            {"type":"message","role":"user","content":[{"type":"input_text","text":"after first checkpoint"}]}
        ]})).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let second: Value = response.json().await.unwrap();
    let response = replay_context_checkpoint(
        &gateway,
        session,
        second["output"][0]["encrypted_content"].as_str().unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    let recorded = requests.lock().unwrap()[0].to_string();
    assert!(
        recorded.contains("distinctive opening request"),
        "second compact lost original history: {recorded}"
    );
}

#[tokio::test]
async fn context_stream_before_done() {
    for enabled in [false, true] {
        let gate = Arc::new(tokio::sync::Notify::new());
        let sender_gate = gate.clone();
        let app = Router::new().route("/v1/responses", post(move || {
            let gate = sender_gate.clone();
            async move {
                let stream = async_stream::stream! {
                    yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"review-stream\"}}\n\n"));
                    yield Ok(bytes::Bytes::from_static(b"event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_progress\"}}\n\nevent: response.reasoning_summary_text.delta\ndata: {\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"item_id\":\"rs_progress\",\"delta\":\"Working on the request\"}\n\nevent: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"fc_progress\",\"call_id\":\"call_progress\",\"name\":\"shell\",\"arguments\":\"\"}}\n\nevent: response.function_call_arguments.delta\ndata: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"item_id\":\"fc_progress\",\"delta\":\"{}\"}\n\n"));
                    gate.notified().await;
                    yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"review-stream\",\"status\":\"completed\",\"output\":[]}}\n\n"));
                };
                Response::builder().header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(stream)).unwrap()
            }
        }));
        let upstream = spawn_router(app).await;
        let mut config = test_config(upstream);
        config.context.enabled = enabled;
        config.providers[0].protocol = ProviderProtocol::OpenAiResponses;
        config.providers[0].api_path = "/v1/responses".to_owned();
        let db = tempfile::tempdir().unwrap();
        let gateway = spawn_gateway_with_context(config, &db.path().join("context.db")).await;
        let first_chunk = tokio::time::timeout(Duration::from_secs(10), async {
            let response = reqwest::Client::new()
                .post(format!("{gateway}/v1/responses"))
                .bearer_auth("gateway-key")
                .header("x-session-id", "review-stream")
                .json(&responses_request())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut stream = response.bytes_stream();
            let mut progress = Vec::new();
            while !String::from_utf8_lossy(&progress)
                .contains("response.function_call_arguments.delta")
            {
                progress.extend_from_slice(&stream.next().await.unwrap().unwrap());
            }
            assert!(String::from_utf8_lossy(&progress).contains("Working on the request"));
            assert!(!String::from_utf8_lossy(&progress).contains("response.completed"));
            progress
        })
        .await;
        assert!(
            first_chunk.is_ok(),
            "context enabled={enabled}: no SSE delivered before upstream completes"
        );
        if enabled {
            let engine = Arc::new(
                codex_mixin::context::ContextEngine::open(
                    codex_mixin::config::ContextSettings {
                        enabled: true,
                        ..Default::default()
                    },
                    &db.path().join("context.db"),
                )
                .unwrap(),
            );
            let sessions = engine.sessions().unwrap();
            assert_eq!(sessions.len(), 1);
            let session = &sessions[0].session;
            let released = tokio::time::timeout(Duration::from_secs(5), async {
                let lease = engine.acquire_lease(session).await.unwrap();
                drop(lease);
            })
            .await;
            gate.notify_one();
            assert!(
                released.is_ok(),
                "client disconnect must release the context turn before upstream completes"
            );
        } else {
            gate.notify_one();
        }
    }
}

#[tokio::test]
async fn context_websocket_close_releases_lease_before_upstream_finishes() {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let sender_gate = gate.clone();
    let app = Router::new().route("/v1/responses", post(move || {
        let gate = sender_gate.clone();
        async move {
            let stream = async_stream::stream! {
                yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"review-ws-cancel\"}}\n\n"));
                gate.acquire().await.unwrap().forget();
                yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"review-ws-cancel\",\"status\":\"completed\",\"output\":[]}}\n\n"));
            };
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(stream))
                .unwrap()
        }
    }));
    let upstream = spawn_router(app).await;
    let mut config = test_config(upstream);
    config.context.enabled = true;
    config.providers[0].protocol = ProviderProtocol::OpenAiResponses;
    config.providers[0].api_path = "/v1/responses".to_owned();
    let database = tempfile::tempdir().unwrap();
    let gateway = spawn_gateway_with_context(config, &database.path().join("context.db")).await;
    let websocket_url = gateway.replacen("http://", "ws://", 1);

    let connect = || {
        let mut request = format!("{websocket_url}/v1/responses")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
        request
            .headers_mut()
            .insert("x-session-id", "thread-ws-stop-resume".parse().unwrap());
        request
    };

    let (mut stopped, _) = connect_async(connect()).await.unwrap();
    let mut body = responses_request();
    body["type"] = json!("response.create");
    stopped
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();
    let progress = tokio::time::timeout(Duration::from_secs(5), stopped.next())
        .await
        .expect("first WS turn did not emit progress")
        .unwrap()
        .unwrap();
    assert!(matches!(progress, WsMessage::Text(_)));
    drop(stopped);

    let (mut resumed, _) = connect_async(connect()).await.unwrap();
    resumed
        .send(WsMessage::Text(body.to_string().into()))
        .await
        .unwrap();
    let progress = tokio::time::timeout(Duration::from_secs(2), resumed.next()).await;
    let resumed_before_upstream_finishes = matches!(
        progress,
        Ok(Some(Ok(WsMessage::Text(text)))) if text.contains("response.created")
    );

    // Release both upstream rounds after measuring cancellation. This also lets
    // the red case unwind cleanly when the first WS handler missed its Close.
    gate.add_permits(8);
    let completion = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(message) = resumed.next().await {
            let message = message.unwrap();
            if let WsMessage::Text(text) = message
                && text.contains("response.completed")
            {
                break;
            }
        }
    })
    .await;
    assert!(
        completion.is_ok(),
        "resumed WS turn did not complete after releasing upstream"
    );
    assert!(
        resumed_before_upstream_finishes,
        "closing the first WS must release its session lease before the upstream completes"
    );
}

#[tokio::test]
async fn context_recompact_validates() {
    let (upstream, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream);
    config.context.enabled = true;
    let db = tempfile::tempdir().unwrap();
    let path = db.path().join("context.db");
    let gateway = spawn_gateway_with_context(config, &path).await;
    let token = request_context_checkpoint(&gateway, "checkpoint-owner").await;
    for (session, token) in [
        ("another-session", token),
        (
            "checkpoint-owner",
            "codex-mixin:context:v2:corrupted".to_owned(),
        ),
    ] {
        let response = reqwest::Client::new().post(format!("{gateway}/v1/responses/compact"))
            .bearer_auth("gateway-key").header("x-session-id", session)
            .json(&json!({"model":"DeepSeek-V4-Flash-custom", "input":[
                {"type":"compaction","encrypted_content":token},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"must not replace history"}]}
            ]})).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert!(requests.lock().unwrap().is_empty());
    let connection = rusqlite::Connection::open(&path).unwrap();
    let history: String = connection
        .query_row("SELECT group_concat(item_json) FROM messages", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(!history.contains("must not replace history"));
}

/// Two turns of one session used to refuse the second one with "context
/// session is busy with another turn". The arriving turn waits for the live
/// holder now, so both finish.
#[tokio::test]
async fn concurrent_turns_of_one_session_wait_instead_of_failing() {
    let (upstream_url, _requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;

    let send = |index: usize| {
        let gateway_url = gateway_url.clone();
        async move {
            let mut body = responses_request();
            body["input"][1]["content"][0]["text"] = json!(format!("say hi {index}"));
            reqwest::Client::new()
                .post(format!("{gateway_url}/v1/responses"))
                .bearer_auth("gateway-key")
                .header("x-session-id", "thread-concurrent-turns")
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };
    let (first, second) = tokio::join!(send(0), send(1));
    assert_eq!(first.status(), StatusCode::OK, "first turn");
    assert_eq!(
        second.status(),
        StatusCode::OK,
        "the second turn waits for the lease instead of failing"
    );
    for response in [first, second] {
        let stream = String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap();
        assert!(stream.contains("hello codex"), "missing answer: {stream}");
    }
}

/// A session-title style request carries its own instructions text and a fresh
/// message line. It must land on its own session so it cannot take the main
/// turn's lease or rewrite the archived view.
#[tokio::test]
async fn auxiliary_persona_forks_off_the_main_session() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.context.enabled = true;
    let database = tempfile::tempdir().unwrap();
    let database_path = database.path().join("context.db");
    let gateway_url = spawn_gateway_with_context(config, &database_path).await;
    let client = reqwest::Client::new();

    let main = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-persona")
        .json(&responses_request())
        .send()
        .await
        .unwrap();
    assert_eq!(main.status(), StatusCode::OK);
    let _ = main.bytes().await.unwrap();

    let title_body = json!({
        "model": "DeepSeek-V4-Flash-custom",
        "stream": true,
        "instructions": "Create a concise title for a session.",
        "input": [
            {"type":"message","role":"user","content":[{"type":"input_text","text":"Name the session"}]}
        ]
    });
    let title = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("x-session-id", "thread-persona")
        .json(&title_body)
        .send()
        .await
        .unwrap();
    assert_eq!(title.status(), StatusCode::OK);
    let _ = title.bytes().await.unwrap();

    let recorded = requests.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "round count: {recorded:?}");
    let title_view = recorded[1]["messages"].to_string();
    assert!(title_view.contains("Name the session"), "{title_view}");
    assert!(
        !title_view.contains("dev rules"),
        "the auxiliary request inherited the main view: {title_view}"
    );

    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let keys: String = connection
        .query_row(
            "SELECT group_concat(session_key, ' ') FROM sessions",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        keys.contains("sub:"),
        "the auxiliary request needs its own session: {keys}"
    );
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2, "main and forked sessions: {keys}");
}
