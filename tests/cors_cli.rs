use std::path::Path;
use std::process::{Command, Output};

use codex_mixin::config::{
    StoredGatewayConfig, load_stored_config_from_path, save_stored_config_to_path,
};
use serde_json::Value;

const ORIGIN: &str = "http://10.252.145.171:8723";

fn cli(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codex-mixin"))
        .args(["--no-tui", "--json-errors"])
        .args(args)
        .env("HOME", directory.join("home"))
        .env("CODEX_HOME", directory.join("codex"))
        .env("CODEX_GATEWAY_CONFIG", directory.join("config.json"))
        .output()
        .unwrap()
}

fn successful(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cors_cli_default_replace_disable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let value = successful(cli(directory.path(), &["config", "cors", "get", "--json"]));
    assert_eq!(value["cors_allowed_origins"], serde_json::json!([ORIGIN]));
    assert_eq!(value["restart_required"], false);
    assert!(
        !path.exists(),
        "reading defaults must not create configuration"
    );

    let stored = StoredGatewayConfig {
        gateway_api_key: Some("retained-key".to_owned()),
        ..Default::default()
    };
    save_stored_config_to_path(&path, &stored).unwrap();
    let value = successful(cli(
        directory.path(),
        &[
            "config",
            "cors",
            "set",
            "--origin",
            "https://example.com",
            "--origin",
            "http://localhost:8723",
            "--json",
        ],
    ));
    assert_eq!(
        value["cors_allowed_origins"],
        serde_json::json!(["https://example.com", "http://localhost:8723"])
    );
    assert_eq!(value["restart_required"], true);
    let saved = load_stored_config_from_path(&path).unwrap().unwrap();
    assert_eq!(saved.gateway_api_key, stored.gateway_api_key);
    assert_eq!(
        saved.cors_allowed_origins,
        ["https://example.com", "http://localhost:8723"]
    );
    let value = successful(cli(directory.path(), &["config", "cors", "get", "--json"]));
    assert_eq!(
        value["cors_allowed_origins"],
        serde_json::json!(saved.cors_allowed_origins)
    );
    let value = successful(cli(
        directory.path(),
        &["config", "cors", "set", "--disable", "--json"],
    ));
    assert_eq!(value["cors_allowed_origins"], serde_json::json!([]));
    assert!(
        load_stored_config_from_path(&path)
            .unwrap()
            .unwrap()
            .cors_allowed_origins
            .is_empty()
    );
}

#[test]
fn cors_cli_invalid_origins_do_not_commit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    save_stored_config_to_path(&path, &StoredGatewayConfig::default()).unwrap();
    let original = std::fs::read(&path).unwrap();
    for origin in [
        "*",
        "null",
        "http://*.example.com",
        "https://example.com/",
        "https://user:pass@example.com",
        "https://example.com?query",
        "http://example.com:80",
        "https://example.com https://other.example",
    ] {
        let output = cli(
            directory.path(),
            &["config", "cors", "set", "--origin", origin, "--json"],
        );
        assert!(!output.status.success(), "{origin}");
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        let error: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
        assert_eq!(error["protocol_version"], 1);
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("cors_allowed_origins[0]")
        );
        assert_eq!(std::fs::read(&path).unwrap(), original, "{origin}");
    }
}

#[test]
fn cors_cli_requires_explicit_allowlist() {
    let directory = tempfile::tempdir().unwrap();
    for args in [
        vec!["config", "cors", "set", "--json"],
        vec![
            "config",
            "cors",
            "set",
            "--disable",
            "--origin",
            ORIGIN,
            "--json",
        ],
    ] {
        let output = cli(directory.path(), &args);
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        let error: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
        assert_eq!(error["error"]["code"], "invalid_arguments");
        assert_eq!(error["error"]["committed"], false);
    }
    assert!(!directory.path().join("config.json").exists());
}
