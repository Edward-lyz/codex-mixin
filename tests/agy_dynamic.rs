use codex_mixin::provider::{
    ProviderModel, ProviderModelSource, agy_provider, apply_discovered_models,
};

fn model(id: &str) -> ProviderModel {
    ProviderModel {
        id: id.to_owned(),
        display_name: Some(id.to_owned()),
        ..Default::default()
    }
}

#[test]
fn agy_config_remains_legacy_readable() {
    #[derive(serde::Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum LegacySource {
        OpenAiCompatible { path: String },
        BaiduOneApi,
        AwsBedrock,
        Static,
    }
    let provider = agy_provider("agy", "");
    let stored = serde_json::to_value(&provider).unwrap();
    let source = serde_json::from_value::<LegacySource>(stored["model_source"].clone())
        .expect("AGY configuration must remain readable by the installed CLI");
    match source {
        LegacySource::Static => {}
        LegacySource::OpenAiCompatible { path } => panic!("unexpected HTTP source {path}"),
        _ => panic!("unexpected model source"),
    }
}

#[test]
fn transient_source_is_read_but_not_written() {
    let source: ProviderModelSource =
        serde_json::from_value(serde_json::json!({"kind":"antigravity"})).unwrap();
    assert_eq!(
        serde_json::to_value(source).unwrap(),
        serde_json::json!({"kind":"static"})
    );
}

#[test]
fn config_load_preserves_refresh_and_selection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let mut provider = agy_provider("agy", "");
    provider.cached_models = vec![model("current")];
    provider.selected_models = vec!["current".to_owned(), "unavailable".to_owned()];
    provider.models_refreshed_at_ms = Some(12345);
    let config = codex_mixin::config::StoredGatewayConfig {
        providers: vec![provider.clone()],
        ..Default::default()
    };
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    for _ in 0..2 {
        let loaded = codex_mixin::config::load_stored_config_from_path(&path)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.providers[0].models_refreshed_at_ms, Some(12345));
        assert_eq!(
            loaded.providers[0].selected_models,
            provider.selected_models
        );
        assert_eq!(loaded.providers[0].cached_models.len(), 1);
    }
}

#[test]
fn agy_requires_live_discovery() {
    let provider = agy_provider("agy", "");
    assert_eq!(provider.model_source, ProviderModelSource::Static);
    assert!(
        provider.cached_models.is_empty(),
        "static seeds must not impersonate discovery"
    );
}

#[test]
fn agy_preserves_selection_intent() {
    let mut provider = agy_provider("agy", "");
    provider.cached_models = vec![model("kept"), model("gone")];
    provider.selected_models = vec!["kept".to_owned(), "gone".to_owned()];
    provider.models_refreshed_at_ms = Some(1);
    let changes =
        apply_discovered_models(&mut provider, vec![model("kept"), model("new")]).unwrap();
    assert_eq!(provider.selected_models, ["kept", "gone"]);
    assert_eq!(changes.added, ["new"]);
    assert_eq!(changes.removed, ["gone"]);
    assert!(changes.auto_selected.is_empty());
    apply_discovered_models(&mut provider, vec![model("kept"), model("gone")]).unwrap();
    assert!(provider.selects_model("gone"));
}

#[test]
fn empty_catalog_stops_routing() {
    let mut provider = agy_provider("agy", "");
    provider.cached_models = vec![model("gone")];
    provider.selected_models = vec!["gone".to_owned()];
    provider.models_refreshed_at_ms = Some(1);
    apply_discovered_models(&mut provider, vec![]).unwrap();
    assert_eq!(provider.selected_models, ["gone"]);
    assert_eq!(
        codex_mixin::provider::ProviderRegistry::new(vec![provider])
            .unwrap()
            .routable_models()
            .count(),
        0
    );
}

#[test]
fn model_rename_keeps_identity() {
    let mut provider = agy_provider("agy", "");
    provider.cached_models = vec![model("kept")];
    provider.selected_models = vec!["kept".to_owned()];
    let mut updated = model("kept");
    updated.display_name = Some("New display name".to_owned());
    let changes = apply_discovered_models(&mut provider, vec![updated]).unwrap();
    assert_eq!(changes.updated, ["kept"]);
    assert_eq!(provider.selected_models, ["kept"]);
    assert!(changes.removed.is_empty());
}

#[tokio::test]
async fn explicit_keys_use_remote_models() {
    use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    let reply = Arc::new(Mutex::new(
        json!({"models":{"future-id":{"displayName":"Future model"}}}),
    ));
    async fn models(State(reply): State<Arc<Mutex<Value>>>, headers: HeaderMap) -> Json<Value> {
        assert_eq!(headers["authorization"], "Bearer configured-key");
        Json(reply.lock().unwrap().clone())
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/v1internal:fetchAvailableModels", post(models))
        .with_state(reply.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut provider = agy_provider("agy", "configured-key");
    provider.base_url = format!("http://{address}");
    let client = reqwest::Client::new();
    let models = codex_mixin::provider::discover_provider_models(&client, &provider)
        .await
        .unwrap();
    assert_eq!(models[0].id, "future-id");
    *reply.lock().unwrap() = json!({"models":null});
    assert!(
        codex_mixin::provider::discover_provider_models(&client, &provider)
            .await
            .is_err()
    );
    *reply.lock().unwrap() = json!({"models":{}});
    assert!(
        codex_mixin::provider::discover_provider_models(&client, &provider)
            .await
            .unwrap()
            .is_empty()
    );
    server.abort();
}

#[tokio::test]
async fn quota_cli_returns_dynamic_rows() {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use serde_json::{Value, json};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route("/v1internal:retrieveUserQuotaSummary", post(|headers: HeaderMap| async move {
        assert_eq!(headers["authorization"], "Bearer configured-key");
        Json(json!({"groups":[{"displayName":"Future family","buckets":[
            {"bucketId":"future.daily","displayName":"Daily","remainingFraction":0.42,"resetTime":"2026-10-16T00:00:00Z"}
        ]}]}))
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let output = tokio::task::spawn_blocking(move || {
        let directory = tempfile::tempdir().unwrap();
        let mut provider = agy_provider("agy", "configured-key");
        provider.base_url = format!("http://{address}");
        let config = codex_mixin::config::StoredGatewayConfig {
            providers: vec![provider],
            ..Default::default()
        };
        let path = directory.path().join("config.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_codex-mixin"));
        codex_mixin::platform::set_home_env(&mut command, directory.path());
        // Windows needs its system ACL tool even when AGY CLI is absent.
        #[cfg(windows)]
        let quota_path =
            std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        #[cfg(not(windows))]
        let quota_path = "";
        command
            .args(["--no-tui", "quota", "--provider", "agy", "--json"])
            .env("CODEX_GATEWAY_CONFIG", path)
            .env("CODEX_HOME", directory.path().join("codex-home"))
            .env("PATH", quota_path)
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
    let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows[0]["quota_id"], "future.daily");
    assert_eq!(rows[0]["remaining"], 42.0);
    assert_eq!(rows[0]["unit"], "percent");
    assert_eq!(rows[0]["source"], "agy_oauth");
    assert_eq!(rows[0]["reset_at"], "2026-10-16T00:00:00Z");
    server.abort();
}

#[cfg(unix)]
#[test]
fn failed_refresh_retains_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("agy");
    std::fs::write(&executable, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 1.3.5; exit 0; fi\nprintf '%s' '{\"status\":\"FAILURE\"}'\n").unwrap();
    codex_mixin::platform::make_private_executable(&executable).unwrap();
    let mut provider = agy_provider("agy", "");
    provider.cached_models = vec![model("kept")];
    provider.selected_models = vec!["kept".to_owned()];
    provider.models_refreshed_at_ms = Some(12345);
    let config = codex_mixin::config::StoredGatewayConfig {
        providers: vec![provider],
        ..Default::default()
    };
    let config_path = directory.path().join("config.json");
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut paths = vec![directory.path().to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_codex-mixin"));
    codex_mixin::platform::set_home_env(&mut command, directory.path());
    let output = command
        .args(["--no-tui", "providers", "discover", "agy"])
        .env("CODEX_GATEWAY_CONFIG", &config_path)
        .env("CODEX_HOME", directory.path().join("codex-home"))
        .env("PATH", std::env::join_paths(paths).unwrap())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let loaded = codex_mixin::config::load_stored_config_from_path(&config_path)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.providers[0].cached_models[0].id, "kept");
    assert_eq!(loaded.providers[0].selected_models, ["kept"]);
    assert_eq!(loaded.providers[0].models_refreshed_at_ms, Some(12345));
    assert!(loaded.providers[0].models_refresh_error.is_some());
}

#[tokio::test]
#[ignore = "requires saved AGY OAuth credentials; direct read-only HTTP, no inference"]
async fn live_agy_account_reports() {
    let provider = agy_provider("agy", "");
    let models =
        codex_mixin::provider::discover_provider_models(&reqwest::Client::new(), &provider)
            .await
            .unwrap();
    let windows = codex_mixin::provider::fetch_agy_quota(&provider)
        .await
        .unwrap();
    assert!(!models.is_empty());
    assert!(!windows.is_empty());
    println!(
        "AGY native reports: {} models, {} measured quota windows",
        models.len(),
        windows.len()
    );
}
