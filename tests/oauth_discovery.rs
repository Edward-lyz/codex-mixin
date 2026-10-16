mod common;

use serde_json::{Value, json};

#[test]
fn oauth_is_an_add_preset_with_a_bounded_timeout() {
    let directory = tempfile::tempdir().unwrap();
    let invoke = |args: &[&str]| {
        common::cli(directory.path())
            .arg("--no-tui")
            .args(args)
            .output()
            .unwrap()
    };
    let help = invoke(&["providers", "add", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("openai"));
    assert!(help.contains("--oauth-timeout-seconds"));
    assert!(!invoke(&["providers", "login", "official"]).status.success());
    assert!(
        !invoke(&[
            "providers",
            "add",
            "--preset",
            "openai",
            "--oauth-timeout-seconds",
            "0"
        ])
        .status
        .success()
    );
    let interface = query(directory.path(), &["interface", "--json"]);
    assert!(
        interface["provider_catalog"]
            .get("oauth_providers")
            .is_none()
    );
    assert_eq!(interface["provider_catalog"]["oauth_timeout_seconds"], 120);
    let official = interface["provider_catalog"]["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|preset| preset["id"] == "openai")
        .unwrap();
    assert_eq!(official["default_id"], "official");
    assert_eq!(official["auth_mode"], "local_oauth");
    assert_eq!(official["api_key_required"], false);
}
fn query(directory: &std::path::Path, args: &[&str]) -> Value {
    let output = common::cli(directory)
        .args(["--no-tui"])
        .args(args)
        .env("CODEX_GATEWAY_CONFIG", directory.join("mixin/config.json"))
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn discovers_official_login_without_installation_or_config_writes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("codex")).unwrap();
    let empty = query(root, &["providers", "list", "--json"]);
    assert!(empty["providers"].as_array().unwrap().is_empty());
    std::fs::write(
        root.join("codex/auth.json"),
        serde_json::to_vec(
            &json!({"tokens":{"access_token":"fixture","account_id":"fixture-account"}}),
        )
        .unwrap(),
    )
    .unwrap();
    std::fs::write(root.join("codex/models_cache.json"), serde_json::to_vec(&json!({"models":[{"slug":"future-official","display_name":"Future official","input_modalities":["text"]}]})).unwrap()).unwrap();
    for _ in 0..2 {
        let listed = query(root, &["providers", "list", "--json"]);
        assert_eq!(listed["providers"].as_array().unwrap().len(), 1);
        assert_eq!(listed["providers"][0]["kind"], "official");
        assert_eq!(
            listed["providers"][0]["cached_models"][0]["id"],
            "future-official"
        );
        assert!(listed["codex_install_mode"].is_null());
        assert!(!root.join("mixin/config.json").exists());
        assert!(!root.join("codex/config.toml").exists());
    }
    let status = query(root, &["info", "--json"]);
    assert_eq!(status["configured"], true);
    std::fs::write(
        root.join("codex/auth.json"),
        r#"{"OPENAI_API_KEY":"fixture"}"#,
    )
    .unwrap();
    assert!(
        query(root, &["providers", "list", "--json"])["providers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    std::fs::create_dir_all(root.join("mixin/oauth")).unwrap();
    std::fs::write(
        root.join("mixin/oauth/official.json"),
        r#"{"tokens":{"access_token":"owned-fixture","account_id":"owned-account"}}"#,
    )
    .unwrap();
    assert_eq!(
        query(root, &["providers", "list", "--json"])["providers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!root.join("mixin/config.json").exists());
    let snapshot =
        |account: &str| json!({"codex_mixin_account_id":account,"models":[{"slug":"owned-model"}]});
    std::fs::write(
        root.join("mixin/official-models.json"),
        serde_json::to_vec(&snapshot("another-account")).unwrap(),
    )
    .unwrap();
    let stale = query(root, &["providers", "list", "--json"]);
    assert!(
        stale["providers"][0]["cached_models"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(stale["providers"][0]["readiness"], "degraded");
    std::fs::write(
        root.join("mixin/official-models.json"),
        serde_json::to_vec(&snapshot("owned-account")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        query(root, &["providers", "list", "--json"])["providers"][0]["cached_models"][0]["id"],
        "owned-model"
    );
}
