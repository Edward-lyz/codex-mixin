//! Grok Build only offers `/effort` for custom models whose config carries a
//! reasoning-effort menu. Verify the CLI writes that menu from gateway model
//! capabilities, including the default level, without advertising models the
//! provider marked as non-reasoning.

use std::fs;
use std::path::Path;
use std::process::{Output, Stdio};

use codex_mixin::config::{StoredGatewayConfig, save_stored_config_to_path};
use codex_mixin::provider::{ProviderModel, ProviderModelSource, ProviderPreset};
use tokio::process::Command;

mod common;

#[tokio::test]
async fn grok_connect_advertises_reasoning_efforts() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let root = root.as_path();
    let mut provider = ProviderPreset::BaiduOneApi.create("baidu-oneapi", "fixture-key");
    provider.base_url = "http://127.0.0.1:9".to_owned();
    provider.model_source = ProviderModelSource::Static;
    provider.request_policy.baidu_auth_bridge = None;
    provider.request_policy.baidu_code_report = false;
    provider.quota_url = None;
    provider.selected_models = vec![
        "grok-4.7".to_owned(),
        "GLM-5.3".to_owned(),
        "dsf4.1".to_owned(),
        "gpt-5.6-sol".to_owned(),
    ];
    provider.cached_models = provider
        .selected_models
        .iter()
        .map(|id| ProviderModel {
            id: id.clone(),
            display_name: Some(id.clone()),
            context_window: Some(1_000_000),
            supports_thinking: Some(id.as_str() != "GLM-5.3"),
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

    let args = ["--no-tui", "--json-errors", "agent", "connect", "grok"];
    let mut connect = common::tokio_cli(root);
    connect.args(args);
    let connected = run_cli(connect, &args, root).await;
    let config = fs::read_to_string(root.join("home/.grok/config.toml"));
    let stop_args = ["--no-tui", "service", "stop", "--force"];
    let mut stop = common::tokio_cli(root);
    stop.args(stop_args);
    let stopped = run_cli(stop, &stop_args, root).await;
    assert!(
        connected.status.success(),
        "{}",
        String::from_utf8_lossy(&connected.stderr)
    );
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let config = config.expect("generated Grok config");
    let document = config
        .parse::<toml_edit::DocumentMut>()
        .expect("generated Grok config is valid TOML");
    let models = document
        .get("model")
        .and_then(toml_edit::Item::as_inline_table)
        .unwrap_or_else(|| panic!("model table:\n{config}"));

    for (id, default) in [
        ("grok-4.7-baidu-oneapi", "medium"),
        ("dsf4.1-baidu-oneapi", "medium"),
        ("gpt-5.6-sol-baidu-oneapi", "low"),
    ] {
        let entry = models
            .get(&format!("codex-mixin/{id}"))
            .and_then(toml_edit::Value::as_inline_table)
            .unwrap_or_else(|| panic!("missing {id}:\n{config}"));
        let options = entry
            .get("reasoning_efforts")
            .and_then(toml_edit::Value::as_array)
            .unwrap_or_else(|| panic!("{id} must advertise reasoning efforts:\n{config}"));
        let levels = options
            .iter()
            .map(|option| {
                option
                    .as_inline_table()
                    .and_then(|option| option.get("value"))
                    .and_then(toml_edit::Value::as_str)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(levels, ["low", "medium", "high", "xhigh", "max"]);
        let defaults = options
            .iter()
            .filter(|option| {
                option
                    .as_inline_table()
                    .and_then(|option| option.get("default"))
                    .and_then(toml_edit::Value::as_bool)
                    == Some(true)
            })
            .map(|option| {
                option
                    .as_inline_table()
                    .and_then(|option| option.get("value"))
                    .and_then(toml_edit::Value::as_str)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(defaults, [default], "{id} default effort");
    }

    let non_reasoning = models
        .get("codex-mixin/GLM-5.3-baidu-oneapi")
        .and_then(toml_edit::Value::as_inline_table)
        .expect("GLM-5.3 entry");
    assert!(
        non_reasoning.get("reasoning_efforts").is_none(),
        "a provider-declared non-reasoning model must not offer /effort:\n{config}"
    );
}

async fn run_cli(mut command: Command, args: &[&str], directory: &Path) -> Output {
    // Descendant gateways can keep inherited Windows pipe handles open.
    // Files let us wait for the CLI's exit independently of gateway lifetime.
    let stdout = directory.join("cli.stdout");
    let stderr = directory.join("cli.stderr");
    command
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap());
    let status = tokio::time::timeout(std::time::Duration::from_secs(60), command.status())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "CLI command timed out: {args:?}: {}",
                fs::read_to_string(&stderr).unwrap()
            )
        })
        .unwrap();
    Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    }
}
