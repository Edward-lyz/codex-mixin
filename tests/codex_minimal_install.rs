use std::fs;
use std::time::Duration;

use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
use codex_mixin::provider::{ProviderModel, ProviderModelSource, ProviderPreset};

mod common;

#[tokio::test]
async fn install_needs_only_config() {
    for (mode, recovery) in ["--custom-only", "--codex-oauth-proxy"]
        .into_iter()
        .flat_map(|mode| {
            ["normal", "missing-config", "missing-home", "auth-conflict"]
                .map(|recovery| (mode, recovery))
        })
    {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let home = root.join("codex");
        fs::create_dir_all(&home).unwrap();
        let config = home.join("config.toml");
        let original = "# User settings\npersonality = 'precise'\nopenai_base_url = 'https://original.example/v1'\n[model_providers.existing]\nbase_url = 'https://example.test'\n";
        fs::write(&config, original).unwrap();
        let original_auth = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"original-fixture-key"}"#;
        fs::write(home.join("auth.json"), original_auth).unwrap();
        let catalog_path = home.join("model-catalogs/mixin-models.json");
        fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        let original_catalog = r#"{"unrelated":"existing catalog must survive"}"#;
        fs::write(&catalog_path, original_catalog).unwrap();
        fs::write(home.join("models_cache.json"), "not valid JSON").unwrap();
        let sessions = home.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let history = r#"{"type":"session_meta","payload":{"model_provider":"user-provider"}}"#;
        fs::write(sessions.join("history.jsonl"), history).unwrap();
        fs::write(home.join("state_5.sqlite"), "leave this database untouched").unwrap();
        let unavailable_cli = root.join("not-a-codex-cli");
        fs::write(&unavailable_cli, "not an executable").unwrap();
        let mut provider = ProviderPreset::Custom.create("static", "fixture-key");
        provider.base_url = "http://127.0.0.1:9".to_owned();
        provider.model_source = ProviderModelSource::Static;
        provider.selected_models = vec!["test-model".to_owned()];
        provider.cached_models = vec![ProviderModel {
            id: "test-model".to_owned(),
            ..Default::default()
        }];
        save_stored_config_to_path(
            &root.join("config.json"),
            &StoredGatewayConfig {
                gateway_bind: Some("127.0.0.1:8787".to_owned()),
                official_selected_models: Some(Vec::new()),
                providers: vec![provider],
                ..Default::default()
            },
        )
        .unwrap();
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            common::tokio_cli(root)
                .args([
                    "--no-tui",
                    "connect",
                    "codex",
                    mode,
                    "--set-default",
                    "--config",
                ])
                .arg(&config)
                .args(["--base-url", "http://127.0.0.1:8787/v1"])
                .env("CODEX_CLI_PATH", &unavailable_cli)
                .env(
                    "CODEX_GATEWAY_MODEL_METADATA_URL",
                    "http://127.0.0.1:9/models.json",
                )
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let document = fs::read_to_string(&config)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(document["personality"].as_str(), Some("precise"));
        assert_eq!(
            document["model_providers"]["existing"]["base_url"].as_str(),
            Some("https://example.test")
        );
        assert_eq!(document["model"].as_str(), Some("test-model-static"));
        if mode == "--codex-oauth-proxy" {
            assert_eq!(document["model_provider"].as_str(), Some("openai"));
            assert_eq!(
                document["openai_base_url"].as_str(),
                Some("http://127.0.0.1:8787/v1")
            );
            assert!(document["model_providers"].get("openai").is_none());
        } else {
            assert!(document.get("openai_base_url").is_none());
        }
        let catalog = document["model_catalog_json"].as_str().unwrap();
        let catalog: serde_json::Value =
            serde_json::from_slice(&fs::read(catalog).unwrap()).unwrap();
        assert!(
            catalog["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["slug"] == "test-model-static")
        );
        assert_eq!(
            fs::read_to_string(home.join("models_cache.json")).unwrap(),
            "not valid JSON"
        );
        assert_eq!(
            fs::read_to_string(sessions.join("history.jsonl")).unwrap(),
            history
        );
        assert_eq!(
            fs::read_to_string(home.join("state_5.sqlite")).unwrap(),
            "leave this database untouched"
        );
        let installed_config = fs::read(&config).unwrap();
        if recovery == "missing-config" {
            fs::remove_file(&config).unwrap();
        } else if recovery == "missing-home" {
            fs::remove_dir_all(&home).unwrap();
        } else if recovery == "auth-conflict" {
            fs::remove_file(home.join("auth.json")).unwrap();
            fs::create_dir(home.join("auth.json")).unwrap();
            let rejected = common::tokio_cli(root)
                .args(["--no-tui", "uninstall-codex", "--config"])
                .arg(&config)
                .output()
                .await
                .unwrap();
            assert!(!rejected.status.success(), "{mode}: invalid auth must fail");
            assert_eq!(
                fs::read(&config).unwrap(),
                installed_config,
                "{mode}: failed disconnect must leave the managed config intact"
            );
            fs::remove_dir(home.join("auth.json")).unwrap();
        }
        let removed = common::tokio_cli(root)
            .args(["--no-tui", "uninstall-codex", "--config"])
            .arg(&config)
            .env("CODEX_CLI_PATH", &unavailable_cli)
            .output()
            .await
            .unwrap();
        assert!(
            removed.status.success(),
            "{mode}/{recovery}: {}",
            String::from_utf8_lossy(&removed.stderr)
        );
        assert_eq!(fs::read_to_string(&config).unwrap(), original);
        assert_eq!(
            fs::read_to_string(home.join("auth.json")).unwrap(),
            original_auth
        );
        assert_eq!(fs::read_to_string(&catalog_path).unwrap(), original_catalog);
        if recovery != "missing-home" {
            assert_eq!(
                fs::read_to_string(sessions.join("history.jsonl")).unwrap(),
                history
            );
            assert_eq!(
                fs::read_to_string(home.join("state_5.sqlite")).unwrap(),
                "leave this database untouched"
            );
        }
    }
}
