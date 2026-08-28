use std::fs;
use std::process::Stdio;
use std::time::Duration;

use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
use codex_mixin::provider::{CONFIG_VERSION, open_code_go_provider};
use serde::Deserialize;

#[derive(Deserialize)]
struct RuntimeMetadata {
    bind: String,
}

#[tokio::test]
async fn startup_does_not_wait_for_official_catalog_network() {
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = proxy_listener.local_addr().unwrap();
    let (proxy_connected, proxy_connection) = tokio::sync::oneshot::channel();
    let proxy_task = tokio::spawn(async move {
        let (_connection, _) = proxy_listener.accept().await.unwrap();
        let _ = proxy_connected.send(());
        std::future::pending::<()>().await;
    });

    let directory = tempfile::tempdir().unwrap();
    let gateway_config_path = directory.path().join("gateway.json");
    let runtime_path = directory.path().join("runtime.json");
    let codex_home = directory.path().join("codex");
    let catalog_path = codex_home.join("model-catalogs").join("mixin-models.json");
    fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
    save_stored_config_to_path(
        &gateway_config_path,
        &StoredGatewayConfig {
            config_version: CONFIG_VERSION,
            gateway_bind: None,
            gateway_api_key: Some("gateway-key".to_owned()),
            gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys::default(),
            compaction_secret: None,
            official_selected_models: None,
            fusion_profiles: Vec::new(),
            providers: vec![open_code_go_provider("test-provider", "upstream-key")],
        },
    )
    .unwrap();
    fs::write(
        codex_home.join("config.toml"),
        format!(
            "# codex-mixin managed config. Run `codex-mixin uninstall-codex` to restore the previous config.\nmodel_catalog_json = {:?}\n\n[model_providers.codex-mixin]\nrequires_openai_auth = true\nsupports_websockets = true\n",
            catalog_path.to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(&catalog_path, r#"{"models":[{"slug":"gpt-5.6-sol"}]}"#).unwrap();
    fs::write(
        codex_home.join("models_cache.json"),
        r#"{"client_version":"0.147.0","models":[{"slug":"gpt-5.6-sol"}]}"#,
    )
    .unwrap();
    fs::write(
        codex_home.join("auth.json"),
        r#"{"tokens":{"access_token":"official-token","account_id":"account-1"}}"#,
    )
    .unwrap();

    let proxy_url = format!("http://{proxy_address}");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_codex-mixin"))
        .args(["start", "--bind", "127.0.0.1:0"])
        .env("CODEX_GATEWAY_CONFIG", &gateway_config_path)
        .env("CODEX_GATEWAY_RUNTIME_FILE", &runtime_path)
        .env("CODEX_HOME", &codex_home)
        .env("HTTPS_PROXY", &proxy_url)
        .env("https_proxy", &proxy_url)
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    tokio::time::timeout(Duration::from_secs(3), proxy_connection)
        .await
        .expect("official catalog request did not reach the hanging proxy")
        .unwrap();
    let runtime = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(raw) = fs::read(&runtime_path)
                && let Ok(runtime) = serde_json::from_slice::<RuntimeMetadata>(&raw)
            {
                break runtime;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("gateway did not publish runtime metadata while catalog refresh was blocked");
    let response = reqwest::Client::new()
        .get(format!("http://{}/healthz", runtime.bind))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    let health: serde_json::Value = response.json().await.unwrap();
    assert_eq!(health["ok"], true);
    assert_eq!(health["provider_readiness"], "healthy");
    assert!(child.try_wait().unwrap().is_none());
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    proxy_task.abort();
}
