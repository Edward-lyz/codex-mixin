use std::fs;

use codex_mixin::config::{
    StoredGatewayConfig, load_stored_config_from_path, save_stored_config_to_path,
};
use codex_mixin::provider::baidu_oneapi_provider;

mod common;

#[test]
fn provider_update_expands_panel_secret_references() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.json");
    let mut provider = baidu_oneapi_provider("baidu-oneapi", "original-key");
    provider.quota_username = Some("test-user".to_owned());
    save_stored_config_to_path(
        &config_path,
        &StoredGatewayConfig {
            providers: vec![provider],
            ..StoredGatewayConfig::default()
        },
    )
    .unwrap();

    for key_args in [
        vec!["--key=@env:CODEX_MIXIN_PANEL_SECRET_0"],
        vec!["--key", "@env:CODEX_MIXIN_PANEL_SECRET_0"],
    ] {
        let secret = "test-key=with spaces=@env:literal";
        let output = common::cli(directory.path())
            .args([
                "--no-tui",
                "--json-errors",
                "providers",
                "update",
                "baidu-oneapi",
            ])
            .args(key_args)
            .env("CODEX_MIXIN_PANEL_SECRET_0", secret)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
        let stored = load_stored_config_from_path(&config_path).unwrap().unwrap();
        assert_eq!(stored.providers[0].auth.api_key, secret);
    }
}

#[test]
fn unresolved_panel_secret_fails_before_commit() {
    let directory = tempfile::tempdir().unwrap();
    for reference in ["--key=@env:CODEX_MIXIN_PANEL_SECRET_0", "--key=@env:"] {
        let output = common::cli(directory.path())
            .args([
                "--no-tui",
                "--json-errors",
                "providers",
                "add",
                "--preset=baidu-oneapi",
                "--quota-username=test-user",
                reference,
            ])
            .env_remove("CODEX_MIXIN_PANEL_SECRET_0")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let envelope: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(envelope["error"]["code"], "invalid_arguments");
        assert_eq!(envelope["error"]["committed"], false);
        assert!(!directory.path().join("config.json").exists());
    }
}

fn add_provider_with_blocked_cache(cache_name: &str) {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("gateway.json");
    fs::create_dir(directory.path().join(cache_name)).unwrap();

    let output = common::cli(directory.path())
        .args([
            "providers",
            "add",
            "--preset",
            "custom",
            "--id",
            "review",
            "--key",
            "upstream-key",
            "--base-url",
            "http://127.0.0.1:9",
            "--model",
            "static-model",
        ])
        .env("CODEX_GATEWAY_CONFIG", &config_path)
        .env("CODEX_HOME", directory.path().join("codex"))
        .env("HOME", directory.path().join("home"))
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("configuration was saved"), "{stderr}");
    assert!(stderr.contains("cache invalidation"), "{stderr}");
    let stored = load_stored_config_from_path(&config_path).unwrap().unwrap();
    assert_eq!(stored.providers.len(), 1);
    assert_eq!(stored.providers[0].id, "review");
}

#[test]
fn add_reports_web_search_cache_failure_after_commit() {
    add_provider_with_blocked_cache("web-search-capabilities.json");
}

#[test]
fn add_reports_provider_cache_failure_after_commit() {
    add_provider_with_blocked_cache("provider-capabilities.json");
}
