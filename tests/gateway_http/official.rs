use super::*;

#[tokio::test]
async fn forwards_thinking_and_anthropic_server_web_search() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::WebSearchRetry).await;
    let mut config = test_config(upstream_url);
    config.thinking_mode = ThinkingMode::Auto;
    config.enable_web_search_tool = true;
    config.web_search_max_uses = Some(1);
    let capability_dir = tempfile::tempdir().unwrap();
    let capabilities = WebSearchCapabilities::load(
        capability_dir.path().join("web-search-capabilities.json"),
        &config,
    )
    .unwrap();
    let mut models = vec![ModelInfo {
        id: "Claude Sonnet 5-custom".to_owned(),
        ..ModelInfo::default()
    }];
    let registry = ProviderRegistry::new(config.providers.clone()).unwrap();
    capabilities
        .probe_models(&mut models, &config, &registry, true)
        .await
        .unwrap();
    let probe_requests = requests.lock().unwrap().clone();
    assert_eq!(probe_requests.len(), 1);
    assert_eq!(probe_requests[0]["tools"].as_array().unwrap().len(), 2);
    assert_eq!(probe_requests[0]["tool_choice"]["type"], "tool");
    assert_eq!(probe_requests[0]["tool_choice"]["name"], "web_search");
    assert_eq!(probe_requests[0]["tools"][0]["name"], "web_search");
    assert_eq!(
        probe_requests[0]["tools"][1]["name"],
        "codex_mixin_probe_noop"
    );
    requests.lock().unwrap().clear();
    let state = AppState::with_web_search_capabilities(config, capabilities).unwrap();
    let gateway_url = spawn_router(router(state)).await;
    let client = reqwest::Client::new();
    let mut request = responses_request();
    request["model"] = json!("Claude Sonnet 5-custom");
    request["reasoning"] = json!({"effort": "xhigh"});
    request["input"] = json!([
        {"type":"message","role":"developer","content":[{"type":"input_text","text":"dev rules"}]},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"Teach me KDA attention"}]},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"KDA keeps a recurrent matrix state."}]},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"Continue polishing the lesson"}]}
    ]);
    request["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"web_search","external_web_access":true}));

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("response.completed"));
    assert!(body.contains("\"type\":\"web_search_call\""));
    assert!(body.contains("\"query\":\"OpenAI Codex\""));

    let supported_requests = requests.lock().unwrap().clone();
    assert_eq!(supported_requests.len(), 2);
    assert_ne!(supported_requests[0]["tool_choice"]["type"], "tool");
    assert_eq!(supported_requests[1]["tool_choice"]["type"], "tool");
    assert_eq!(supported_requests[1]["tool_choice"]["name"], "web_search");
    let upstream_request = supported_requests[1].clone();
    assert_eq!(upstream_request["thinking"]["type"], "adaptive");
    assert_eq!(upstream_request["output_config"]["effort"], "max");
    assert_eq!(upstream_request["system"].as_array().unwrap().len(), 2);
    assert_eq!(upstream_request["messages"].as_array().unwrap().len(), 3);
    assert_eq!(
        upstream_request["messages"][0]["content"][0]["text"],
        "Teach me KDA attention"
    );
    assert_eq!(
        upstream_request["messages"][2]["content"][0]["text"],
        "Continue polishing the lesson"
    );
    assert_eq!(upstream_request["tools"].as_array().unwrap().len(), 2);
    assert_eq!(upstream_request["tools"][0]["name"], "exec_command");
    assert_eq!(upstream_request["tools"][1]["type"], "web_search_20250305");
    assert_eq!(upstream_request["tools"][1]["name"], "web_search");
    assert_eq!(upstream_request["tools"][1]["max_uses"], 1);
    assert!(upstream_request["tools"][1].get("input_schema").is_none());

    let mut unsupported_request = responses_request();
    unsupported_request["model"] = json!("DeepSeek-V4-Flash-custom");
    unsupported_request["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"web_search","external_web_access":true}));
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&unsupported_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.text().await.unwrap();

    let upstream_request = requests.lock().unwrap()[2].clone();
    assert_eq!(upstream_request["tools"].as_array().unwrap().len(), 1);
    assert_ne!(upstream_request["tools"][0]["name"], "web_search");
}

#[tokio::test]
async fn proxies_codex_image_generation_to_official_backend() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, auth_headers, account_headers, _, _) =
        spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let mut config = test_config(upstream_url);
    config.providers[0].image_generation_path = Some("/v1/images/generations".to_owned());
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let request = json!({
        "prompt": "draw the Codex logo",
        "model": "gpt-image-2",
        "background": "auto",
        "quality": "auto",
        "size": "auto"
    });

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .bearer_auth("gateway-key")
        .header("x-codex-originator", "codex_cli_rs")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"][0]["b64_json"], "b2ZmaWNpYWwtaW1hZ2U=");
    assert!(upstream_requests.lock().unwrap().is_empty());
    assert_eq!(official_requests.lock().unwrap().as_slice(), &[request]);
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
async fn proxies_codex_image_edits_to_official_backend() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
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
    let request = json!({
        "images": [{"image_url":"data:image/png;base64,aW1hZ2U="}],
        "prompt": "add a blue border",
        "model": "gpt-image-2",
        "background": "auto",
        "quality": "auto",
        "size": "auto"
    });

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/edits"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(official_requests.lock().unwrap().as_slice(), &[request]);
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
async fn routes_auto_review_to_official_http_backend() {
    let (gateway_url, official_requests, _, _codex_home) = spawn_gateway_with_mock_official(
        OfficialWebSocketBehavior::Persistent,
        Duration::from_secs(2),
    )
    .await;
    let mut request = responses_request();
    request["model"] = json!("codex-auto-review");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(official_requests.lock().unwrap().as_slice(), &[request]);
}

#[tokio::test]
async fn codex_thread_title_mapping_routes_http_and_websocket_without_official_quota() {
    for (websocket, provided_schema, string_input) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, false),
        (false, false, true),
    ] {
        let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
        let (official_url, official_requests, _, _, _, _) =
            spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
        let mut config = test_config(upstream_url);
        config.official_responses_url = format!("{official_url}/v1/responses");
        config.providers[0].codex_thread_title_model = Some("DeepSeek-V4-Flash".to_owned());
        config.enable_web_search_tool = true;
        let codex_home = tempfile::tempdir().unwrap();
        config.codex_auth_path = codex_home.path().join("auth.json");
        std::fs::write(
            &config.codex_auth_path,
            r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
        )
        .unwrap();
        let gateway_url = spawn_gateway_with_config(config).await;
        let mut body = responses_request();
        body["model"] = json!("gpt-6-luna");
        body["client_metadata"] = json!({"x-codex-turn-metadata":json!({"thread_source":"thread_title","turn_trigger":"thread_title"}).to_string()});
        body["tool_choice"] = json!("required");
        body["text"] = json!({"verbosity":"low"});
        body["input"].as_array_mut().unwrap().extend([
            json!({"type":"configuration_update","text":{"format":{"type":"text"}}}),
            json!({"type":"additional_tools","tools":[{"type":"function","name":"inherited_exec","parameters":{"type":"object"}}]}),
            json!({"type":"tool_search_output","execution":"client","call_id":"search-1","tools":[{"type":"function","name":"discovered_exec","parameters":{"type":"object"}}]}),
            json!({"type":"message","role":"user","content":[{"type":"input_text","text":"Output the title as plain text with no surrounding quotes or backticks."}]}),
        ]);
        if provided_schema {
            body["input"][2]["text"]["format"] = json!({
                "type":"json_schema", "name":"client_title", "strict":true,
                "schema":{
                    "type":"object",
                    "properties":{
                        "title":{"type":"string","minLength":1,"maxLength":36},
                        "description":{"type":"string","minLength":1}
                    },
                    "required":["title","description"],
                    "additionalProperties":false
                }
            });
        }
        if string_input {
            body["input"] = json!("Output the title as plain text.");
        }
        let response = if websocket {
            let mut handshake = format!(
                "{}/v1/responses",
                gateway_url.replacen("http://", "ws://", 1)
            )
            .into_client_request()
            .unwrap();
            handshake
                .headers_mut()
                .insert(header::AUTHORIZATION, "Bearer gateway-key".parse().unwrap());
            let (mut socket, _) = connect_async(handshake).await.unwrap();
            body["type"] = json!("response.create");
            socket
                .send(WsMessage::Text(body.to_string().into()))
                .await
                .unwrap();
            websocket_response_frames(&mut socket).await.join("\n")
        } else {
            reqwest::Client::new()
                .post(format!("{gateway_url}/v1/responses"))
                .bearer_auth("gateway-key")
                .json(&body)
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        };
        assert!(response.contains("response.completed"), "{response}");
        assert!(
            response.contains("gpt-6-luna"),
            "client-facing model must be preserved: {response}"
        );
        assert_eq!(upstream_requests.lock().unwrap().len(), 1);
        assert_eq!(
            upstream_requests.lock().unwrap()[0]["model"],
            "DeepSeek-V4-Flash"
        );
        let upstream = upstream_requests.lock().unwrap()[0].clone();
        assert!(upstream.get("tools").is_none(), "{upstream}");
        assert!(upstream.get("tool_choice").is_none(), "{upstream}");
        assert_eq!(upstream["output_config"]["format"]["type"], "json_schema");
        let schema = &upstream["output_config"]["format"]["schema"];
        assert_eq!(schema["required"], json!(["title", "description"]));
        assert_eq!(
            schema["properties"]["title"]["maxLength"],
            if provided_schema { 36 } else { 60 }
        );
        assert_eq!(schema["additionalProperties"], false);
        let title_instructions = upstream["system"].as_array().unwrap().last().unwrap()["text"]
            .as_str()
            .unwrap();
        assert!(title_instructions.contains("ONLY a valid JSON object"));
        assert!(title_instructions.contains("Do not call any tools"));
        assert!(title_instructions.contains(&format!(
            "\"maxLength\":{}",
            if provided_schema { 36 } else { 60 }
        )));
        assert!(
            official_requests.lock().unwrap().is_empty(),
            "mapped title must never call official inference"
        );

        // The identical model slug remains official without the title marker.
        body.as_object_mut().unwrap().remove("client_metadata");
        body.as_object_mut().unwrap().remove("type");
        let response = reqwest::Client::new()
            .post(format!("{gateway_url}/v1/responses"))
            .bearer_auth("gateway-key")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.bytes().await.unwrap();
        assert_eq!(official_requests.lock().unwrap().len(), 1);
        assert_eq!(upstream_requests.lock().unwrap().len(), 1);
        let official = official_requests.lock().unwrap()[0].clone();
        assert_eq!(official["tools"], body["tools"]);
        assert_eq!(official["input"], body["input"]);
        assert_eq!(official["text"], body["text"]);
    }
}

#[tokio::test]
async fn omits_unsupported_output_limit_from_official_responses() {
    let (gateway_url, official_requests, _, _codex_home) = spawn_gateway_with_mock_official(
        OfficialWebSocketBehavior::Persistent,
        Duration::from_secs(2),
    )
    .await;
    let mut request = responses_request();
    request["model"] = json!("codex-auto-review");
    request["max_output_tokens"] = json!(8192);

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let official_requests = official_requests.lock().unwrap();
    assert_eq!(official_requests.len(), 1);
    assert!(official_requests[0].get("max_output_tokens").is_none());
}

#[tokio::test]
async fn codex_thread_title_mapping_never_falls_back_when_selected_model_is_missing() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, _, _, _, _) =
        spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    config.providers[0]
        .selected_models
        .push("gpt-missing".to_owned());
    config.providers[0].codex_thread_title_model = Some("gpt-missing".to_owned());
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut body = responses_request();
    body["model"] = json!("gpt-6-luna");
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header(
            "x-codex-turn-metadata",
            r#"{"turn_trigger":"thread_title"}"#,
        )
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(official_requests.lock().unwrap().is_empty());
    assert!(upstream_requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn review_override_routes_http_without_auxiliary_switch() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (official_url, official_requests, _, _, _, _) =
        spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    config.providers[0].auto_review_model = Some("codex-auto-review".to_owned());
    config.providers[0]
        .selected_models
        .push("codex-auto-review".to_owned());
    config.providers[0].cached_models.push(ProviderModel {
        id: "codex-auto-review".to_owned(),
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
    let mut request = responses_request();
    request["model"] = json!("codex-auto-review");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        upstream_requests.lock().unwrap()[0]["model"],
        "codex-auto-review"
    );
    assert!(official_requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn baidu_review_override_routes_the_selected_model() {
    let (upstream_url, upstream_requests) = spawn_baidu_protocol_upstream().await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    configure_custom_headers_from_env(&mut config);
    config.providers[0].model_source = ProviderModelSource::BaiduOneApi;
    let review_model = config.providers[0].selected_models[0].clone();
    config.providers[0].auto_review_model = Some(review_model.clone());
    config.accept_codex_oauth = false;
    assert!(
        config.providers[0]
            .cached_models
            .iter()
            .all(|model| model.id != "codex-auto-review")
    );
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("codex-auto-review");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let upstream_requests = upstream_requests.lock().unwrap();
    assert_eq!(upstream_requests.len(), 1);
    assert_eq!(upstream_requests[0]["path"], "/v1/messages");
    assert_eq!(upstream_requests[0]["body"]["model"], review_model);
}

#[tokio::test]
async fn custom_only_routes_available_auto_review_model_to_provider() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.providers[0].cached_models.push(ProviderModel {
        id: "codex-auto-review".to_owned(),
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
    let mut request = responses_request();
    request["model"] = json!("codex-auto-review");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        upstream_requests.lock().unwrap()[0]["model"],
        "codex-auto-review"
    );
}

#[tokio::test]
async fn custom_only_rejects_unavailable_auto_review_model_without_official_auth() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"auth_mode":"bedrockApiKey","bedrock_api_key":{"api_key":"codex-mixin-local-test","region":"us-east-1"}}"#,
    )
    .unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = responses_request();
    request["model"] = json!("codex-auto-review");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(upstream_requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn routes_official_gpt_and_custom_gpt_aliases_separately() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let (
        official_url,
        official_requests,
        official_auth_headers,
        official_account_headers,
        _,
        official_forwarded_headers,
    ) = spawn_mock_official(OfficialWebSocketBehavior::Persistent).await;
    let mut config = test_config(upstream_url);
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    let original_auth =
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#;
    std::fs::write(&config.codex_auth_path, original_auth).unwrap();
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();

    let mut official_request = responses_request();
    official_request["model"] = json!("gpt-5.5");
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("originator", "codex_cli_rs")
        .header("x-codex-originator", "legacy-codex")
        .header("x-openai-subagent", "review")
        .header("x-openai-memgen-request", "true")
        .json(&official_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(official_requests.lock().unwrap()[0]["model"], "gpt-5.5");
    assert_eq!(
        official_auth_headers.lock().unwrap()[0].as_deref(),
        Some("Bearer codex-oauth-token")
    );
    assert_eq!(
        official_account_headers.lock().unwrap()[0].as_deref(),
        Some("account-1")
    );
    {
        let forwarded_headers = official_forwarded_headers.lock().unwrap();
        assert_eq!(
            forwarded_headers[0]
                .get("originator")
                .and_then(|value| value.to_str().ok()),
            Some("codex_cli_rs")
        );
        assert_eq!(
            forwarded_headers[0]
                .get("x-codex-originator")
                .and_then(|value| value.to_str().ok()),
            Some("legacy-codex")
        );
        assert_eq!(
            forwarded_headers[0]
                .get("x-openai-subagent")
                .and_then(|value| value.to_str().ok()),
            Some("review")
        );
        assert_eq!(
            forwarded_headers[0]
                .get("x-openai-memgen-request")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
    }
    assert!(upstream_requests.lock().unwrap().is_empty());

    let mut custom_request = responses_request();
    custom_request["model"] = json!("gpt-5.5-custom");
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("codex-oauth-token")
        .json(&custom_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(official_requests.lock().unwrap().len(), 1);
    assert_eq!(upstream_requests.lock().unwrap()[0]["model"], "gpt-5.5");
    assert_eq!(
        std::fs::read_to_string(codex_home.path().join("auth.json")).unwrap(),
        original_auth
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handles_one_hundred_parallel_streams() {
    let (upstream_url, _) = spawn_mock_upstream(MockMode::Text).await;
    let gateway_url = spawn_gateway(upstream_url).await;
    let client = reqwest::Client::new();
    let jobs = (0..100).map(|_| {
        let client = client.clone();
        let url = format!("{gateway_url}/v1/responses");
        async move {
            let response = client
                .post(url)
                .bearer_auth("gateway-key")
                .json(&responses_request())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = response.text().await.unwrap();
            assert!(body.contains("response.completed"));
        }
    });
    join_all(jobs).await;
}
