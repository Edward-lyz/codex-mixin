use super::*;

#[tokio::test]
async fn fusion_runs_on_every_user_turn_then_routes_tool_continuations_to_final() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![fusion_profile()];
    let gateway_url = spawn_gateway_with_config(config).await;
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
    let fusion_model = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == "auto/default")
        .unwrap();
    assert_eq!(fusion_model["display_name"], "auto/default");

    let mut request = fusion_request();
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = response.text().await.unwrap();
    assert!(response_body.contains("response.output_text.delta"));
    let mut encoded = response_body.into_bytes();
    let events = drain_events(&mut encoded);
    let detail_items = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("response.output_item.done"))
        .filter_map(|event| serde_json::from_str::<Value>(&event.data).ok())
        .filter(|event| {
            event["item"]["type"] == "message"
                && event["item"]["content"][0]["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("## Fusion · "))
        })
        .collect::<Vec<_>>();
    assert_eq!(detail_items.len(), 2);
    assert!(detail_items.iter().all(|event| {
        event["item"]["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("msg_"))
            && event["item"]["role"] == "assistant"
    }));
    let panel_results = detail_items
        .iter()
        .find(|event| {
            event["item"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("## Fusion · Panel Results"))
        })
        .unwrap();
    let panel_results = panel_results["item"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(panel_results.contains("| # | Panel Model | Status | Result |"));
    assert!(panel_results.contains("<code>panel-a-custom</code>"));
    assert!(panel_results.contains("<code>panel-b-custom</code>"));
    assert_eq!(panel_results.matches("<details>").count(), 2);
    assert!(detail_items.iter().any(|event| {
        event["item"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("## Fusion · Judge Synthesis"))
    }));
    let final_message = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("response.output_item.done"))
        .filter_map(|event| serde_json::from_str::<Value>(&event.data).ok())
        .find(|event| {
            event["item"]["type"] == "message"
                && !event["item"]["content"][0]["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("## Fusion · "))
        })
        .unwrap();
    assert_eq!(final_message["output_index"], 2);
    let completed = events
        .into_iter()
        .find(|event| event.event.as_deref() == Some("response.completed"))
        .unwrap();
    let completed: Value = serde_json::from_str(&completed.data).unwrap();
    assert_eq!(completed["response"]["model"], "auto/default");
    assert_eq!(completed["response"]["output"][0]["type"], "message");
    assert_eq!(completed["response"]["output"][1]["type"], "message");
    assert_eq!(completed["response"]["output"][2]["type"], "message");

    let first_turn_requests = requests.lock().unwrap().clone();
    assert_eq!(first_turn_requests.len(), 4);
    let mut models = first_turn_requests
        .iter()
        .map(|request| request["model"].as_str().unwrap())
        .collect::<Vec<_>>();
    models.sort_unstable();
    assert_eq!(models, ["final", "judge", "panel-a", "panel-b"]);
    let final_request = first_turn_requests
        .iter()
        .find(|request| request["model"] == "final")
        .unwrap();
    // The advisory analysis is generated fresh every turn and is explicitly
    // labelled untrusted, so it belongs at the tail of the transcript rather than
    // in the system prompt. Putting it in `system` would also rewrite the cached
    // prefix of the final model on every single turn.
    assert!(
        !final_request["system"]
            .to_string()
            .contains("JUDGE_ANALYSIS"),
        "advisory analysis must stay out of the system prompt"
    );
    let last_message = final_request["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(last_message["role"], "user");
    assert!(
        last_message.to_string().contains("JUDGE_ANALYSIS"),
        "advisory analysis must reach the final model: {last_message}"
    );
    assert!(
        last_message.to_string().contains("hello codex"),
        "panel summaries must reach the final model: {last_message}"
    );
    for panel_request in first_turn_requests
        .iter()
        .filter(|request| request["model"].as_str().unwrap().starts_with("panel-"))
    {
        assert!(
            !panel_request["system"]
                .to_string()
                .contains("You are Codex")
        );
    }

    request["input"] = json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"implement the plan"}]}
    ]);
    request["instructions"] = json!(
        "You are Codex.\n<collaboration_mode># Collaboration Mode: Default\nExecute the requested work.</collaboration_mode>"
    );
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let later_body = response.text().await.unwrap();
    assert!(later_body.contains("response.reasoning_summary_text.delta"));
    let later_turn_requests = requests.lock().unwrap()[4..].to_vec();
    assert_eq!(later_turn_requests.len(), 4);
    let panel_task = later_turn_requests
        .iter()
        .find(|request| request["model"] == "panel-a")
        .unwrap()
        .to_string();
    assert!(panel_task.contains("EARLIER_CONVERSATION"), "{panel_task}");
    assert!(panel_task.contains("answer"), "{panel_task}");
    assert!(panel_task.contains("implement the plan"), "{panel_task}");

    request["input"] = json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"run a tool"}]},
        {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
        {"type":"function_call_output","call_id":"call_1","output":"/tmp"}
    ]);
    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let continuation_body = response.text().await.unwrap();
    assert!(!continuation_body.contains("response.reasoning_summary_text.delta"));
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 9);
    assert_eq!(captured.last().unwrap()["model"], "final");
}

#[tokio::test]
async fn fusion_time_rotation_routes_directly_to_the_active_model() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut config = test_config(upstream_url);
    let mut profile = fusion_profile();
    profile.mode = FusionMode::TimeRotation;
    profile.time_routes = vec![
        FusionTimeRoute {
            start_minute: 0,
            end_minute: 720,
            model: "panel-a-custom".to_owned(),
        },
        FusionTimeRoute {
            start_minute: 720,
            end_minute: 0,
            model: "panel-a-custom".to_owned(),
        },
    ];
    config.fusion_profiles = vec![profile];
    let gateway_url = spawn_gateway_with_config(config).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&fusion_request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = response.text().await.unwrap();
    assert!(!response_body.contains("Fusion · Panel Results"));

    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0]["model"], "panel-a");
}

#[tokio::test]
async fn fusion_uses_codex_inline_visualization_when_thread_root_is_available() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let codex_home = tempfile::tempdir().unwrap();
    let thread_id = uuid::Uuid::new_v4().to_string();
    let visualization_dir = codex_home
        .path()
        .join("visualizations/2026/07/21")
        .join(&thread_id);
    let mut config = test_config(upstream_url);
    config.codex_auth_path = codex_home.path().join("auth.json");
    config.fusion_profiles = vec![fusion_profile()];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = fusion_request();
    request["input"].as_array_mut().unwrap().insert(
        0,
        json!({
            "type":"message",
            "role":"developer",
            "content":[{
                "type":"input_text",
                "text":format!(
                    "<workspace_roots><root>{}</root></workspace_roots>",
                    visualization_dir.display()
                )
            }]
        }),
    );

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .header("thread-id", &thread_id)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let events = drain_events(&mut encoded);
    let detail_texts = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("response.output_item.done"))
        .filter_map(|event| serde_json::from_str::<Value>(&event.data).ok())
        .filter_map(|event| {
            event["item"]["content"][0]["text"]
                .as_str()
                .map(str::to_owned)
        })
        .filter(|text| text.starts_with("## Fusion · "))
        .collect::<Vec<_>>();
    assert_eq!(detail_texts.len(), 1, "{detail_texts:#?}");
    let visualization = detail_texts
        .iter()
        .find(|text| text.starts_with("## Fusion · Review"))
        .unwrap();
    let file_name = visualization
        .lines()
        .find_map(|line| {
            line.strip_prefix("::codex-inline-vis{file=\"")
                .and_then(|value| value.strip_suffix("\"}"))
        })
        .unwrap();
    let fragment = std::fs::read_to_string(visualization_dir.join(file_name)).unwrap();
    assert!(fragment.contains("class=\"viz-grid fusion-panels\""));
    assert!(fragment.contains("class=\"card fusion-panel\""));
    assert!(fragment.contains("panel-a"));
    assert!(fragment.contains("panel-b"));
    assert!(fragment.contains("Judge synthesis"));
    assert!(fragment.contains("data-fusion-point"));
    assert!(
        !detail_texts
            .iter()
            .any(|text| text.contains("Final Answer"))
    );
    let completed = events
        .iter()
        .find(|event| event.event.as_deref() == Some("response.completed"))
        .unwrap();
    let completed: Value = serde_json::from_str(&completed.data).unwrap();
    assert_eq!(completed["response"]["output"].as_array().unwrap().len(), 2);
    assert_eq!(requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn fusion_can_hide_intermediate_results() {
    let (upstream_url, requests) = spawn_mock_upstream(MockMode::Text).await;
    let mut profile = fusion_profile();
    profile.show_intermediate_results = false;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![profile];
    let gateway_url = spawn_gateway_with_config(config).await;
    let request = fusion_request();

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut encoded = response.bytes().await.unwrap().to_vec();
    let events = drain_events(&mut encoded);
    assert!(!events.iter().any(|event| {
        serde_json::from_str::<Value>(&event.data)
            .ok()
            .and_then(|event| {
                event["item"]["content"][0]["text"]
                    .as_str()
                    .map(str::to_owned)
            })
            .is_some_and(|text| text.starts_with("## Fusion · "))
    }));
    let final_message = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("response.output_item.done"))
        .filter_map(|event| serde_json::from_str::<Value>(&event.data).ok())
        .find(|event| event["item"]["type"] == "message")
        .unwrap();
    assert_eq!(final_message["output_index"], 0);
    let completed = events
        .iter()
        .find(|event| event.event.as_deref() == Some("response.completed"))
        .unwrap();
    let completed: Value = serde_json::from_str(&completed.data).unwrap();
    assert_eq!(completed["response"]["output"].as_array().unwrap().len(), 1);
    assert_eq!(requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn fusion_panels_run_concurrently() {
    // Hold both requests until they meet instead of racing the runner's scheduling.
    let (upstream_url, requests, max_active) =
        spawn_fusion_mock_upstream(Some(Arc::new(tokio::sync::Barrier::new(2))), Vec::new()).await;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![fusion_profile()];
    let gateway_url = spawn_gateway_with_config(config).await;
    let request = fusion_request();

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = tokio::time::timeout(Duration::from_secs(15), response.text())
        .await
        .expect("Fusion panel requests must overlap")
        .unwrap();
    assert!(body.contains("response.completed"), "{body}");
    assert_eq!(max_active.load(Ordering::SeqCst), 2);
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request["model"] == "judge"),
        "both panels must succeed before the judge runs"
    );
}

#[tokio::test]
async fn fusion_routes_models_across_official_and_upstream_providers() {
    let (upstream_url, upstream_requests) = spawn_mock_upstream(MockMode::Text).await;
    let official_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = official_requests.clone();
    let official_url = spawn_router(Router::new().route(
        "/v1/responses",
        post(move |Json(body): Json<Value>| {
            let captured = captured.clone();
            async move {
                captured.lock().unwrap().push(body.clone());
                let model = body["model"].as_str().unwrap_or("gpt-5.6-sol");
                let output = if body["instructions"]
                    .as_str()
                    .is_some_and(|instructions| {
                        instructions.contains("substantive, concise report")
                    })
                {
                    r#"{"findings":["Official panel finding."],"risks":[],"recommendations":["Official recommendation."],"evidence":["official evidence"]}"#
                } else {
                    "official result"
                };
                let item = json!({
                    "id":"msg_official",
                    "type":"message",
                    "status":"completed",
                    "role":"assistant",
                    "content":[{"type":"output_text","text":output,"annotations":[]}]
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(format!(
                        "event: response.output_item.done\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
                        json!({
                            "type":"response.output_item.done",
                            "output_index":0,
                            "item":item
                        }),
                        json!({
                            "type":"response.completed",
                            "response":{
                                "id":"resp_official",
                                "object":"response",
                                "status":"completed",
                                "model":model,
                                "output":[],
                                "usage":null
                            }
                        })
                    )))
                    .unwrap()
            }
        }),
    ))
    .await;
    let mut config = test_config(upstream_url);
    configure_baidu_policy(&mut config);
    config.official_responses_url = format!("{official_url}/v1/responses");
    let codex_home = tempfile::tempdir().unwrap();
    config.codex_auth_path = codex_home.path().join("auth.json");
    std::fs::write(
        &config.codex_auth_path,
        r#"{"tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}}"#,
    )
    .unwrap();
    config.fusion_profiles = vec![FusionProfile {
        id: "mixed".to_owned(),
        mode: Default::default(),
        time_routes: Vec::new(),
        panel_models: vec![
            "official:gpt-5.6-sol".to_owned(),
            "panel-a-custom".to_owned(),
        ],
        judge_model: "judge-custom".to_owned(),
        final_model: "official:gpt-5.6-sol".to_owned(),
        min_successful: 2,
        max_completion_tokens: 2048,
        timeout_ms: 300_000,
        show_intermediate_results: true,
        panel_tools: PanelToolsConfig {
            enabled: false,
            ..PanelToolsConfig::default()
        },
    }];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = fusion_request();
    request["model"] = json!("auto/mixed");
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("official result"));
    let upstream_models = upstream_requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request["model"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(upstream_models.contains(&"panel-a".to_owned()));
    assert!(upstream_models.contains(&"judge".to_owned()));
    {
        let official = official_requests.lock().unwrap();
        assert_eq!(official.len(), 2);
        assert!(
            official
                .iter()
                .all(|request| request["model"] == "gpt-5.6-sol")
        );
        assert!(official.iter().all(|request| request["store"] == false));
        assert!(
            official
                .iter()
                .all(|request| request.get("max_output_tokens").is_none())
        );
        assert!(official[0].get("text").is_none());
        assert!(official[1].to_string().contains("JUDGE_ANALYSIS"));
    }

    request["input"] = json!([
        {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
        {"type":"function_call_output","call_id":"call_1","output":"/tmp"}
    ]);
    let continuation = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(continuation.status(), StatusCode::OK);
    assert!(continuation.text().await.unwrap().contains("auto/mixed"));
    assert_eq!(official_requests.lock().unwrap().len(), 3);
    assert_eq!(upstream_requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn fusion_degrades_on_partial_and_total_panel_failure() {
    let (upstream_url, requests, _) = spawn_fusion_mock_upstream(None, vec!["panel-b"]).await;
    let mut partial_profile = fusion_profile();
    partial_profile.min_successful = 1;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![partial_profile];
    let gateway_url = spawn_gateway_with_config(config).await;
    let request = fusion_request();
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("response.completed"));
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .any(|body| body["model"] == "judge")
    );

    let (upstream_url, requests, _) =
        spawn_fusion_mock_upstream(None, vec!["panel-a", "panel-b"]).await;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![fusion_profile()];
    let gateway_url = spawn_gateway_with_config(config).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth("gateway-key")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    assert!(body.contains("panel 成功数不足"));
    assert!(body.contains("response.completed"));
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 3);
    assert!(!captured.iter().any(|body| body["model"] == "judge"));
    assert!(captured.iter().any(|body| body["model"] == "final"));
}

#[tokio::test]
async fn fusion_panel_tool_loop_stops_at_round_limit() {
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let app = Router::new()
        .route(
            "/v1/messages",
            post(
                |State(requests): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                    let model = body["model"].as_str().unwrap_or_default().to_owned();
                    requests.lock().unwrap().push(body);
                    let payload = if model == "panel-a" {
                        tool_sse("read_file", json!({"path":"input.txt"}))
                    } else {
                        text_sse()
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(payload))
                        .unwrap()
                },
            ),
        )
        .with_state(requests.clone());
    let upstream_url = spawn_router(app).await;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("input.txt"), "tool input").unwrap();
    let mut profile = fusion_profile();
    profile.panel_models = vec!["panel-a-custom".to_owned()];
    profile.min_successful = 1;
    profile.panel_tools.enabled = true;
    profile.panel_tools.max_rounds = 1;
    let mut config = test_config(upstream_url);
    config.fusion_profiles = vec![profile];
    let gateway_url = spawn_gateway_with_config(config).await;
    let mut request = fusion_request();
    request["input"][1]["content"][0]["text"] = json!(format!(
        "inspect the file\n<environment_context><cwd>{}</cwd></environment_context>",
        workspace.path().display()
    ));
    let response = reqwest::Client::new()
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
    let captured = requests.lock().unwrap();
    let panel_requests = captured
        .iter()
        .filter(|request| request["model"] == "panel-a")
        .collect::<Vec<_>>();
    assert_eq!(panel_requests.len(), 2);
    assert!(
        panel_requests[1]
            .to_string()
            .contains("tool budget is exhausted")
    );
    assert_eq!(captured.last().unwrap()["model"], "final");
}
