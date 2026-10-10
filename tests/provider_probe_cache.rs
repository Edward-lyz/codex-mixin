use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{Json, Router, http::StatusCode, routing::get};
use codex_mixin::{
    application::provider::{discovery::refresh_provider_models, models::probe_provider_models},
    config::{StoredGatewayConfig, save_stored_config_to_path, stored_config_path},
    provider::{ProviderModel, ProviderProtocol, custom_provider},
};
use serde_json::json;

const CHILD_CASE_ENV: &str = "CODEX_MIXIN_PROBE_CACHE_TEST_CASE";
const MODEL_ID: &str = "probe-cache-regression-model";
const PROVIDER_IDS: [&str; 2] = ["first", "second"];
const REFRESH_ROUNDS: usize = 3;

#[test]
fn refresh_reuses_probe_results() {
    if let Ok(case) = std::env::var(CHILD_CASE_ENV) {
        // A separate process isolates configuration without mutating global env in tests.
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(check_repeated_refresh(&case));
        return;
    }

    for case in ["supported", "indeterminate"] {
        let directory = tempfile::tempdir().unwrap();
        let metadata_path = directory.path().join("metadata.json");
        std::fs::write(&metadata_path, "{}").unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "refresh_reuses_probe_results", "--nocapture"])
            .env(CHILD_CASE_ENV, case)
            .env("CODEX_GATEWAY_CONFIG", directory.path().join("config.json"))
            .env("CODEX_GATEWAY_MODEL_METADATA", metadata_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

async fn check_repeated_refresh(case: &str) {
    let successful_baseline = match case {
        "supported" => true,
        "indeterminate" => false,
        _ => panic!("unknown test case: {case}"),
    };
    let completions = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&completions);
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"data": [{"id": MODEL_ID}]})) }),
        )
        .route(
            "/v1/chat/completions",
            axum::routing::post(move |Json(body): Json<serde_json::Value>| {
                let counted = Arc::clone(&counted);
                async move {
                    assert_eq!(body["model"], MODEL_ID);
                    counted.fetch_add(1, Ordering::SeqCst);
                    if !successful_baseline {
                        return (
                            StatusCode::TOO_MANY_REQUESTS,
                            [("content-type", "application/json")],
                            "{\"error\":{\"message\":\"rate limited\"}}",
                        );
                    }
                    (
                        StatusCode::OK,
                        [("content-type", "text/event-stream")],
                        "data: {\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\n\ndata: [DONE]\n\n",
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let providers = PROVIDER_IDS
        .map(|id| {
            let mut provider = custom_provider(id, "local-test-key");
            provider.base_url = format!("http://{address}");
            provider.protocol = ProviderProtocol::OpenAiChat;
            provider.api_path = "/v1/chat/completions".to_owned();
            // Keep quota discovery out of this test's inference count.
            provider.quota_url = Some(format!("http://{address}/quota"));
            provider.cached_models = vec![ProviderModel {
                id: MODEL_ID.to_owned(),
                ..ProviderModel::default()
            }];
            provider.selected_models = vec![MODEL_ID.to_owned()];
            provider.models_refreshed_at_ms = Some(1);
            provider
        })
        .to_vec();
    let config_path = stored_config_path();
    save_stored_config_to_path(
        &config_path,
        &StoredGatewayConfig {
            providers,
            ..StoredGatewayConfig::default()
        },
    )
    .unwrap();

    let mut cumulative_requests = Vec::new();
    for _ in 0..REFRESH_ROUNDS {
        for id in PROVIDER_IDS {
            refresh_provider_models(id).await.unwrap();
            probe_provider_models(id, None, true, false).await.unwrap();
        }
        cumulative_requests.push(completions.load(Ordering::SeqCst));
    }
    server.abort();
    let expected_requests = PROVIDER_IDS.len() * if successful_baseline { 6 } else { 1 };
    assert_eq!(
        cumulative_requests,
        vec![expected_requests; REFRESH_ROUNDS],
        "unchanged providers must not repeat inference probes after the first refresh"
    );
    let cache: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config_path.with_file_name("provider-capabilities.json")).unwrap(),
    )
    .unwrap();
    for id in PROVIDER_IDS {
        assert!(cache["providers"][id]["models"][MODEL_ID].is_object());
    }
}
