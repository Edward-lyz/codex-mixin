mod common;

use std::fs;
use std::path::Path;

use codex_mixin::clients::codex;
use codex_mixin::config::{
    StoredGatewayConfig, load_stored_config_from_path, save_stored_config_to_path,
};
use codex_mixin::provider::{ModelOrder, ProviderModel, ProviderModelSource, custom_provider};
use serde_json::{Value, json};

fn prepare(root: &Path, official: bool) -> std::path::PathBuf {
    let home = root.join("codex");
    fs::create_dir_all(&home).unwrap();
    let catalog = home.join("models.json");
    fs::write(&catalog, "{\"models\":[]}").unwrap();
    let mut document = toml_edit::DocumentMut::new();
    codex::upsert(
        &mut document,
        None,
        &catalog,
        "http://127.0.0.1:9/v1",
        "text",
        Some("fixture-client-key"),
        official,
    )
    .unwrap();
    fs::write(home.join("config.toml"), codex::serialize(&document)).unwrap();
    let mut provider = custom_provider("test", "fixture-key");
    provider.base_url = "http://127.0.0.1:9".to_owned();
    provider.model_source = ProviderModelSource::Static;
    provider.selected_models = [
        "Claude Haiku 5.5",
        "Opus 4.8",
        "Opus 5.5",
        "Claude Sonnet 5.5",
    ]
    .map(str::to_owned)
    .to_vec();
    provider.cached_models = provider
        .selected_models
        .iter()
        .map(|id| ProviderModel {
            id: id.clone(),
            supports_function_tools: Some(true),
            ..Default::default()
        })
        .collect();
    save_stored_config_to_path(
        &root.join("config.json"),
        &StoredGatewayConfig {
            providers: vec![provider],
            ..Default::default()
        },
    )
    .unwrap();
    catalog
}

fn select(root: &Path, id: &str, args: &[&str]) -> std::process::Output {
    common::cli(root)
        .args(["--no-tui", "--json-errors", "providers", "select", id])
        .args(args)
        .env(
            "CODEX_GATEWAY_MODEL_METADATA_URL",
            "http://127.0.0.1:9/models.json",
        )
        .output()
        .unwrap()
}

fn catalog_models(path: &Path) -> Vec<Value> {
    serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap()["models"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn cli_model_order_is_persisted_and_reaches_codex_priorities() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let catalog = prepare(root, false);
    for (mode, expected) in [
        (
            "manual",
            [
                "Claude Haiku 5.5",
                "Opus 4.8",
                "Opus 5.5",
                "Claude Sonnet 5.5",
            ],
        ),
        (
            "family",
            [
                "Opus 5.5",
                "Claude Sonnet 5.5",
                "Claude Haiku 5.5",
                "Opus 4.8",
            ],
        ),
    ] {
        let mut args = vec!["--model-order", mode];
        if mode == "manual" {
            for model in expected {
                args.extend(["--model", model]);
            }
        }
        let output = select(root, "test", &args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let config = load_stored_config_from_path(&root.join("config.json"))
            .unwrap()
            .unwrap();
        assert_eq!(config.providers[0].model_order.as_str(), mode);
        assert_eq!(config.providers[0].selected_models, expected);
        let models = catalog_models(&catalog);
        assert_eq!(
            models
                .iter()
                .map(|model| model["slug"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected.map(|id| format!("{id}-test"))
        );
        assert!(
            models
                .windows(2)
                .all(|pair| pair[0]["priority"].as_u64() < pair[1]["priority"].as_u64())
        );
    }
    let output = select(
        root,
        "test",
        &["--model", "Opus 4.8", "--model", "Claude Haiku 5.5"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = load_stored_config_from_path(&root.join("config.json"))
        .unwrap()
        .unwrap();
    assert_eq!(config.providers[0].model_order, ModelOrder::Manual);
    assert_eq!(
        config.providers[0].selected_models,
        ["Opus 4.8", "Claude Haiku 5.5"]
    );
    assert_eq!(catalog_models(&catalog)[0]["slug"], "Opus 4.8-test");
}

#[test]
fn account_catalog_membership_and_manual_order_reach_codex_together() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let catalog = prepare(root, true);
    fs::write(
        root.join("codex/auth.json"),
        serde_json::to_vec(&json!({"tokens":{
            "access_token":"fixture-token","account_id":"fixture-account"
        }}))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("official-template.json"),
        serde_json::to_vec(&json!({"models":[
            {"slug":"gpt-5.5","visibility":"list"},
            {"slug":"gpt-current","visibility":"list","context_window":272000,
                "base_instructions":"public instructions"},
            {"slug":"gpt-hidden","visibility":"list"}
        ]}))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("official-models.json"),
        serde_json::to_vec(&json!({
            "codex_mixin_source":"https://chatgpt.com/backend-api/codex/models",
            "codex_mixin_account_id":"fixture-account",
            "models":[
                {"slug":"gpt-current","visibility":"list","context_window":500000},
                {"slug":"gpt-hidden","visibility":"hide"},
                {"slug":"gpt-new","visibility":"list","context_window":600000}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let output = select(root, "test", &["--model-order=family"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let models = catalog_models(&catalog);
    assert!(!models.iter().any(|model| model["slug"] == "gpt-5.5"));
    assert_eq!(
        models
            .iter()
            .find(|model| model["slug"] == "gpt-hidden")
            .unwrap()["visibility"],
        "hide"
    );
    assert!(models.iter().any(|model| model["slug"] == "gpt-new"));
    let output = select(root, "official", &["--model-order=manual"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = load_stored_config_from_path(&root.join("config.json"))
        .unwrap()
        .unwrap();
    assert_eq!(config.official_model_order, ModelOrder::Manual);
    assert_eq!(
        config.official_selected_models.unwrap(),
        ["gpt-current", "gpt-new"]
    );
    let output = select(
        root,
        "official",
        &[
            "--model-order=manual",
            "--model=gpt-new",
            "--model=gpt-current",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let models = catalog_models(&catalog);
    assert_eq!(models[0]["slug"], "gpt-new");
    assert_eq!(models[1]["slug"], "gpt-current");
    assert!(models[0]["priority"].as_u64() < models[1]["priority"].as_u64());
    assert_eq!(models[1]["base_instructions"], "public instructions");
    assert_eq!(models[1]["context_window"], 500000);
    assert!(models.iter().any(|model| model["slug"] == "Opus 5.5-test"));
    fs::write(
        root.join("official-models.json"),
        serde_json::to_vec(&json!({
            "codex_mixin_source":"https://chatgpt.com/backend-api/codex/models",
            "codex_mixin_account_id":"other-account", "models":[{"slug":"gpt-5.5"}]
        }))
        .unwrap(),
    )
    .unwrap();
    let before = fs::read(&catalog).unwrap();
    let output = select(root, "test", &["--model-order=manual"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("current official account model catalog is missing")
    );
    assert_eq!(fs::read(&catalog).unwrap(), before);
    let stderr = String::from_utf8(output.stderr).unwrap();
    let envelope: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(envelope["error"]["committed"], true);
    assert_eq!(
        envelope["error"]["stage"],
        "Codex model catalog synchronization"
    );
}
