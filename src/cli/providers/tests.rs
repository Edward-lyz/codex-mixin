use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::Router;
use axum::http::{HeaderMap, header};
use axum::routing::get;
use codex_mixin::fusion::{FusionProfile, PanelToolsConfig};

use super::discovery::{
    detect_custom_provider_protocol, discover_custom_quota, endpoint_join,
    infer_custom_provider_endpoint, protocol_probe_body_matches,
};
use super::management::{
    remove_provider_from_config, reorder_provider_ids, set_auxiliary_model_upstream,
};
use super::models::apply_model_selection;
use super::*;
use codex_mixin::provider::{ProviderModel, redact_provider_error};

#[test]
fn selecting_unknown_model_adds_it_with_safe_defaults() {
    let mut provider = codex_mixin::provider::custom_provider("custom", "key");
    provider.base_url = "https://example.test".to_owned();

    let contexts = BTreeMap::from([("hidden-model".to_owned(), 256_000)]);
    apply_model_selection(&mut provider, vec!["hidden-model".to_owned()], &contexts).unwrap();

    assert_eq!(provider.selected_models, ["hidden-model"]);
    let model = &provider.cached_models[0];
    assert!(model.manually_added);
    assert_eq!(model.context_window, Some(256_000));
    assert_eq!(model.supports_image, Some(false));
    assert_eq!(model.supports_thinking, Some(true));
    assert_eq!(model.supports_web_search, Some(false));
    assert_eq!(model.supports_tool_search, Some(false));
    assert_eq!(model.supports_function_tools, Some(true));

    apply_model_selection(&mut provider, Vec::new(), &BTreeMap::new()).unwrap();

    assert!(provider.selected_models.is_empty());
    assert!(provider.cached_models.is_empty());
}

#[test]
fn context_override_rejects_discovered_model() {
    let mut provider = codex_mixin::provider::custom_provider("custom", "key");
    provider
        .cached_models
        .push(codex_mixin::provider::ProviderModel {
            id: "discovered-model".to_owned(),
            ..codex_mixin::provider::ProviderModel::default()
        });
    let contexts = BTreeMap::from([("discovered-model".to_owned(), 256_000)]);

    let error = apply_model_selection(
        &mut provider,
        vec!["discovered-model".to_owned()],
        &contexts,
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("model context can only be edited for manually added models")
    );
}

#[test]
fn official_provider_view_is_reserved_and_read_only() {
    let config = StoredGatewayConfig {
        official_selected_models: Some(vec!["gpt-5.6-sol".to_owned()]),
        ..StoredGatewayConfig::default()
    };
    let provider = official_provider_view(
        &config,
        vec![ProviderModel {
            id: "gpt-5.6-sol".to_owned(),
            ..ProviderModel::default()
        }],
    );

    assert!(official_provider_is_available(Some("codex_oauth_proxy")));
    assert!(!official_provider_is_available(Some("custom_only")));
    assert!(!official_provider_is_available(None));
    assert_eq!(provider["id"], "official");
    assert_eq!(provider["kind"], "official");
    assert_eq!(provider["display_name"], "OpenAI");
    assert_eq!(provider["enabled"], true);
    assert_eq!(provider["selected_models"], json!(["gpt-5.6-sol"]));
    assert_eq!(provider["cached_models"][0]["id"], "gpt-5.6-sol");
}

#[test]
fn baidu_oneapi_add_without_bridge_leaves_loopback_unset() {
    let mut provider = codex_mixin::provider::baidu_oneapi_provider("baidu-oneapi", "key");

    apply_baidu_auth_options(&mut provider, None, None).unwrap();

    assert_eq!(provider.request_policy.baidu_auth_bridge, None);
}

#[test]
fn provider_mutations_persist_managed_ducx_options() {
    let mut provider = codex_mixin::provider::baidu_oneapi_provider("baidu-oneapi", "key");
    provider.quota_username = Some("user@example.com".to_owned());
    let executable =
        PathBuf::from("/Users/example/.codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx");

    apply_baidu_auth_options(
        &mut provider,
        Some("ducx_loopback"),
        Some(executable.clone()),
    )
    .unwrap();

    assert_eq!(
        provider.request_policy.baidu_auth_bridge,
        Some(BaiduAuthBridge::DucxLoopback)
    );
    assert_eq!(provider.request_policy.ducx_executable, Some(executable));
    provider.request_policy.baidu_code_report = true;
    provider.request_policy.data_report_executable = provider
        .request_policy
        .ducx_executable
        .as_deref()
        .and_then(data_report_sibling);
    assert_eq!(
        provider.request_policy.data_report_executable,
        Some(PathBuf::from(
            "/Users/example/.codex-mixin/ducx/home/.baidu-cx/baidu-cx/hooks/data-report"
        ))
    );
    provider.validate().unwrap();
}

#[test]
fn parses_custom_header_environment_mappings() {
    let mapping = parse_header_env(&[
        "x-example-auth=EXAMPLE_AUTH".to_owned(),
        "x-routing-token=ROUTING_TOKEN".to_owned(),
    ])
    .unwrap();

    assert_eq!(mapping["x-example-auth"], "EXAMPLE_AUTH");
    assert_eq!(mapping["x-routing-token"], "ROUTING_TOKEN");
    assert!(parse_header_env(&["missing-separator".to_owned()]).is_err());
}

#[test]
fn parses_opencode_go_quota_parser() {
    assert_eq!(
        parse_quota_parser("opencode_go").unwrap(),
        ProviderQuotaParser::OpenCodeGo
    );
    assert_eq!(
        parse_quota_parser("opencode-go").unwrap(),
        ProviderQuotaParser::OpenCodeGo
    );
}

#[tokio::test]
async fn discovers_a_read_only_custom_quota_endpoint() {
    let authorization = Arc::new(Mutex::new(None));
    let captured_authorization = Arc::clone(&authorization);
    let app = Router::new().route(
        "/api/v1/credits",
        get(move |headers: HeaderMap| {
            let captured_authorization = Arc::clone(&captured_authorization);
            async move {
                *captured_authorization.lock().unwrap() = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                axum::Json(serde_json::json!({
                    "data": {
                        "total_usage": 12.5,
                        "total_credits": 100,
                        "currency": "USD"
                    }
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "community-secret");
    provider.base_url = format!("http://{address}");
    let client = reqwest::Client::new();

    let discovered = discover_custom_quota(&client, &provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        discovered.url.as_str(),
        format!("http://{address}/api/v1/credits")
    );
    assert_eq!(discovered.currency.as_deref(), Some("USD"));
    assert_eq!(discovered.usage.used, Some(12.5));
    assert_eq!(discovered.usage.limit, Some(100.0));
    assert_eq!(
        authorization.lock().unwrap().as_deref(),
        Some("Bearer community-secret")
    );
}

#[tokio::test]
async fn discovers_new_api_token_usage_with_its_canonical_trailing_slash() {
    let app = Router::new().route(
        "/api/usage/token/",
        get(|| async {
            axum::Json(serde_json::json!({
                "code": true,
                "message": "ok",
                "data": {
                    "object": "token_usage",
                    "total_granted": 100,
                    "total_used": 12.5,
                    "total_available": 87.5
                }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("new-api", "new-api-key");
    provider.base_url = format!("http://{address}");

    let discovered = discover_custom_quota(&reqwest::Client::new(), &provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        discovered.url.as_str(),
        format!("http://{address}/api/usage/token/")
    );
    assert_eq!(discovered.usage.used, Some(12.5));
    assert_eq!(discovered.usage.limit, Some(100.0));
    assert_eq!(discovered.usage.remaining, Some(87.5));
}

#[tokio::test]
async fn discovers_sub2api_wallet_usage_from_the_api_key_endpoint() {
    let app = Router::new().route(
        "/v1/usage",
        get(|| async {
            axum::Json(serde_json::json!({
                "mode": "unrestricted",
                "isValid": true,
                "remaining": 37.5,
                "balance": 37.5,
                "unit": "USD",
                "usage": {
                    "total": {
                        "actual_cost": 12.5
                    }
                }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("sub2api", "sub2api-key");
    provider.base_url = format!("http://{address}");

    let discovered = discover_custom_quota(&reqwest::Client::new(), &provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        discovered.url.as_str(),
        format!("http://{address}/v1/usage")
    );
    assert_eq!(discovered.currency.as_deref(), Some("USD"));
    assert_eq!(discovered.usage.used, Some(12.5));
    assert_eq!(discovered.usage.limit, Some(50.0));
    assert_eq!(discovered.usage.remaining, Some(37.5));
}

#[test]
fn reorders_provider_ids_without_changing_provider_data() {
    let mut first = codex_mixin::provider::custom_provider("first", "first-key");
    first.selected_models = vec!["first-model".to_owned()];
    first.enabled = false;
    first.quota_username = Some("first-user".to_owned());
    first.request_policy.baidu_auth_bridge = Some(BaiduAuthBridge::DucxLoopback);
    first.request_policy.baidu_code_report = true;
    let mut second = codex_mixin::provider::custom_provider("second", "second-key");
    second.selected_models = vec!["second-model".to_owned()];
    second.quota_username = Some("second-user".to_owned());
    let first_before = first.clone();
    let second_before = second.clone();
    let mut config = StoredGatewayConfig {
        providers: vec![first, second],
        ..StoredGatewayConfig::default()
    };

    reorder_provider_ids(&mut config, &["second".to_owned(), "first".to_owned()]).unwrap();

    assert_eq!(
        config
            .providers
            .iter()
            .map(|provider| provider.id.as_str())
            .collect::<Vec<_>>(),
        ["second", "first"]
    );
    assert_eq!(config.providers[0].selected_models, ["second-model"]);
    assert_eq!(config.providers[1].selected_models, ["first-model"]);
    assert_eq!(config.providers, [second_before, first_before]);
}

#[test]
fn rejects_incomplete_or_duplicate_provider_orders() {
    let first = codex_mixin::provider::custom_provider("first", "first-key");
    let second = codex_mixin::provider::custom_provider("second", "second-key");
    let mut config = StoredGatewayConfig {
        providers: vec![first, second],
        ..StoredGatewayConfig::default()
    };

    assert!(reorder_provider_ids(&mut config, &["first".to_owned()]).is_err());
    assert!(reorder_provider_ids(&mut config, &["first".to_owned(), "first".to_owned()]).is_err());
    assert_eq!(
        config
            .providers
            .iter()
            .map(|provider| provider.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
}

#[test]
fn removing_a_generated_provider_compacts_ids_and_fusion_model_references() {
    let mut first = codex_mixin::provider::custom_provider("custom", "first-key");
    first.selected_models = vec!["first-model".to_owned()];
    first.cached_models = vec![ProviderModel {
        id: "first-model".to_owned(),
        ..ProviderModel::default()
    }];
    let mut second = codex_mixin::provider::custom_provider("custom-2", "second-key");
    second.selected_models = vec!["second-model".to_owned()];
    second.cached_models = vec![ProviderModel {
        id: "second-model".to_owned(),
        ..ProviderModel::default()
    }];
    let mut third = codex_mixin::provider::custom_provider("custom-3", "third-key");
    third.selected_models = vec!["third-model".to_owned()];
    third.cached_models = vec![ProviderModel {
        id: "third-model".to_owned(),
        ..ProviderModel::default()
    }];
    let mut config = StoredGatewayConfig {
        providers: vec![first, second, third],
        fusion_profiles: vec![FusionProfile {
            id: "review".to_owned(),
            panel_models: vec![
                "second-model-custom-2".to_owned(),
                "third-model-custom-3".to_owned(),
            ],
            judge_model: "second-model-custom-2".to_owned(),
            final_model: "third-model-custom-3".to_owned(),
            min_successful: 1,
            max_completion_tokens: 2_048,
            timeout_ms: 300_000,
            show_intermediate_results: true,
            panel_tools: PanelToolsConfig::default(),
        }],
        ..StoredGatewayConfig::default()
    };

    remove_provider_from_config(&mut config, "custom").unwrap();

    assert_eq!(
        config
            .providers
            .iter()
            .map(|provider| provider.id.as_str())
            .collect::<Vec<_>>(),
        ["custom", "custom-2"]
    );
    assert_eq!(
        config.fusion_profiles[0].panel_models,
        ["second-model-custom", "third-model-custom-2"]
    );
    assert_eq!(config.fusion_profiles[0].judge_model, "second-model-custom");
    assert_eq!(
        config.fusion_profiles[0].final_model,
        "third-model-custom-2"
    );
}

#[test]
fn selecting_auxiliary_model_upstream_is_exclusive_and_can_be_cleared() {
    let first = codex_mixin::provider::custom_provider("first", "first-key");
    let mut second = codex_mixin::provider::custom_provider("second", "second-key");
    second.auxiliary_model_upstream = true;
    let mut config = StoredGatewayConfig {
        providers: vec![first, second],
        ..StoredGatewayConfig::default()
    };

    set_auxiliary_model_upstream(&mut config, "first", true).unwrap();
    assert!(config.providers[0].auxiliary_model_upstream);
    assert!(!config.providers[1].auxiliary_model_upstream);

    set_auxiliary_model_upstream(&mut config, "first", false).unwrap();
    assert!(
        config
            .providers
            .iter()
            .all(|provider| !provider.auxiliary_model_upstream)
    );
}

#[test]
fn infers_custom_provider_endpoints_without_exposing_protocol_fields() {
    let openai = infer_custom_provider_endpoint("https://public.example/v1").unwrap();
    assert_eq!(openai.base_url, "https://public.example/v1");
    assert_eq!(openai.protocol, ProviderProtocol::OpenAiResponses);
    assert_eq!(openai.api_path, "/v1/responses");
    assert_eq!(openai.models_path, "/v1/models");
    assert!(!openai.path_explicit);

    let anthropic =
        infer_custom_provider_endpoint("https://public.example/api/v1/messages").unwrap();
    assert_eq!(anthropic.base_url, "https://public.example/api");
    assert_eq!(anthropic.protocol, ProviderProtocol::AnthropicMessages);
    assert_eq!(anthropic.api_path, "/v1/messages");
    assert_eq!(anthropic.models_path, "/v1/models");
    assert!(anthropic.path_explicit);

    let responses = infer_custom_provider_endpoint("https://public.example/v1/responses").unwrap();
    assert_eq!(responses.base_url, "https://public.example");
    assert_eq!(responses.protocol, ProviderProtocol::OpenAiResponses);
    assert_eq!(responses.api_path, "/v1/responses");
    assert_eq!(responses.models_path, "/v1/models");
    assert!(responses.path_explicit);
    assert_eq!(
        endpoint_join("https://public.example/api/v1", "/v1/models")
            .unwrap()
            .as_str(),
        "https://public.example/api/v1/models"
    );
}

#[tokio::test]
async fn detects_responses_before_messages_and_chat_for_custom_providers() {
    use axum::routing::post;
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
        )
        .route(
            "/v1/responses",
            post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                )
            }),
        )
        .route(
            "/v1/messages",
            post(|| async {
                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({"id":"messages-should-not-win"})),
                )
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|| async {
                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({"id":"should-not-win"})),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "secret");
    provider.base_url = format!("http://{address}");
    assert_eq!(
        endpoint_join(&provider.base_url, "/v1/models")
            .unwrap()
            .path(),
        "/v1/models"
    );

    let detected = detect_custom_provider_protocol(&provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(detected.protocol, ProviderProtocol::OpenAiResponses);
    assert_eq!(detected.api_path, "/v1/responses");
    assert_eq!(detected.models_path, "/v1/models");
}

#[tokio::test]
async fn allows_slow_custom_protocol_probes() {
    use axum::routing::post;
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
        )
        .route(
            "/v1/responses",
            post(|| async {
                tokio::time::sleep(std::time::Duration::from_secs(6)).await;
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "secret");
    provider.base_url = format!("http://{address}");

    let detected = detect_custom_provider_protocol(&provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(detected.protocol, ProviderProtocol::OpenAiResponses);
}

#[tokio::test]
async fn forbidden_protocol_probes_do_not_switch_custom_providers_to_messages() {
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
        )
        .fallback(|| async {
            (
                axum::http::StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({
                    "error": {"message": "This group does not allow this protocol dispatch"}
                })),
            )
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "secret");
    provider.base_url = format!("http://{address}");

    let error = detect_custom_provider_protocol(&provider)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("models endpoint is valid"));
    assert!(error.contains("protocol detection failed"));
    assert!(error.contains("within 30 seconds"));
    assert_eq!(provider.protocol, ProviderProtocol::OpenAiResponses);
    assert_eq!(provider.api_path, "/v1/responses");
}

#[tokio::test]
async fn falls_back_to_messages_when_responses_is_missing() {
    use axum::routing::post;
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
        )
        .route(
            "/v1/messages",
            post(|| async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    axum::Json(serde_json::json!({"error":{"type":"authentication_error"}})),
                )
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|| async {
                (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({"id":"chat"})),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "secret");
    provider.base_url = format!("http://{address}");

    let detected = detect_custom_provider_protocol(&provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(detected.protocol, ProviderProtocol::AnthropicMessages);
    assert_eq!(detected.api_path, "/v1/messages");
}

#[tokio::test]
async fn falls_back_to_chat_when_native_apis_are_missing() {
    use axum::routing::post;
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
        )
        .route(
            "/v1/chat/completions",
            post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error":{"message":"missing messages"}})),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "secret");
    provider.base_url = format!("http://{address}");

    let detected = detect_custom_provider_protocol(&provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(detected.protocol, ProviderProtocol::OpenAiChat);
    assert_eq!(detected.api_path, "/v1/chat/completions");
}

#[tokio::test]
async fn accepts_an_empty_v1_models_list_as_a_real_endpoint() {
    use axum::routing::post;
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(serde_json::json!({"data":[]})) }),
        )
        .route(
            "/v1/responses",
            post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "secret");
    provider.base_url = format!("http://{address}");

    let detected = detect_custom_provider_protocol(&provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(detected.protocol, ProviderProtocol::OpenAiResponses);
}

#[tokio::test]
async fn falls_back_to_legacy_paths_after_v1_models_failure() {
    use axum::response::Html;
    use axum::routing::post;
    let legacy_requests = Arc::new(AtomicUsize::new(0));
    let legacy_requests_for_handler = Arc::clone(&legacy_requests);
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { Html("<!doctype html><html>login</html>") }),
        )
        .route(
            "/models",
            get(move || {
                legacy_requests_for_handler.fetch_add(1, Ordering::Relaxed);
                async { axum::Json(serde_json::json!({"data":[{"id":"legacy"}]})) }
            }),
        )
        .route(
            "/responses",
            post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "secret");
    provider.base_url = format!("http://{address}");

    let detected = detect_custom_provider_protocol(&provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(detected.api_path, "/responses");
    assert_eq!(detected.models_path, "/models");
    assert_eq!(legacy_requests.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn falls_back_to_legacy_response_paths_after_v1_models_succeeds() {
    use axum::routing::post;
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { axum::Json(serde_json::json!({"data":[{"id":"model"}]})) }),
        )
        .route(
            "/models",
            get(|| async { axum::Json(serde_json::json!({"data":[{"id":"legacy"}]})) }),
        )
        .route(
            "/responses",
            post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "secret");
    provider.base_url = format!("http://{address}");

    let detected = detect_custom_provider_protocol(&provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(detected.api_path, "/responses");
    assert_eq!(detected.models_path, "/models");
}

#[tokio::test]
async fn falls_back_to_legacy_paths_after_v1_models_api_error() {
    use axum::routing::post;
    let legacy_requests = Arc::new(AtomicUsize::new(0));
    let legacy_requests_for_handler = Arc::clone(&legacy_requests);
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    axum::Json(serde_json::json!({
                        "error": {"message": "invalid API key"}
                    })),
                )
            }),
        )
        .route(
            "/models",
            get(move || {
                legacy_requests_for_handler.fetch_add(1, Ordering::Relaxed);
                async { axum::Json(serde_json::json!({"data": [{"id": "legacy"}]})) }
            }),
        )
        .route(
            "/responses",
            post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error":{"message":"missing input"}})),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "secret");
    provider.base_url = format!("http://{address}");

    let detected = detect_custom_provider_protocol(&provider)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(detected.api_path, "/responses");
    assert_eq!(detected.models_path, "/models");
    assert_eq!(legacy_requests.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn rejects_unrelated_json_from_the_models_endpoint() {
    let app = Router::new().route(
        "/v1/models",
        get(|| async { axum::Json(serde_json::json!({"status":"ok"})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = codex_mixin::provider::custom_provider("community", "secret");
    provider.base_url = format!("http://{address}");

    let error = detect_custom_provider_protocol(&provider)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("neither a valid /v1/models nor /models endpoint"));
}

#[test]
fn protocol_probe_rejects_pages_and_accepts_protocol_errors() {
    assert!(!protocol_probe_body_matches(
        ProviderProtocol::OpenAiResponses,
        "text/html; charset=utf-8",
        "<html>dashboard</html>"
    ));
    assert!(!protocol_probe_body_matches(
        ProviderProtocol::OpenAiResponses,
        "application/json",
        "{\"object\":\"list\",\"data\":[]}"
    ));
    assert!(protocol_probe_body_matches(
        ProviderProtocol::OpenAiResponses,
        "application/json",
        "{\"error\":{\"message\":\"missing input\"}}"
    ));
    assert!(protocol_probe_body_matches(
        ProviderProtocol::OpenAiResponses,
        "text/event-stream",
        "data: {\"id\":\"resp_1\",\"object\":\"response\"}\n\n"
    ));
    assert!(protocol_probe_body_matches(
        ProviderProtocol::OpenAiResponses,
        "text/event-stream",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n"
    ));
    assert!(protocol_probe_body_matches(
        ProviderProtocol::AnthropicMessages,
        "text/event-stream",
        "data: {\"type\":\"message_start\",\"message\":{\"type\":\"message\",\"content\":[]}}\n\n"
    ));
}

#[test]
fn model_selection_can_preserve_or_remove_an_unavailable_selected_model() {
    let mut provider = codex_mixin::provider::open_code_go_provider("provider", "key");
    provider.selected_models.push("temporarily-gone".to_owned());
    provider.new_models = vec!["new-model".to_owned()];
    provider.cached_models.push(ProviderModel {
        id: "new-model".to_owned(),
        ..ProviderModel::default()
    });

    apply_model_selection(
        &mut provider,
        vec!["glm-5.2".to_owned(), "temporarily-gone".to_owned()],
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(provider.selected_models, ["glm-5.2", "temporarily-gone"]);
    assert!(provider.new_models.is_empty());

    apply_model_selection(&mut provider, vec!["glm-5.2".to_owned()], &BTreeMap::new()).unwrap();
    assert_eq!(provider.selected_models, ["glm-5.2"]);
}

#[test]
fn discovery_errors_are_bounded_and_redact_the_provider_key() {
    let provider = codex_mixin::provider::open_code_go_provider("provider", "secret-key");
    let error = format!("request used secret-key: {}", "x".repeat(20_000));

    let redacted = redact_provider_error(&provider, &error);

    assert!(!redacted.contains("secret-key"));
    assert!(redacted.contains("<redacted>"));
    assert_eq!(redacted.chars().count(), 8_000);
}
