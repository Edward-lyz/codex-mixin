use std::fs;
use std::process::Command;

use codex_mixin::config::load_stored_config_from_path;

fn add_provider_with_blocked_cache(cache_name: &str) {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("gateway.json");
    fs::create_dir(directory.path().join(cache_name)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_codex-mixin"))
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
