use std::collections::BTreeMap;
use std::fs;
use std::process::Command;

use codex_mixin::config::{
    StoredGatewayConfig, load_stored_config_from_path, save_stored_config_to_path,
};
use codex_mixin::provider::custom_provider;
use serde_json::{Value, json};

#[test]
fn official_contexts_reach_clients() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("gateway.json");
    let codex_home = directory.path().join("codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(codex_home.join("auth.json"), "{}").unwrap();
    fs::write(
        directory.path().join("official-models.json"),
        serde_json::to_vec(&json!({"models":[{
            "slug":"gpt-test", "display_name":"GPT Test",
            "context_window":272000, "max_context_window":1050000
        }]}))
        .unwrap(),
    )
    .unwrap();
    save_stored_config_to_path(
        &config_path,
        &StoredGatewayConfig {
            providers: vec![{
                let mut provider = custom_provider("unused", "fixture-key");
                provider.base_url = "http://127.0.0.1:9".to_owned();
                provider.enabled = false;
                provider
            }],
            official_selected_models: Some(vec!["gpt-test".to_owned()]),
            official_model_contexts: BTreeMap::from([("gpt-test".to_owned(), 1_000_000)]),
            ..StoredGatewayConfig::default()
        },
    )
    .unwrap();
    let claude_settings = directory.path().join("claude/settings.json");
    let pi_dir = directory.path().join("pi");
    let desktop_root = directory.path().join("desktop");
    for context in [Some(1_000_000), Some(512_000), None] {
        let mut stored = load_stored_config_from_path(&config_path).unwrap().unwrap();
        stored.official_model_contexts = context
            .map(|window| BTreeMap::from([("gpt-test".to_owned(), window)]))
            .unwrap_or_default();
        save_stored_config_to_path(&config_path, &stored).unwrap();
        for args in [
            vec![
                "connect",
                "claude",
                "--settings-path",
                claude_settings.to_str().unwrap(),
            ],
            vec!["connect", "pi", "--agent-dir", pi_dir.to_str().unwrap()],
            vec![
                "connect",
                "claude-desktop",
                "--config-root",
                desktop_root.to_str().unwrap(),
            ],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_codex-mixin"))
                .args(&args)
                .env("CODEX_GATEWAY_CONFIG", &config_path)
                .env("CODEX_HOME", &codex_home)
                .env("HOME", directory.path().join("home"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let claude: Value = serde_json::from_slice(&fs::read(&claude_settings).unwrap()).unwrap();
        let pi: Value =
            serde_json::from_slice(&fs::read(pi_dir.join("models.json")).unwrap()).unwrap();
        let profile: Value = serde_json::from_slice(
            &fs::read(
                desktop_root
                    .join("Claude-3p/configLibrary/be16e7b5-9352-4cca-a197-32a6e608d930.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            json!({
                "claude_model":claude["modelPicker"]["options"][0]["model"],
                "pi_context":pi["providers"]["codex-mixin"]["models"][0]["contextWindow"],
                "desktop_1m":profile["inferenceModels"][0]["supports1m"]
            }),
            json!({"claude_model": if context == Some(1_000_000) { "gpt-test[1m]" } else { "gpt-test" }, "pi_context":context.unwrap_or(272000), "desktop_1m":context == Some(1_000_000)})
        );
    }
}
