//! Provider model refreshes must keep the new catalog even when a saved route
//! references a model that disappeared upstream. The route is disabled with
//! its configuration intact and can be enabled again after the model returns.

use std::net::SocketAddr;
use std::path::Path;
use std::process::Output;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::routing::get;
use codex_mixin::config::{
    FailoverRoute, FusionMode, FusionProfile, PanelToolsConfig, StoredGatewayConfig,
    load_stored_config_from_path, save_stored_config_to_path,
};
use codex_mixin::provider::{ProviderModel, ProviderPreset};
use serde_json::{Value, json};

mod common;

const ROUTE: &str = "auto/panel-route";
const MISSING_MODEL: &str = "removed-openrouter";

#[tokio::test(flavor = "multi_thread")]
async fn refresh_disables_routes_with_removed_models_and_preserves_configuration() {
    let models = Arc::new(Mutex::new(vec![
        "panel".to_owned(),
        "judge".to_owned(),
        "final".to_owned(),
    ]));
    let address = spawn_models_upstream(Arc::clone(&models)).await;
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("gateway.json");
    let mut provider = ProviderPreset::OpenRouter.create("openrouter", "upstream-key");
    provider.base_url = format!("http://{address}");
    provider.models_refreshed_at_ms = Some(1);
    provider.cached_models = provider_models(&["panel", "removed", "judge", "final"]);
    provider.selected_models = ["panel", "removed", "judge", "final"]
        .map(str::to_owned)
        .to_vec();
    save_stored_config_to_path(
        &config_path,
        &StoredGatewayConfig {
            providers: vec![provider],
            fusion_profiles: vec![fusion_profile()],
            ..StoredGatewayConfig::default()
        },
    )
    .unwrap();

    let discover = run_cli(
        directory.path(),
        &config_path,
        &["--no-tui", "providers", "discover", "openrouter"],
    )
    .await;
    assert!(
        discover.status.success(),
        "{}",
        String::from_utf8_lossy(&discover.stderr)
    );

    let stored = load_stored_config_from_path(&config_path).unwrap().unwrap();
    assert_eq!(stored.disabled_routes, vec![ROUTE.to_owned()]);
    assert!(
        stored.providers[0]
            .cached_models
            .iter()
            .all(|model| model.id != "removed")
    );
    assert_eq!(
        stored.fusion_profiles[0].panel_models,
        vec!["panel-openrouter".to_owned(), MISSING_MODEL.to_owned(),]
    );

    let list = run_cli(
        directory.path(),
        &config_path,
        &["--no-tui", "providers", "list"],
    )
    .await;
    let stdout = String::from_utf8_lossy(&list.stdout);
    assert!(
        stdout.contains("model route auto/panel-route is disabled"),
        "{stdout}"
    );
    assert!(stdout.contains(MISSING_MODEL), "{stdout}");

    let routes = run_cli(
        directory.path(),
        &config_path,
        &["--no-tui", "route", "list", "--json"],
    )
    .await;
    assert!(routes.status.success());
    let routes: Value = serde_json::from_slice(&routes.stdout).unwrap();
    assert_eq!(routes["routes"][0]["enabled"], false);
    assert_eq!(routes["routes"][0]["missing_models"][0], MISSING_MODEL);

    let doctor = run_cli(
        directory.path(),
        &config_path,
        &["--no-tui", "doctor", "--quick", "--json"],
    )
    .await;
    assert!(doctor.status.success());
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let route_check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "model_routes")
        .unwrap();
    assert_eq!(route_check["status"], "warning");
    assert!(
        route_check["detail"]
            .as_str()
            .unwrap()
            .contains(MISSING_MODEL)
    );

    let still_missing = run_cli(
        directory.path(),
        &config_path,
        &["--no-tui", "route", "enable", "--model", ROUTE],
    )
    .await;
    assert!(!still_missing.status.success());

    models.lock().unwrap().push("removed".to_owned());
    let discover = run_cli(
        directory.path(),
        &config_path,
        &["--no-tui", "providers", "discover", "openrouter"],
    )
    .await;
    assert!(
        discover.status.success(),
        "{}",
        String::from_utf8_lossy(&discover.stderr)
    );

    let enable = run_cli(
        directory.path(),
        &config_path,
        &["--no-tui", "route", "enable", "--model", ROUTE],
    )
    .await;
    assert!(
        enable.status.success(),
        "{}",
        String::from_utf8_lossy(&enable.stderr)
    );
    let stored = load_stored_config_from_path(&config_path).unwrap().unwrap();
    assert!(stored.disabled_routes.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn failover_route_is_disabled_when_a_fallback_disappears() {
    let models = Arc::new(Mutex::new(vec!["source".to_owned()]));
    let address = spawn_models_upstream(Arc::clone(&models)).await;
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("gateway.json");
    let mut provider = ProviderPreset::OpenRouter.create("openrouter", "upstream-key");
    provider.base_url = format!("http://{address}");
    provider.models_refreshed_at_ms = Some(1);
    provider.cached_models = provider_models(&["source", "fallback"]);
    provider.selected_models = ["source", "fallback"].map(str::to_owned).to_vec();
    save_stored_config_to_path(
        &config_path,
        &StoredGatewayConfig {
            providers: vec![provider],
            failover_routes: vec![FailoverRoute {
                id: Some("failover-route".to_owned()),
                source: "source-openrouter".to_owned(),
                fallbacks: vec!["fallback-openrouter".to_owned()],
            }],
            ..StoredGatewayConfig::default()
        },
    )
    .unwrap();

    let discover = run_cli(
        directory.path(),
        &config_path,
        &["--no-tui", "providers", "discover", "openrouter"],
    )
    .await;
    assert!(
        discover.status.success(),
        "{}",
        String::from_utf8_lossy(&discover.stderr)
    );

    let stored = load_stored_config_from_path(&config_path).unwrap().unwrap();
    assert_eq!(
        stored.disabled_routes,
        vec!["auto/failover-route".to_owned()]
    );
    assert_eq!(
        stored.failover_routes[0].fallbacks,
        vec!["fallback-openrouter".to_owned()]
    );
}

fn fusion_profile() -> FusionProfile {
    FusionProfile {
        id: "panel-route".to_owned(),
        mode: FusionMode::Orchestration,
        time_routes: Vec::new(),
        panel_models: vec!["panel-openrouter".to_owned(), MISSING_MODEL.to_owned()],
        judge_model: "judge-openrouter".to_owned(),
        final_model: "final-openrouter".to_owned(),
        min_successful: 1,
        max_completion_tokens: 1024,
        timeout_ms: 10_000,
        show_intermediate_results: false,
        panel_tools: PanelToolsConfig::default(),
    }
}

fn provider_models(ids: &[&str]) -> Vec<ProviderModel> {
    ids.iter()
        .map(|id| ProviderModel {
            id: (*id).to_owned(),
            ..ProviderModel::default()
        })
        .collect()
}

async fn run_cli(directory: &Path, config_path: &Path, args: &[&str]) -> Output {
    let directory = directory.to_path_buf();
    let config_path = config_path.to_path_buf();
    let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        common::cli(&directory)
            .args(args)
            .env("CODEX_GATEWAY_CONFIG", config_path)
            .output()
            .unwrap()
    })
    .await
    .unwrap()
}

async fn spawn_models_upstream(models: Arc<Mutex<Vec<String>>>) -> SocketAddr {
    let app = Router::new().route(
        "/v1/models",
        get(move || {
            let models = Arc::clone(&models);
            async move {
                let data = models
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|id| json!({"id": id}))
                    .collect::<Vec<_>>();
                axum::Json(json!({"object": "list", "data": data}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}
