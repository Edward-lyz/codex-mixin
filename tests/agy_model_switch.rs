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
async fn antigravity_desktop_connect_discovers_models_and_restores_settings() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let root = root.as_path();
    prepare(root);
    let settings = [
        root.join("home/.gemini/config/config.json"),
        root.join("home/.antigravity/settings.json"),
    ];
    let original = json!({
        "gateway":{"enabled":false,"wireProtocol":"genai"},
        "models":{"defaultModelId":"previous","list":[{"modelId":"previous"}],"other":null},
        "plugins":{"enabled":true}
    });
    for path in &settings {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&original).unwrap()).unwrap();
    }

    let connected = run_cli(root, &["--no-tui", "agent", "connect", "antigravity"]).await;
    assert!(
        connected.status.success(),
        "{}",
        String::from_utf8_lossy(&connected.stderr)
    );
    let cli_after_connect = read_settings(root);
    let managed: Value = serde_json::from_slice(&fs::read(&settings[0]).unwrap()).unwrap();
    assert_eq!(managed["gateway"]["enabled"], true);
    assert_eq!(managed["gateway"]["wireProtocol"], "openai");
    assert!(managed["models"].get("list").is_none());
    let catalog = reqwest::Client::new()
        .get(managed["models"]["listSource"].as_str().unwrap())
        .bearer_auth(managed["gateway"]["apiKey"].as_str().unwrap())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        catalog["list"][0]["modelId"],
        managed["models"]["defaultModelId"]
    );
    assert_eq!(catalog["list"][0]["maxTokens"], 200_000);
    assert_eq!(catalog["list"].as_array().unwrap().len(), 2);
    let legacy: Value = serde_json::from_slice(&fs::read(&settings[1]).unwrap()).unwrap();
    assert_eq!(legacy, managed);

    let mut edited = managed;
    edited["models"]["defaultModelId"] = catalog["list"][1]["modelId"].clone();
    edited["plugins"]["new"] = json!(true);
    fs::write(&settings[0], serde_json::to_vec(&edited).unwrap()).unwrap();
    let reconnected = run_cli(root, &["--no-tui", "agent", "connect", "antigravity"]).await;
    assert!(
        reconnected.status.success(),
        "{}",
        String::from_utf8_lossy(&reconnected.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&settings[0]).unwrap()).unwrap(),
        edited
    );
    let disconnected = run_cli(root, &["--no-tui", "agent", "disconnect", "antigravity"]).await;
    stop_gateway(root).await;
    assert!(
        disconnected.status.success(),
        "{}",
        String::from_utf8_lossy(&disconnected.stderr)
    );
    assert_eq!(read_settings(root), cli_after_connect);
    assert_eq!(cli_after_connect["model"], ORIGINAL_MODEL);
    let mut expected = original.clone();
    expected["plugins"]["new"] = json!(true);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&settings[0]).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        fs::read(&settings[1]).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
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

#[cfg(not(windows))]
#[tokio::test]
async fn agy_disconnect_recovers_after_installer_strips_alias_line() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let root = root.as_path();
    prepare(root);

    let zshrc = root.join("home/.zshrc");
    let bashrc = root.join("home/.bashrc");
    let profile = root.join("home/.profile");
    let fish = root.join("xdg-config/fish/conf.d/codex-mixin-agy.fish");
    fs::write(&zshrc, "export FOO=1\n").unwrap();
    fs::write(&bashrc, "").unwrap();
    fs::write(&profile, "export BAR=1\n").unwrap();

    let connected = run_cli(root, &["--no-tui", "agent", "connect", "agy"]).await;
    assert!(
        connected.status.success(),
        "{}",
        String::from_utf8_lossy(&connected.stderr)
    );
    assert_eq!(agy_state(root).await, "managed");
    assert!(fish.exists());

    // Simulate the Antigravity CLI installer: strips `alias agy=...` lines
    // (leaving an empty managed fence) and appends its PATH export.
    let installer_tail =
        "\n\n# Added by Antigravity CLI installer\nexport PATH=\"$HOME/.local/bin:$PATH\"\n";
    for path in [&zshrc, &bashrc, &profile] {
        let text = fs::read_to_string(path).unwrap();
        let mut stripped: String = text
            .lines()
            .filter(|line| !line.starts_with("alias agy="))
            .map(|line| format!("{line}\n"))
            .collect();
        stripped.push_str(installer_tail);
        fs::write(path, stripped).unwrap();
    }

    assert_eq!(agy_state(root).await, "drift");

    let disconnected = run_cli(root, &["--no-tui", "agent", "disconnect", "agy"]).await;
    assert!(
        disconnected.status.success(),
        "{}",
        String::from_utf8_lossy(&disconnected.stderr)
    );
    assert_eq!(agy_state(root).await, "unmanaged");
    assert_eq!(
        fs::read_to_string(&zshrc).unwrap(),
        format!("export FOO=1\n{installer_tail}")
    );
    assert_eq!(fs::read_to_string(&bashrc).unwrap(), installer_tail);
    assert_eq!(
        fs::read_to_string(&profile).unwrap(),
        format!("export BAR=1\n{installer_tail}")
    );
    assert!(!fish.exists());
    assert_eq!(
        read_settings(root),
        json!({"model":ORIGINAL_MODEL,"trustedWorkspaces":["/workspace"]})
    );

    // Reconnecting after disconnect must succeed cleanly.
    let reconnected = run_cli(root, &["--no-tui", "agent", "connect", "agy"]).await;
    stop_gateway(root).await;
    assert!(
        reconnected.status.success(),
        "{}",
        String::from_utf8_lossy(&reconnected.stderr)
    );
    assert_eq!(agy_state(root).await, "managed");
}

#[cfg(not(windows))]
#[tokio::test]
async fn agy_disconnect_removes_created_profile_when_empty_fence_leaves_it_empty() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let root = root.as_path();
    prepare(root);

    let zshrc = root.join("home/.zshrc");
    assert!(!zshrc.exists());

    let connected = run_cli(root, &["--no-tui", "agent", "connect", "agy"]).await;
    assert!(
        connected.status.success(),
        "{}",
        String::from_utf8_lossy(&connected.stderr)
    );
    assert!(zshrc.exists());

    let text = fs::read_to_string(&zshrc).unwrap();
    let stripped: String = text
        .lines()
        .filter(|line| !line.starts_with("alias agy="))
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(&zshrc, stripped).unwrap();

    let disconnected = run_cli(root, &["--no-tui", "agent", "disconnect", "agy"]).await;
    stop_gateway(root).await;
    assert!(
        disconnected.status.success(),
        "{}",
        String::from_utf8_lossy(&disconnected.stderr)
    );
    assert!(!zshrc.exists());
}

#[cfg(not(windows))]
#[tokio::test]
async fn agy_disconnect_refuses_edited_or_ambiguous_startup_fences_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let root = root.as_path();
    prepare(root);

    let zshrc = root.join("home/.zshrc");
    let bashrc = root.join("home/.bashrc");
    fs::write(&zshrc, "export FOO=1\n").unwrap();
    fs::write(&bashrc, "export BAR=1\n").unwrap();

    let connected = run_cli(root, &["--no-tui", "agent", "connect", "agy"]).await;
    assert!(
        connected.status.success(),
        "{}",
        String::from_utf8_lossy(&connected.stderr)
    );

    let empty_fence_zshrc = concat!(
        "export FOO=1\n\n",
        "# >>> codex-mixin managed agy >>>\n",
        "# <<< codex-mixin managed agy <<<\n",
    );
    fs::write(&zshrc, empty_fence_zshrc).unwrap();

    // 1. Non-empty edit in .bashrc must fail without touching .zshrc's empty fence.
    let edited_bashrc = concat!(
        "export BAR=1\n\n",
        "# >>> codex-mixin managed agy >>>\n",
        "alias agy='custom-user-wrapper'\n",
        "# <<< codex-mixin managed agy <<<\n",
    );
    fs::write(&bashrc, edited_bashrc).unwrap();
    let refused = run_cli(root, &["--no-tui", "agent", "disconnect", "agy"]).await;
    assert!(!refused.status.success());
    assert_eq!(fs::read_to_string(&zshrc).unwrap(), empty_fence_zshrc);
    assert_eq!(fs::read_to_string(&bashrc).unwrap(), edited_bashrc);
    assert_eq!(agy_state(root).await, "drift");

    // 2. Inline trailing text on the end marker must be rejected.
    let inline_bashrc = concat!(
        "export BAR=1\n\n",
        "# >>> codex-mixin managed agy >>>\n",
        "# <<< codex-mixin managed agy <<< foo\n",
    );
    fs::write(&bashrc, inline_bashrc).unwrap();
    let refused = run_cli(root, &["--no-tui", "agent", "disconnect", "agy"]).await;
    assert!(!refused.status.success());
    assert_eq!(fs::read_to_string(&zshrc).unwrap(), empty_fence_zshrc);
    assert_eq!(fs::read_to_string(&bashrc).unwrap(), inline_bashrc);

    // 3. Duplicate empty fences must be rejected.
    let duplicate_bashrc = concat!(
        "export BAR=1\n\n",
        "# >>> codex-mixin managed agy >>>\n",
        "# <<< codex-mixin managed agy <<<\n\n",
        "# >>> codex-mixin managed agy >>>\n",
        "# <<< codex-mixin managed agy <<<\n",
    );
    fs::write(&bashrc, duplicate_bashrc).unwrap();
    let refused = run_cli(root, &["--no-tui", "agent", "disconnect", "agy"]).await;
    stop_gateway(root).await;
    assert!(!refused.status.success());
    assert_eq!(fs::read_to_string(&zshrc).unwrap(), empty_fence_zshrc);
    assert_eq!(fs::read_to_string(&bashrc).unwrap(), duplicate_bashrc);
}
