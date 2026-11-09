use std::fs;
use std::path::Path;
use std::process::{Output, Stdio};

use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
use codex_mixin::provider::{ProviderModel, ProviderModelSource, ProviderPreset};
use serde_json::{Value, json};
use tokio::process::Command;

mod common;

const ORIGINAL_MODEL: &str = "Claude Opus 4.6 (Thinking)";

fn settings_path(root: &Path) -> std::path::PathBuf {
    root.join("home/.gemini/antigravity-cli/settings.json")
}

fn read_settings(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(settings_path(root)).unwrap()).unwrap()
}

fn write_settings(root: &Path, settings: &Value) {
    fs::write(settings_path(root), serde_json::to_vec(settings).unwrap()).unwrap();
}

fn prepare(root: &Path) {
    let mut provider = ProviderPreset::BaiduOneApi.create("baidu-oneapi", "fixture-key");
    provider.base_url = "http://127.0.0.1:9".to_owned();
    provider.model_source = ProviderModelSource::Static;
    provider.request_policy.baidu_auth_bridge = None;
    provider.request_policy.baidu_code_report = false;
    provider.quota_url = None;
    provider.selected_models = vec!["gpt-6.1-sol".to_owned(), "DeepSeek-V4.1-Flash".to_owned()];
    provider.cached_models = provider
        .selected_models
        .iter()
        .map(|id| ProviderModel {
            id: id.clone(),
            display_name: Some(id.clone()),
            context_window: Some(200_000),
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
    fs::create_dir_all(settings_path(root).parent().unwrap()).unwrap();
    write_settings(
        root,
        &json!({"model":ORIGINAL_MODEL,"trustedWorkspaces":["/workspace"]}),
    );
}

async fn run_cli(root: &Path, args: &[&str]) -> Output {
    let mut command: Command = common::tokio_cli(root);
    // Files let us wait for the CLI independently of a descendant gateway.
    let stdout = root.join("cli.stdout");
    let stderr = root.join("cli.stderr");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap());
    let status = tokio::time::timeout(std::time::Duration::from_secs(60), command.status())
        .await
        .unwrap_or_else(|_| panic!("CLI command timed out: {args:?}"))
        .unwrap();
    Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    }
}

async fn agy_state(root: &Path) -> String {
    let output = run_cli(root, &["--no-tui", "agent", "list"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let list: Value = serde_json::from_slice(&output.stdout).unwrap();
    let agy = list["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"] == "agy")
        .unwrap();
    agy["integration"]["state"].as_str().unwrap().to_owned()
}

async fn stop_gateway(root: &Path) {
    let _ = run_cli(root, &["service", "stop", "--force"]).await;
}

#[tokio::test]
async fn agy_model_switch_keeps_integration_managed_and_disconnectable() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let root = root.as_path();
    prepare(root);

    let connected = run_cli(root, &["--no-tui", "agent", "connect", "agy"]).await;
    assert!(
        connected.status.success(),
        "{}",
        String::from_utf8_lossy(&connected.stderr)
    );
    assert_eq!(agy_state(root).await, "managed");

    // agy `/model` rewrites `model`; other agy writes touch unrelated keys.
    let mut settings = read_settings(root);
    let switched = settings["customModelsConfig"]["customModels"]
        .as_object()
        .unwrap()
        .keys()
        .nth(1)
        .unwrap()
        .clone();
    settings["model"] = json!(switched);
    settings["theme"] = json!("dark");
    write_settings(root, &settings);
    assert_eq!(agy_state(root).await, "managed");

    let disconnected = run_cli(root, &["--no-tui", "agent", "disconnect", "agy"]).await;
    stop_gateway(root).await;
    assert!(
        disconnected.status.success(),
        "{}",
        String::from_utf8_lossy(&disconnected.stderr)
    );
    assert_eq!(
        read_settings(root),
        json!({"model":ORIGINAL_MODEL,"trustedWorkspaces":["/workspace"],"theme":"dark"})
    );
}

#[tokio::test]
async fn agy_failed_disconnect_keeps_startup_entries() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let root = root.as_path();
    prepare(root);
    let connected = run_cli(root, &["--no-tui", "agent", "connect", "agy"]).await;
    assert!(
        connected.status.success(),
        "{}",
        String::from_utf8_lossy(&connected.stderr)
    );

    let mut settings = read_settings(root);
    settings["modelProvider"] = json!("user-edited");
    write_settings(root, &settings);
    let refused = run_cli(root, &["--no-tui", "agent", "disconnect", "agy"]).await;
    assert!(!refused.status.success());

    settings["modelProvider"] = json!("gemini");
    write_settings(root, &settings);
    assert_eq!(agy_state(root).await, "managed");
    let disconnected = run_cli(root, &["--no-tui", "agent", "disconnect", "agy"]).await;
    stop_gateway(root).await;
    assert!(
        disconnected.status.success(),
        "{}",
        String::from_utf8_lossy(&disconnected.stderr)
    );
}
