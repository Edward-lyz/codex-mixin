mod common;

use serde_json::{Value, json};

#[tokio::test]
async fn business_authorization_reaches_the_bounded_browser_workflow() {
    let error = codex_mixin::application::provider::login::authorize_add(
        codex_mixin::provider::ProviderPreset::AgyBusiness,
        0,
        |_| panic!("invalid timeout must not start browser authorization"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("OAuth timeout"), "{error:#}");
}

#[test]
fn business_add_reuses_owned_credentials_and_preserves_other_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let oauth = root.join("oauth");
    std::fs::create_dir(&oauth).unwrap();
    let owned = br#"{"auth_method":"gcp","token":{"access_token":"fixture-business","expiry":"2099-01-01T00:00:00Z"}}"#;
    let personal = br#"{"auth_method":"consumer","token":{"access_token":"fixture-personal","expiry":"2099-01-01T00:00:00Z"}}"#;
    std::fs::write(oauth.join("agy-business.json"), owned).unwrap();
    std::fs::write(oauth.join("agy.json"), personal).unwrap();
    let output = common::cli(root)
        .args([
            "--no-tui",
            "--json-errors",
            "providers",
            "add",
            "--preset",
            "agy-business",
        ])
        .env("PATH", "")
        .env(
            "GOOGLE_APPLICATION_CREDENTIALS",
            root.join("absent-adc.json"),
        )
        .output()
        .unwrap();
    // Outbound traffic is blocked by tests/common. Reaching discovery proves
    // the owned enterprise token was accepted without opening a browser.
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("Open this URL"), "{stderr}");
    assert!(
        !stderr.contains("AGY Business credentials are missing"),
        "{stderr}"
    );
    let error: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(error["error"]["committed"], true, "{stderr}");
    assert_eq!(error["error"]["stage"], "model discovery");
    let config = codex_mixin::config::load_stored_config_from_path(&root.join("config.json"))
        .unwrap()
        .unwrap();
    assert!(
        config
            .providers
            .iter()
            .any(|provider| provider.preset_id.as_deref() == Some("agy-business"))
    );
    assert_eq!(
        std::fs::read(oauth.join("agy-business.json")).unwrap(),
        owned
    );
    assert_eq!(std::fs::read(oauth.join("agy.json")).unwrap(), personal);
}

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
