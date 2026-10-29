use super::*;
use serde_json::json;

#[test]
fn models_follow_the_report() {
    let models =
        parse_models(&json!({"models":[{"id":"new-upstream-id","label":"New model"}]})).unwrap();
    assert_eq!(models[0].id, "new-upstream-id");
    assert_eq!(models[0].supports_web_search, None);
    assert_eq!(models[0].supports_image, None);
    assert_eq!(models[0].context_window, None);
    assert!(parse_models(&json!({"models":[]})).unwrap().is_empty());
    let models = parse_models(
        &json!({"models":{"api-model":{"displayName":"API Model","inputTokenLimit":32768}}}),
    )
    .unwrap();
    assert_eq!(models[0].id, "api-model");
    assert_eq!(models[0].context_window, Some(32768));
}

#[test]
fn native_token_limits_are_preserved() {
    let models = parse_models(&json!({"models":{
        "gemini-3.8-flash-low": {
            "displayName":"Gemini Flash", "maxTokens":1048576, "maxOutputTokens":65536
        },
        "chat_20706": {"maxTokens":16384}
    }}))
    .unwrap();
    let flash = models
        .iter()
        .find(|model| model.id == "gemini-3.8-flash-low")
        .unwrap();
    assert_eq!(flash.context_window, Some(1_048_576));
    assert_eq!(flash.max_output_tokens, Some(65_536));
    let chat = models
        .iter()
        .find(|model| model.id == "chat_20706")
        .unwrap();
    assert_eq!(chat.context_window, Some(16_384));
    assert_eq!(chat.max_output_tokens, None);
    for field in ["maxTokens", "maxOutputTokens"] {
        let mut report = json!({"models":{"invalid":{}}});
        report["models"]["invalid"][field] = json!(0);
        assert!(parse_models(&report).is_err());
    }
}

#[test]
fn malformed_models_fail_closed() {
    for value in [
        json!({}),
        json!({"models":null}),
        json!({"models":[{"label":"no id"}]}),
        json!({"models":[{"id":"same"},{"id":"same"}]}),
        json!({"models":[{"id":"","label":"empty"}]}),
        json!({"models":{}, "agentModelSorts":{}}),
        json!({"models":{}, "agentModelSorts":[{"groups":[{"modelIds":[1]}]}]}),
        json!({"models":{}, "agentModelSorts":[{}]}),
        json!({"models":{}, "defaultAgentModelId":123}),
    ] {
        assert!(parse_models(&value).is_err());
    }
}

#[test]
fn quota_groups_are_dynamic() {
    let windows = parse_quota(&json!({"groups":[
        {"name":"Future family","buckets":[
            {"name":"Daily window","remaining_fraction":0.25,"reset_time":"2026-10-16T00:00:00Z"},
            {"name":"Inactive","enabled":false}
        ]},
        {"name":"Another family","buckets":[{"name":"Weekly","remaining_fraction":1.0}]}
    ]}))
    .unwrap();
    assert_eq!(windows.len(), 2);
    let daily = windows
        .iter()
        .find(|window| window.id == "Future family/Daily window")
        .unwrap();
    assert_eq!(daily.remaining_percent, 25.0);
    assert_eq!(daily.reset_at.as_deref(), Some("2026-10-16T00:00:00Z"));
}

#[test]
fn unavailable_quota_is_not_zero() {
    for fraction in [json!(null), json!(-0.1), json!(1.1)] {
        assert!(
            parse_quota(&json!({"groups":[{"name":"Family","buckets":[
                {"name":"Limit","remaining_fraction":fraction}
            ]}]}))
            .is_err()
        );
    }
    assert!(parse_quota(&json!({"groups":[]})).is_err());
    assert!(parse_quota(&json!({})).is_err());
}

#[test]
fn native_reports_are_account_bound() {
    let mut provider = super::super::agy_provider("test", "");
    assert!(require_native_account(&provider).is_ok());
    provider.base_url = "https://example.com".to_owned();
    assert!(require_native_account(&provider).is_err());
    provider.base_url = "https://cloudcode-pa.googleapis.com".to_owned();
    provider.auth.api_key = "another-account-key".to_owned();
    assert!(require_native_account(&provider).is_err());
}

#[tokio::test]
async fn projects_are_bound_to_the_credential() {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let requests = std::sync::Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route("/v1internal:loadCodeAssist", post(move |headers: HeaderMap| {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            Json(json!({"cloudaicompanionProject":if headers["authorization"] == "Bearer first" { "first-project" } else { "second-project" }}))
        }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut definition = super::super::agy_provider("fixture", "fixture");
    definition.base_url = format!("http://{address}");
    let provider = ProviderRuntime::new(definition, &|_| None).unwrap();
    for (token, expected) in [
        ("first", "first-project"),
        ("first", "first-project"),
        ("second", "second-project"),
    ] {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        assert_eq!(
            project_id(&reqwest::Client::new(), &provider, &headers)
                .await
                .unwrap(),
            expected
        );
    }
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn projects_missing_from_load_are_resolved_from_the_account_list() {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let loads = std::sync::Arc::new(AtomicUsize::new(0));
    let lists = std::sync::Arc::new(AtomicUsize::new(0));
    let load_count = loads.clone();
    let list_count = lists.clone();
    let app = Router::new()
        .route(
            "/v1internal:loadCodeAssist",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let count = load_count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(headers["authorization"], "Bearer account");
                    assert_eq!(body["metadata"]["ideType"], "ANTIGRAVITY");
                    Json(json!({"currentTier":{"id":"fixture-tier"}}))
                }
            }),
        )
        .route(
            "/v1internal:listCloudAICompanionProjects",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let count = list_count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(headers["authorization"], "Bearer account");
                    assert_eq!(body, json!({}));
                    Json(json!({"projects":[{"projectId":"listed-project"}]}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut definition = super::super::agy_provider("fixture", "fixture");
    definition.base_url = format!("http://{address}");
    let provider = ProviderRuntime::new(definition, &|_| None).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer account".parse().unwrap());
    let client = reqwest::Client::new();

    for _ in 0..2 {
        assert_eq!(
            project_id(&client, &provider, &headers).await.unwrap(),
            "listed-project"
        );
    }
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    assert_eq!(lists.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn project_reports_support_native_aliases_and_account_lists() {
    use axum::{Json, Router, routing::post};

    let reports = [
        (
            json!({"backendProjectId":" backend-project "}),
            "backend-project",
        ),
        (
            json!({"userDefinedCloudaicompanionProject":{"id":"managed-project"}}),
            "managed-project",
        ),
        (json!({"project":"direct-project"}), "direct-project"),
        (
            json!({"cloudaicompanionProject":{"id":"nested-project"}}),
            "nested-project",
        ),
        (
            json!({"antigravityProjectId":null,"projectId":"valid-project"}),
            "valid-project",
        ),
        (
            json!({"cloudaicompanionProject":"","projectId":"valid-project"}),
            "valid-project",
        ),
        (
            json!({"projectId":"bad\nid","backendProjectId":"safe-project"}),
            "safe-project",
        ),
        (
            json!({"projects":[{}, {"projectId":false}, "", {"project":{"id":"listed-project"}}]}),
            "listed-project",
        ),
        (json!({"projectIds":["", "listed-string"]}), "listed-string"),
        (
            json!({"cloudaicompanionProjects":[{"cloudaicompanionProject":{"id":"listed-nested"}}]}),
            "listed-nested",
        ),
    ];
    for (index, (report, expected)) in reports.into_iter().enumerate() {
        let app = Router::new().route(
            "/v1internal:loadCodeAssist",
            post(move || {
                let report = report.clone();
                async move { Json(report) }
            }),
        );
        // No list route: an already-resolved project must not trigger another request.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut definition = super::super::agy_provider("fixture", "fixture");
        definition.base_url = format!("http://{address}");
        let provider = ProviderRuntime::new(definition, &|_| None).unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer report-{index}").parse().unwrap(),
        );
        assert_eq!(
            project_id(&reqwest::Client::new(), &provider, &headers)
                .await
                .unwrap(),
            expected
        );
        server.abort();
    }
}

#[tokio::test]
async fn project_discovery_failures_stay_errors_and_are_not_cached() {
    use axum::{Json, Router, http::StatusCode, routing::post};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    for (index, (load_status, load_body, list_status, expected_error, list_calls)) in [
        (
            StatusCode::FORBIDDEN,
            json!({"error":"private-upstream-body"}),
            StatusCode::OK,
            "AGY loadCodeAssist returned 403",
            0,
        ),
        (
            StatusCode::OK,
            json!([]),
            StatusCode::OK,
            "invalid AGY loadCodeAssist project response",
            0,
        ),
        (
            StatusCode::OK,
            json!({}),
            StatusCode::UNAUTHORIZED,
            "AGY listCloudAICompanionProjects returned 401",
            1,
        ),
        (
            StatusCode::OK,
            json!({}),
            StatusCode::OK,
            "AGY account has no Cloud Code project",
            1,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let loads = Arc::new(AtomicUsize::new(0));
        let lists = Arc::new(AtomicUsize::new(0));
        let load_count = loads.clone();
        let list_count = lists.clone();
        let app = Router::new()
            .route(
                "/v1internal:loadCodeAssist",
                post(move || {
                    let count = load_count.clone();
                    let body = load_body.clone();
                    async move {
                        if count.fetch_add(1, Ordering::SeqCst) == 0 {
                            (load_status, Json(body))
                        } else {
                            (
                                StatusCode::OK,
                                Json(json!({"projectId":"recovered-project"})),
                            )
                        }
                    }
                }),
            )
            .route(
                "/v1internal:listCloudAICompanionProjects",
                post(move || {
                    let count = list_count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        (
                            list_status,
                            Json(json!({"projects":[], "error":"private-upstream-body"})),
                        )
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut definition = super::super::agy_provider("fixture", "fixture");
        definition.base_url = format!("http://{address}");
        let provider = ProviderRuntime::new(definition, &|_| None).unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer retry-{index}").parse().unwrap(),
        );
        let client = reqwest::Client::new();
        let error = project_id(&client, &provider, &headers)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected_error), "{error}");
        assert!(!error.contains("private-upstream-body"));
        assert_eq!(
            project_id(&client, &provider, &headers).await.unwrap(),
            "recovered-project"
        );
        assert_eq!(loads.load(Ordering::SeqCst), 2);
        assert_eq!(lists.load(Ordering::SeqCst), list_calls);
        server.abort();
    }
}

#[test]
fn native_search_membership() {
    for (ids, expected) in [
        (json!(["search-only"]), Some(false)),
        (json!(["agent"]), Some(true)),
        (json!([]), Some(false)),
        (Value::Null, None),
    ] {
        let models = parse_models(&json!({
            "models":{"agent":{}, "explicit":{"supports_web_search":true}},
            "webSearchModelIds":ids
        }))
        .unwrap();
        assert_eq!(
            models
                .iter()
                .find(|model| model.id == "agent")
                .unwrap()
                .supports_web_search,
            expected
        );
        assert_eq!(
            models
                .iter()
                .find(|model| model.id == "explicit")
                .unwrap()
                .supports_web_search,
            Some(true)
        );
    }
    assert!(parse_models(&json!({"models":{}, "webSearchModelIds":[123]})).is_err());
}

#[test]
fn quota_labels_are_compact() {
    let windows = parse_quota(&json!({"groups":[
        {"name":"Claude and GPT models","buckets":[
            {"name":"Five Hour Limit Remaining","remaining_fraction":1.0},
            {"name":"Weekly Limit Remaining","remaining_fraction":0.97}
        ]},
        {"name":"Gemini Models","buckets":[
            {"name":"Five Hour Limit Remaining","remaining_fraction":0.83},
            {"name":"Weekly Limit Remaining","remaining_fraction":0.94}
        ]},
        {"name":"Future family","buckets":[{"name":"Daily Limit Remaining","remaining_fraction":0.5}]}
    ]}))
    .unwrap();
    let mut labels = windows
        .iter()
        .map(|window| window.label.as_str())
        .collect::<Vec<_>>();
    labels.sort_unstable();
    assert_eq!(
        labels,
        [
            "Claude 5h",
            "Claude 7d",
            "Future family Daily",
            "Gemini 5h",
            "Gemini 7d"
        ]
    );
}
