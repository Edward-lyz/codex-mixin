use std::fs;
use std::process::Command;

use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
use codex_mixin::provider::{CONFIG_VERSION, ProviderModel, ProviderModelSource, custom_provider};

#[test]
fn info_and_provider_list_ignore_corrupted_managed_client_config() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("gateway.json");
    let codex_home = directory.path().join("codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(
        codex_home.join("config.toml"),
        "# codex-mixin managed config.\n[model_providers",
    )
    .unwrap();
    let mut provider = custom_provider("static", "upstream-key");
    provider.base_url = "http://127.0.0.1:9".to_owned();
    provider.model_source = ProviderModelSource::Static;
    provider.selected_models = vec!["model".to_owned()];
    provider.cached_models = vec![ProviderModel {
        id: "model".to_owned(),
        ..ProviderModel::default()
    }];
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
        &["info", "--json"][..],
        &["providers", "list", "--json"][..],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_codex-mixin"))
            .args(args)
            .env("CODEX_GATEWAY_CONFIG", &config_path)
            .env("CODEX_HOME", &codex_home)
            .env("HOME", directory.path().join("home"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{:?} failed because of an unrelated client config: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    }
}
