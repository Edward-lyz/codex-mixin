use std::fs;
use std::process::Stdio;
use std::time::Duration;

use codex_mixin::config::{
    StoredGatewayConfig, load_stored_config_from_path, save_stored_config_to_path,
};
use codex_mixin::provider::{CONFIG_VERSION, ProviderModel, ProviderModelSource, custom_provider};
use serde::Deserialize;
use wait_timeout::ChildExt;

mod common;

// Windows startup also runs ACL subprocesses; network independence is checked
// by keeping the catalog connection open until the gateway serves healthz.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const DAEMON_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

struct DaemonGuard<'a>(&'a std::path::Path);

impl Drop for DaemonGuard<'_> {
    fn drop(&mut self) {
        let stopped = common::cli(self.0)
            .args(["stop", "--allow-codex-disconnect"])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .and_then(
                |mut child| match child.wait_timeout(Duration::from_secs(15))? {
                    Some(status) => Ok(status),
                    None => {
                        child.kill()?;
                        child.wait()?;
                        Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "test daemon cleanup timed out",
                        ))
                    }
                },
            );
        if !stopped
            .as_ref()
            .is_ok_and(std::process::ExitStatus::success)
        {
            eprintln!(
                "failed to stop test daemon in {}: {stopped:?}",
                self.0.display()
            );
            assert!(std::thread::panicking(), "test daemon cleanup failed");
        }
    }
}

#[derive(Deserialize)]
struct RuntimeMetadata {
    bind: String,
}

async fn daemon_control(root: &std::path::Path, arguments: &[&str]) -> std::process::Output {
    let stdout = root.join("daemon-control.stdout");
    let stderr = root.join("daemon-control.stderr");
    // A detached Windows child can retain inherited pipe handles. Wait for
    // the control process itself, with logs in files instead of waiting for EOF.
    let mut child = common::tokio_cli(root)
        .args(arguments)
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let status = tokio::time::timeout(DAEMON_COMMAND_TIMEOUT, child.wait())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "daemon control timed out: {arguments:?}; {}",
                fs::read_to_string(&stderr).unwrap()
            )
        })
        .unwrap();
    std::process::Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    }
}

async fn wait_for_runtime(
    path: &std::path::Path,
    child: &mut tokio::process::Child,
) -> RuntimeMetadata {
    let runtime = tokio::time::timeout(STARTUP_TIMEOUT, async {
        loop {
            if let Ok(raw) = fs::read(path)
                && let Ok(runtime) = serde_json::from_slice::<RuntimeMetadata>(&raw)
            {
                break runtime;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    match runtime {
        Ok(runtime) => runtime,
        Err(error) => {
            let status = child.try_wait().expect("inspect gateway process status");
            panic!("gateway did not publish runtime metadata: {error}; child status: {status:?}")
        }
    }
}

#[tokio::test]
async fn daemon_upgrade_keeps_bind_mode() {
    enum Replacement {
        Outdated,
        ConfigChanged,
        MissingRuntime,
        ForegroundChanged,
    }
    for (reason, explicit) in [
        (Replacement::Outdated, false),
        (Replacement::Outdated, true),
        (Replacement::ConfigChanged, false),
        (Replacement::MissingRuntime, false),
        (Replacement::ForegroundChanged, false),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let config_path = root.join("config.json");
        let runtime_path = root.join("runtime.json");
        let codex_home = root.join("codex");
        fs::create_dir_all(&codex_home).unwrap();
        let mut provider = custom_provider("static", "upstream-key");
        provider.base_url = "http://127.0.0.1:9".to_owned();
        provider.model_source = ProviderModelSource::Static;
        provider.selected_models = vec!["test-model".to_owned()];
        provider.cached_models = vec![ProviderModel {
            id: "test-model".to_owned(),
            ..Default::default()
        }];
        save_stored_config_to_path(
            &config_path,
            &StoredGatewayConfig {
                providers: vec![provider],
                official_selected_models: Some(Vec::new()),
                ..Default::default()
            },
        )
        .unwrap();
        let _cleanup = DaemonGuard(root);
        let started = daemon_control(root, &["start", "--daemon", "--bind", "127.0.0.1:0"]).await;
        assert!(
            started.status.success(),
            "{}",
            String::from_utf8_lossy(&started.stderr)
        );
        let mut runtime: serde_json::Value =
            serde_json::from_slice(&fs::read(&runtime_path).unwrap()).unwrap();
        let old_bind = runtime["bind"].as_str().unwrap().to_owned();
        let old_address: std::net::SocketAddr = old_bind.parse().unwrap();
        assert!(!(8000..=9000).contains(&old_address.port()));
        let mut stored = load_stored_config_from_path(&config_path).unwrap().unwrap();
        stored.gateway_bind = Some(old_bind.clone());
        save_stored_config_to_path(&config_path, &stored).unwrap();
        match reason {
            Replacement::Outdated => {
                runtime["version"] = serde_json::json!("0.0.0");
                fs::write(&runtime_path, serde_json::to_vec(&runtime).unwrap()).unwrap();
            }
            Replacement::ConfigChanged => {}
            Replacement::MissingRuntime => fs::remove_file(&runtime_path).unwrap(),
            Replacement::ForegroundChanged => fs::remove_file(root.join("daemon.json")).unwrap(),
        }
        let codex_config = codex_home.join("config.toml");
        fs::write(&codex_config, format!(
            "# codex-mixin managed config\nmodel_provider = 'codex-mixin-custom'\n[model_providers.codex-mixin-custom]\nbase_url = 'http://{old_bind}/v1'\nwire_api = 'responses'\n"
        )).unwrap();
        let mut arguments = vec!["start", "--daemon"];
        if explicit {
            arguments.extend(["--bind", &old_bind]);
        }
        let replaced = daemon_control(root, &arguments).await;
        assert!(
            replaced.status.success(),
            "{}",
            String::from_utf8_lossy(&replaced.stderr)
        );
        let new_runtime: serde_json::Value =
            serde_json::from_slice(&fs::read(&runtime_path).unwrap()).unwrap();
        assert_ne!(new_runtime["pid"], runtime["pid"]);
        let new_bind = new_runtime["bind"].as_str().unwrap();
        let new_address: std::net::SocketAddr = new_bind.parse().unwrap();
        if explicit {
            assert_eq!(new_bind, old_bind);
        } else {
            assert!((8000..=9000).contains(&new_address.port()), "{new_address}");
        }
        let stored = load_stored_config_from_path(&config_path).unwrap().unwrap();
        assert_eq!(stored.gateway_bind.as_deref(), Some(new_bind));
        let client_config = fs::read_to_string(&codex_config).unwrap();
        assert!(
            client_config.contains(&format!("http://{new_bind}/v1")),
            "{client_config}"
        );
    }
}

#[tokio::test]
async fn startup_does_not_wait_for_official_catalog_network() {
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = proxy_listener.local_addr().unwrap();
    let (proxy_connected, proxy_connection) = tokio::sync::oneshot::channel();
    let (proxy_closed, mut proxy_close) = tokio::sync::oneshot::channel();
    let proxy_task = tokio::spawn(async move {
        let (mut connection, _) = proxy_listener.accept().await.unwrap();
        let _ = proxy_connected.send(());
        let closed = tokio::io::copy(&mut connection, &mut tokio::io::sink()).await;
        let _ = proxy_closed.send(closed);
    });

    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let gateway_config_path = directory.path().join("gateway.json");
    let runtime_path = directory.path().join("runtime.json");
    let codex_home = directory.path().join("codex");
    let catalog_path = codex_home.join("model-catalogs").join("mixin-models.json");
    fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
    let mut provider = custom_provider("static", "upstream-key");
    provider.base_url = "http://127.0.0.1:9".to_owned();
    provider.model_source = ProviderModelSource::Static;
    provider.selected_models = vec!["test-model".to_owned()];
    provider.cached_models = vec![ProviderModel {
        id: "test-model".to_owned(),
        ..Default::default()
    }];
    save_stored_config_to_path(
        &gateway_config_path,
        &StoredGatewayConfig {
            context: Default::default(),
            config_version: CONFIG_VERSION,
            gateway_bind: None,
            gateway_api_key: Some("gateway-key".to_owned()),
            cors_allowed_origins: StoredGatewayConfig::default().cors_allowed_origins,
            gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys::default(),
            compaction_secret: None,
            official_ech_fallback_reason: None,
            official_ech_proxy: false,
            official_selected_models: None,
            official_model_contexts: Default::default(),
            fusion_profiles: Vec::new(),
            providers: vec![provider],
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
    let mut child = common::tokio_cli(directory.path())
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
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let runtime = wait_for_runtime(&runtime_path, &mut child).await;
    tokio::time::timeout(STARTUP_TIMEOUT, proxy_connection)
        .await
        .expect("official catalog request did not reach the hanging proxy")
        .unwrap();
    let runtime_bind: std::net::SocketAddr = runtime.bind.parse().unwrap();
    // The automatic default listens on all interfaces; clients dial loopback.
    assert!(runtime_bind.ip().is_unspecified());
    assert!((8000..=9000).contains(&runtime_bind.port()));
    let stored = load_stored_config_from_path(&gateway_config_path)
        .unwrap()
        .unwrap();
    assert_eq!(stored.gateway_bind.as_deref(), Some(runtime.bind.as_str()));
    let response = reqwest::Client::new()
        .get(format!(
            "http://{}/healthz",
            codex_mixin::config::client_address(runtime_bind)
        ))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    let health: serde_json::Value = response.json().await.unwrap();
    assert_eq!(health["ok"], true);
    assert_eq!(health["provider_readiness"], "healthy");
    assert!(
        matches!(
            proxy_close.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "official catalog request finished before gateway readiness"
    );
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
            context: Default::default(),
            config_version: CONFIG_VERSION,
            gateway_bind: None,
            gateway_api_key: Some("gateway-key".to_owned()),
            cors_allowed_origins: StoredGatewayConfig::default().cors_allowed_origins,
            gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys::default(),
            compaction_secret: None,
            official_ech_fallback_reason: None,
            official_ech_proxy: false,
            official_selected_models: Some(Vec::new()),
            official_model_contexts: Default::default(),
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

    let mut child = common::tokio_cli(directory.path())
        .args(["start", "--bind", "127.0.0.1:0"])
        .env("CODEX_GATEWAY_CONFIG", &gateway_config_path)
        .env("CODEX_GATEWAY_RUNTIME_FILE", &runtime_path)
        .env("CODEX_HOME", directory.path().join("codex"))
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let runtime = wait_for_runtime(&runtime_path, &mut child).await;
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
#[cfg(feature = "client-auth")]
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
        cors_allowed_origins: StoredGatewayConfig::default().cors_allowed_origins,
        config_version: CONFIG_VERSION,
        gateway_bind: None,
        gateway_api_key: Some("gateway-key".to_owned()),
        gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys {
            claude: Some(key.to_owned()),
            ..Default::default()
        },
        compaction_secret: None,
        official_ech_fallback_reason: None,
        official_ech_proxy: false,
        official_selected_models: Some(Vec::new()),
        official_model_contexts: Default::default(),
        fusion_profiles: Vec::new(),
        context: Default::default(),
        providers: vec![provider.clone()],
    };
    save_stored_config_to_path(&gateway_config_path, &stored_config("rotation-before")).unwrap();

    let mut child = common::tokio_cli(directory.path())
        .args(["start", "--bind", "127.0.0.1:0"])
        .env("CODEX_GATEWAY_CONFIG", &gateway_config_path)
        .env("CODEX_GATEWAY_RUNTIME_FILE", &runtime_path)
        .env("CODEX_HOME", directory.path().join("codex"))
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let runtime = wait_for_runtime(&runtime_path, &mut child).await;
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
