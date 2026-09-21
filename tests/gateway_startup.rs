use std::fs;
use std::process::Stdio;
use std::time::Duration;

use codex_mixin::config::{
    StoredGatewayConfig, load_stored_config_from_path, save_stored_config_to_path,
};
use codex_mixin::provider::{
    CONFIG_VERSION, ProviderModel, ProviderModelSource, custom_provider, open_code_go_provider,
};
use serde::Deserialize;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct RuntimeMetadata {
    bind: String,
}

async fn wait_for_runtime(path: &std::path::Path) -> RuntimeMetadata {
    tokio::time::timeout(STARTUP_TIMEOUT, async {
        loop {
            if let Ok(raw) = fs::read(path)
                && let Ok(runtime) = serde_json::from_slice::<RuntimeMetadata>(&raw)
            {
                break runtime;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("gateway did not publish runtime metadata")
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
    let home = directory.path().join("home");
    fs::create_dir_all(&home).unwrap();
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
        .arg("start")
        .env("CODEX_GATEWAY_CONFIG", &gateway_config_path)
        .env("CODEX_GATEWAY_RUNTIME_FILE", &runtime_path)
        .env("CODEX_HOME", &codex_home)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
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

    tokio::time::timeout(STARTUP_TIMEOUT, proxy_connection)
        .await
        .expect("official catalog request did not reach the hanging proxy")
        .unwrap();
    let runtime = wait_for_runtime(&runtime_path).await;
    let runtime_bind: std::net::SocketAddr = runtime.bind.parse().unwrap();
    assert!(runtime_bind.ip().is_loopback());
    assert_ne!(runtime_bind.port(), 0);
    let stored = load_stored_config_from_path(&gateway_config_path)
        .unwrap()
        .unwrap();
    assert_eq!(stored.gateway_bind.as_deref(), Some(runtime.bind.as_str()));
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

    let config = fs::read_to_string(codex_home.join("config.toml")).unwrap();
    assert_eq!(config.matches("# codex-mixin managed config.").count(), 1);
}

#[tokio::test]
async fn first_start_accepts_client_key_created_during_client_sync() {
    let directory = tempfile::tempdir().unwrap();
    let gateway_config_path = directory.path().join("gateway.json");
    let runtime_path = directory.path().join("runtime.json");
    let home = directory.path().join("home");
    let claude_settings = home.join(".claude").join("settings.json");
    fs::create_dir_all(claude_settings.parent().unwrap()).unwrap();

    let mut provider = custom_provider("static", "upstream-key");
    provider.base_url = "http://127.0.0.1:9".to_owned();
    provider.model_source = ProviderModelSource::Static;
    provider.selected_models = vec!["test-model".to_owned()];
    provider.cached_models = vec![ProviderModel {
        id: "test-model".to_owned(),
        ..ProviderModel::default()
    }];
    save_stored_config_to_path(
        &gateway_config_path,
        &StoredGatewayConfig {
            config_version: CONFIG_VERSION,
            gateway_bind: None,
            gateway_api_key: Some("gateway-key".to_owned()),
            gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys::default(),
            compaction_secret: None,
            official_selected_models: Some(Vec::new()),
            fusion_profiles: Vec::new(),
            providers: vec![provider],
        },
    )
    .unwrap();
    fs::write(
        &claude_settings,
        r#"{"codex_mixin_managed":{"marker":"codex-mixin managed Claude Code"}}"#,
    )
    .unwrap();

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_codex-mixin"))
        .args(["start", "--bind", "127.0.0.1:0"])
        .env("CODEX_GATEWAY_CONFIG", &gateway_config_path)
        .env("CODEX_GATEWAY_RUNTIME_FILE", &runtime_path)
        .env("CODEX_HOME", directory.path().join("codex"))
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let runtime = wait_for_runtime(&runtime_path).await;
    let stored = load_stored_config_from_path(&gateway_config_path)
        .unwrap()
        .unwrap();
    let client_key = stored
        .gateway_client_keys
        .claude
        .expect("startup must persist a Claude client key");
    let response = reqwest::Client::new()
        .get(format!("http://{}/v1/models", runtime.bind))
        .bearer_auth(client_key)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

/// A `connect` command runs in its own process, so a key it mints or revokes
/// only reaches a running gateway by reload. Without that reload a client the
/// user just installed authenticates with the fresh key and gets
/// `401 unauthorized` until the gateway restarts.
#[tokio::test]
async fn running_gateway_serves_a_rotated_client_key() {
    let directory = tempfile::tempdir().unwrap();
    let gateway_config_path = directory.path().join("gateway.json");
    let runtime_path = directory.path().join("runtime.json");
    let home = directory.path().join("home");
    fs::create_dir_all(&home).unwrap();

    let mut provider = custom_provider("static", "upstream-key");
    provider.base_url = "http://127.0.0.1:9".to_owned();
    provider.model_source = ProviderModelSource::Static;
    provider.selected_models = vec!["test-model".to_owned()];
    provider.cached_models = vec![ProviderModel {
        id: "test-model".to_owned(),
        ..ProviderModel::default()
    }];
    let stored_config = |key: &str| StoredGatewayConfig {
        config_version: CONFIG_VERSION,
        gateway_bind: None,
        gateway_api_key: Some("gateway-key".to_owned()),
        gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys {
            claude: Some(key.to_owned()),
            ..Default::default()
        },
        compaction_secret: None,
        official_selected_models: Some(Vec::new()),
        fusion_profiles: Vec::new(),
        providers: vec![provider.clone()],
    };
    save_stored_config_to_path(&gateway_config_path, &stored_config("rotation-before")).unwrap();

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_codex-mixin"))
        .args(["start", "--bind", "127.0.0.1:0"])
        .env("CODEX_GATEWAY_CONFIG", &gateway_config_path)
        .env("CODEX_GATEWAY_RUNTIME_FILE", &runtime_path)
        .env("CODEX_HOME", directory.path().join("codex"))
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let runtime = wait_for_runtime(&runtime_path).await;
    let endpoint = format!("http://{}/v1/model-benchmarks", runtime.bind);
    let client = reqwest::Client::new();
    let status = |key: &'static str| {
        let client = client.clone();
        let endpoint = endpoint.clone();
        async move {
            client
                .get(endpoint)
                .bearer_auth(key)
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(status("rotation-before").await, reqwest::StatusCode::OK);

    save_stored_config_to_path(&gateway_config_path, &stored_config("rotation-after")).unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        while status("rotation-after").await != reqwest::StatusCode::OK {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("the running gateway never served the rotated client key");
    assert_eq!(
        status("rotation-before").await,
        reqwest::StatusCode::UNAUTHORIZED
    );
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}
