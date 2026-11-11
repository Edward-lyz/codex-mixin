use super::*;

#[tokio::test]
async fn replays_tool_images_as_markers_and_forwards_only_the_fresh_one() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let old_image = png_data_url(2000, 1000);
    let latest_image = png_data_url(2000, 1000);
    let mut request = responses_request();
    request["input"].as_array_mut().unwrap().extend([
        json!({
            "type": "function_call",
            "call_id": "call_old_image",
            "name": "view_image",
            "arguments": "{}"
        }),
        json!({
            "type": "function_call_output",
            "call_id": "call_old_image",
            "output": [
                {"type":"input_text","text":"old screenshot"},
                {"type":"input_image","image_url":old_image}
            ]
        }),
        json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type":"output_text","text":"looked at it"}]
        }),
        json!({
            "type": "function_call",
            "call_id": "call_latest_image",
            "name": "view_image",
            "arguments": "{}"
        }),
        json!({
            "type": "function_call_output",
            "call_id": "call_latest_image",
            "output": [
                {"type":"input_text","text":"latest screenshot"},
                {"type":"input_image","image_url":latest_image}
            ]
        }),
    ]);
    assert!(request.to_string().contains("data:image/png;base64,"));

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
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

    let upstream_request = requests.lock().unwrap()[0].clone();
    let tool_results = upstream_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|block| block["type"] == "tool_result")
        .collect::<Vec<_>>();
    assert_eq!(tool_results.len(), 2);
    // The replayed result must stay byte-stable across turns, so its image is
    // gone and replaced by a marker the model can see.
    assert_eq!(
        tool_results[0]["content"],
        json!([
            {"type":"text","text":"old screenshot"},
            {"type":"text","text":TOOL_IMAGE_PLACEHOLDER}
        ])
    );
    // The fresh result is new in this request, so inlining it costs no cached
    // prefix and the model can actually look at the screenshot.
    assert_eq!(
        tool_results[1]["content"][0],
        json!({"type":"text","text":"latest screenshot"})
    );
    let source = &tool_results[1]["content"][1]["source"];
    assert_eq!(source["type"], "base64");
    assert_eq!(source["media_type"], "image/jpeg");
    assert!(
        base64_png_dimensions(source["data"].as_str().unwrap()) == (1568, 784),
        "fresh tool image was not downscaled to the vision budget"
    );
    assert!(
        !upstream_request
            .to_string()
            .contains("older tool image omitted")
    );
}

#[tokio::test]
async fn accepts_a_single_image_request_larger_than_sixteen_mib() {
    let received_bytes = Arc::new(AtomicUsize::new(0));
    let received_bytes_for_route = received_bytes.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        post(move |body: Body| {
            let received_bytes = received_bytes_for_route.clone();
            async move {
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                received_bytes.store(body.len(), Ordering::SeqCst);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(text_sse()))
                    .unwrap()
            }
        }),
    );
    let upstream_url = spawn_router(upstream).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let mut request = responses_request();
    let image_bytes = vec![0u8; 13 * 1024 * 1024];
    let image_url = format!(
        "data:image/unknown;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(image_bytes)
    );
    request["input"].as_array_mut().unwrap().push(json!({
        "type":"message",
        "role":"user",
        "content":[{"type":"input_image","image_url":image_url}]
    }));
    assert!(request.to_string().len() > 16 * 1024 * 1024);

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(received_bytes.load(Ordering::SeqCst) > 16 * 1024 * 1024);
}

#[tokio::test]
async fn accepts_a_websocket_request_larger_than_sixteen_mib() {
    let received_bytes = Arc::new(AtomicUsize::new(0));
    let received_bytes_for_route = received_bytes.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        post(move |body: Body| {
            let received_bytes = received_bytes_for_route.clone();
            async move {
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                received_bytes.store(body.len(), Ordering::SeqCst);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(text_sse()))
                    .unwrap()
            }
        }),
    );
    let upstream_url = spawn_router(upstream).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let websocket_url = gateway_url.replacen("http://", "ws://", 1);
    let mut websocket_request = format!("{websocket_url}/v1/responses")
        .into_client_request()
        .unwrap();
    websocket_request
        .headers_mut()
        .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
    let (mut socket, _) = connect_async(websocket_request).await.unwrap();
    let mut body = responses_request();
    body["type"] = json!("response.create");
    body["input"].as_array_mut().unwrap().push(json!({
        "type":"message",
        "role":"user",
        "content":[{
            "type":"input_text",
            "text":"x".repeat(16 * 1024 * 1024)
        }]
    }));
    let body = body.to_string();
    assert!(body.len() > 16 * 1024 * 1024);

    socket.send(WsMessage::Text(body.into())).await.unwrap();

    let frames = websocket_response_frames(&mut socket).await.join("\n");
    assert!(frames.contains("\"type\":\"response.completed\""));
    assert!(received_bytes.load(Ordering::SeqCst) > 16 * 1024 * 1024);
}

#[tokio::test]
async fn retries_one_provider_413_with_the_payload_fallback_profile() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let attempts_for_route = attempts.clone();
    let requests_for_route = requests.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        post(move |body: Body| {
            let attempts = attempts_for_route.clone();
            let requests = requests_for_route.clone();
            async move {
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                requests
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice::<Value>(&body).unwrap());
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Response::builder()
                        .status(StatusCode::PAYLOAD_TOO_LARGE)
                        .body(Body::from("request too large"))
                        .unwrap();
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(text_sse()))
                    .unwrap()
            }
        }),
    );
    let upstream_url = spawn_router(upstream).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let mut request = responses_request();
    request["input"].as_array_mut().unwrap().push(json!({
        "type":"message",
        "role":"user",
        "content":[{"type":"input_image","image_url":png_data_url(3000, 1500)}]
    }));

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let recorded = requests.lock().unwrap();
    let source = recorded[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .find(|block| block["type"] == "image")
        .unwrap()
        .get("source")
        .unwrap();
    assert_eq!(source["media_type"], "image/jpeg");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(source["data"].as_str().unwrap())
        .unwrap();
    let image = image::load_from_memory(&decoded).unwrap();
    assert_eq!((image.width(), image.height()), (768, 384));
}

async fn spawn_failover_upstream(primary_status: StatusCode) -> (String, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_for_route = requests.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        post(move |body: Body| {
            let requests = requests_for_route.clone();
            async move {
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let body = serde_json::from_slice::<Value>(&body).unwrap();
                let primary = body["model"] == "DeepSeek-V4-Flash";
                requests.lock().unwrap().push(body);
                if primary {
                    return Response::builder()
                        .status(primary_status)
                        .body(Body::from("primary unavailable"))
                        .unwrap();
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(text_sse()))
                    .unwrap()
            }
        }),
    );
    (spawn_router(upstream).await, requests)
}

fn failover_config(upstream_url: String) -> GatewayConfig {
    let mut config = test_config(upstream_url);
    config.failover_routes = vec![FailoverRoute {
        source: "DeepSeek-V4-Flash-custom".to_owned(),
        fallbacks: vec!["panel-a-custom".to_owned()],
        ..FailoverRoute::default()
    }];
    config
}

#[tokio::test]
async fn fails_over_to_the_next_model_after_an_upstream_server_error() {
    let (upstream_url, requests) = spawn_failover_upstream(StatusCode::SERVICE_UNAVAILABLE).await;
    let gateway_url = spawn_gateway_with_config(failover_config(upstream_url)).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&responses_request())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("failover_status"));
    assert!(body.contains("answering with panel-a-custom"));
    assert!(body.contains("response.completed"));
    let recorded = requests.lock().unwrap();
    let models: Vec<_> = recorded
        .iter()
        .map(|request| request["model"].clone())
        .collect();
    assert_eq!(models, vec![json!("DeepSeek-V4-Flash"), json!("panel-a")]);
}

#[tokio::test]
async fn fails_over_when_the_stream_reports_an_error_before_output() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_for_route = requests.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        post(move |body: Body| {
            let requests = requests_for_route.clone();
            async move {
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let body = serde_json::from_slice::<Value>(&body).unwrap();
                let primary = body["model"] == "DeepSeek-V4-Flash";
                requests.lock().unwrap().push(body);
                let sse = if primary {
                    "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"usage limit reached\"}}\n\n".to_owned()
                } else {
                    text_sse()
                };
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(sse))
                    .unwrap()
            }
        }),
    );
    let upstream_url = spawn_router(upstream).await;
    let gateway_url = spawn_gateway_with_config(failover_config(upstream_url)).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&responses_request())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("answering with panel-a-custom"));
    assert!(body.contains("response.completed"));
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn returns_the_second_provider_413_without_a_third_attempt() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_route = attempts.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        post(move |body: Body| {
            let attempts = attempts_for_route.clone();
            async move {
                let _ = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                attempts.fetch_add(1, Ordering::SeqCst);
                Response::builder()
                    .status(StatusCode::PAYLOAD_TOO_LARGE)
                    .body(Body::from("still too large"))
                    .unwrap()
            }
        }),
    );
    let upstream_url = spawn_router(upstream).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let mut request = responses_request();
    request["input"].as_array_mut().unwrap().push(json!({
        "type":"message",
        "role":"user",
        "content":[{"type":"input_image","image_url":png_data_url(3000, 1500)}]
    }));

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(response.text().await.unwrap(), "still too large");
}

#[tokio::test]
async fn does_not_retry_a_provider_413_without_embedded_images() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_route = attempts.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        post(move |body: Body| {
            let attempts = attempts_for_route.clone();
            async move {
                let _ = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                attempts.fetch_add(1, Ordering::SeqCst);
                Response::builder()
                    .status(StatusCode::PAYLOAD_TOO_LARGE)
                    .body(Body::from("text request too large"))
                    .unwrap()
            }
        }),
    );
    let upstream_url = spawn_router(upstream).await;
    let gateway_url = spawn_gateway(upstream_url).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&responses_request())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retries_native_anthropic_messages_after_a_provider_413() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let attempts_for_route = attempts.clone();
    let requests_for_route = requests.clone();
    let upstream = Router::new().route(
        "/v1/messages",
        post(move |body: Body| {
            let attempts = attempts_for_route.clone();
            let requests = requests_for_route.clone();
            async move {
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                requests
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice::<Value>(&body).unwrap());
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Response::builder()
                        .status(StatusCode::PAYLOAD_TOO_LARGE)
                        .body(Body::from("request too large"))
                        .unwrap();
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(text_sse()))
                    .unwrap()
            }
        }),
    );
    let upstream_url = spawn_router(upstream).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let image_url = png_data_url(3000, 1500);
    let image_data = image_url.split_once(";base64,").unwrap().1;
    let request = json!({
        "model":"DeepSeek-V4-Flash-custom",
        "max_tokens":1024,
        "stream":true,
        "messages":[{
            "role":"user",
            "content":[{
                "type":"image",
                "source":{"type":"base64","media_type":"image/png","data":image_data}
            }]
        }]
    });

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let recorded = requests.lock().unwrap();
    let source = &recorded[1]["messages"][0]["content"][0]["source"];
    assert_eq!(source["media_type"], "image/jpeg");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(source["data"].as_str().unwrap())
        .unwrap();
    let image = image::load_from_memory(&decoded).unwrap();
    assert_eq!((image.width(), image.height()), (768, 384));
}

#[tokio::test]
async fn retries_official_responses_after_a_413() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let attempts_for_route = attempts.clone();
    let requests_for_route = requests.clone();
    let official = Router::new().route(
        "/backend-api/codex/responses",
        post(move |body: Body| {
            let attempts = attempts_for_route.clone();
            let requests = requests_for_route.clone();
            async move {
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                requests
                    .lock()
                    .unwrap()
                    .push(serde_json::from_slice::<Value>(&body).unwrap());
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Response::builder()
                        .status(StatusCode::PAYLOAD_TOO_LARGE)
                        .body(Body::from("official request too large"))
                        .unwrap();
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(
                        "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_official\",\"status\":\"completed\",\"model\":\"gpt-5.5\",\"output\":[]}}\n\n",
                    ))
                    .unwrap()
            }
        }),
    );
    let official_url = spawn_router(official).await;
    let (custom_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(custom_url);
    config.official_responses_url = format!("{official_url}/backend-api/codex/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("gpt-5.5");
    request["input"].as_array_mut().unwrap().push(json!({
        "type":"message",
        "role":"user",
        "content":[{"type":"input_image","image_url":png_data_url(3000, 1500)}]
    }));

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let recorded = requests.lock().unwrap();
    let image_url = recorded[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .find(|part| part["type"] == "input_image")
        .unwrap()["image_url"]
        .as_str()
        .unwrap();
    let (_, encoded) = image_url.split_once(";base64,").unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    let image = image::load_from_memory(&decoded).unwrap();
    assert_eq!((image.width(), image.height()), (768, 384));
}

#[tokio::test]
async fn replays_tool_images_as_markers_on_the_openai_chat_path() {
    let (upstream_url, requests) = spawn_mock_openai_chat().await;
    let mut config = test_config(upstream_url);
    configure_openai_chat(&mut config, "/chat/completions");
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();
    let image = png_data_url(2000, 1000);
    let mut request = responses_request();
    request["input"].as_array_mut().unwrap().extend([
        json!({
            "type": "function_call",
            "call_id": "call_old_image",
            "name": "view_image",
            "arguments": "{}"
        }),
        json!({
            "type": "function_call_output",
            "call_id": "call_old_image",
            "output": [
                {"type":"input_text","text":"old screenshot"},
                {"type":"input_image","image_url":image}
            ]
        }),
        json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type":"output_text","text":"looked at it"}]
        }),
        json!({
            "type": "function_call",
            "call_id": "call_latest_image",
            "name": "view_image",
            "arguments": "{}"
        }),
        json!({
            "type": "function_call_output",
            "call_id": "call_latest_image",
            "output": [
                {"type":"input_text","text":"latest screenshot"},
                {"type":"input_image","image_url":image}
            ]
        }),
    ]);

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.unwrap();

    let upstream_request = requests.lock().unwrap()[0].clone();
    let messages = upstream_request["messages"].as_array().unwrap();
    let tool_indexes = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message["role"] == "tool")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(tool_indexes.len(), 2);
    // Chat Completions rejects images inside `tool` messages, so every tool
    // result must be plain text on this path.
    assert!(
        tool_indexes
            .iter()
            .all(|&index| messages[index]["content"].is_string()),
        "a tool message still carries structured content: {messages:?}"
    );

    let replayed = messages[tool_indexes[0]]["content"].as_str().unwrap();
    assert!(
        replayed.contains("old screenshot"),
        "tool text was dropped: {replayed}"
    );
    assert!(
        replayed.contains(TOOL_IMAGE_PLACEHOLDER),
        "omitted image was not marked: {replayed}"
    );

    let fresh_index = tool_indexes[1];
    let fresh = messages[fresh_index]["content"].as_str().unwrap();
    assert!(
        fresh.contains("latest screenshot"),
        "fresh tool text was dropped: {fresh}"
    );
    // The fresh image is handed over as the user message right after the tool run.
    let relocated = &messages[fresh_index + 1];
    assert_eq!(relocated["role"], "user");
    let parts = relocated["content"].as_array().unwrap();
    assert_eq!(parts[1]["type"], "image_url");
    assert_eq!(
        data_url_dimensions(parts[1]["image_url"]["url"].as_str().unwrap()),
        (1568, 784)
    );
}
