//! A disabled provider must never receive paid requests: selecting or
//! discovering its models may not trigger automatic capability probes.

use std::net::SocketAddr;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::routing::{get, post};
use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
use codex_mixin::provider::{
    CONFIG_VERSION, ProviderDefinition, ProviderModel, custom_provider, openrouter_provider,
};
use serde_json::json;

async fn spawn_paid_upstream() -> (SocketAddr, Arc<AtomicUsize>) {
    let completions = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&completions);
    let app = Router::new()
        .route(
            "/v1/models",
            get(|| async {
                axum::Json(json!({"object":"list","data":[
                    {"id":"old-model"},
                    {"id":"new-expensive-model"}
                ]}))
            }),
        )
        .fallback(post(move || {
            let counted = Arc::clone(&counted);
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                (axum::http::StatusCode::PAYMENT_REQUIRED, "billed")
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, completions)
}

/// Run CLI commands against a config holding only `provider`, and return how
/// many paid completion requests reached its upstream.
async fn paid_requests_after(mut provider: ProviderDefinition, commands: &[&[&str]]) -> usize {
    let (address, completions) = spawn_paid_upstream().await;
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("gateway.json");
    provider.base_url = format!("http://{address}");
    provider.models_refreshed_at_ms = Some(1);
    provider.cached_models = vec![ProviderModel {
        id: "old-model".to_owned(),
        ..ProviderModel::default()
    }];
    provider.selected_models = vec!["old-model".to_owned()];
    save_stored_config_to_path(
        &config_path,
        &StoredGatewayConfig {
            config_version: CONFIG_VERSION,
            providers: vec![provider],
            ..StoredGatewayConfig::default()
        },
    )
    .unwrap();
    for args in commands {
        let binary = env!("CARGO_BIN_EXE_codex-mixin");
        let config = config_path.clone();
        let codex_home = directory.path().join("codex");
        let home = directory.path().join("home");
        let args = args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let output = tokio::task::spawn_blocking(move || {
            Command::new(binary)
                .arg("--no-tui")
                .args(&args)
                .env("CODEX_GATEWAY_CONFIG", config)
                .env("CODEX_HOME", codex_home)
                .env("HOME", home)
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    completions.load(Ordering::SeqCst)
}

#[tokio::test(flavor = "multi_thread")]
async fn providers_that_declare_capabilities_are_never_probed() {
    let mut provider = openrouter_provider("openrouter", "upstream-key");
    provider.enabled = true;
    let requests = paid_requests_after(
        provider,
        &[
            &["providers", "discover", "openrouter"],
            &[
                "providers",
                "select",
                "openrouter",
                "--model",
                "new-expensive-model",
            ],
            &["providers", "probe", "openrouter"],
        ],
    )
    .await;
    assert_eq!(requests, 0, "a declared-capability provider was probed");
}

#[tokio::test(flavor = "multi_thread")]
async fn other_enabled_providers_still_probe_new_models() {
    let mut provider = custom_provider("custom", "upstream-key");
    provider.enabled = true;
    let requests = paid_requests_after(
        provider,
        &[&[
            "providers",
            "select",
            "custom",
            "--model",
            "new-expensive-model",
        ]],
    )
    .await;
    assert!(
        requests > 0,
        "probing must still cover providers without a capability catalog"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_provider_models_are_never_probed_automatically() {
    let (address, completions) = spawn_paid_upstream().await;
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("gateway.json");
    let mut provider = custom_provider("openrouter", "upstream-key");
    provider.base_url = format!("http://{address}");
    provider.enabled = false;
    provider.models_refreshed_at_ms = Some(1);
    provider.cached_models = vec![ProviderModel {
        id: "old-model".to_owned(),
        ..ProviderModel::default()
    }];
    provider.selected_models = vec!["old-model".to_owned()];
    save_stored_config_to_path(
        &config_path,
        &StoredGatewayConfig {
            config_version: CONFIG_VERSION,
            providers: vec![provider],
            ..StoredGatewayConfig::default()
        },
    )
    .unwrap();

    for args in [
        &["providers", "discover", "openrouter"][..],
        &[
            "providers",
            "select",
            "openrouter",
            "--model",
            "new-expensive-model",
        ][..],
    ] {
        let binary = env!("CARGO_BIN_EXE_codex-mixin");
        let config = config_path.clone();
        let codex_home = directory.path().join("codex");
        let home = directory.path().join("home");
        let args = args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let output = tokio::task::spawn_blocking(move || {
            Command::new(binary)
                .arg("--no-tui")
                .args(&args)
                .env("CODEX_GATEWAY_CONFIG", config)
                .env("CODEX_HOME", codex_home)
                .env("HOME", home)
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    assert_eq!(
        completions.load(Ordering::SeqCst),
        0,
        "a disabled provider received paid completion requests"
    );
}
