use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Output, Stdio};
use tokio::process::Command;

use codex_mixin::config::{
    StoredGatewayConfig, load_stored_config_from_path, save_stored_config_to_path,
};
use codex_mixin::provider::{ProviderModel, ProviderPreset};
use serde_json::{Value, json};

mod common;

#[tokio::test]
async fn oneapi_rate_labels_reach_native_client_configs() {
    use codex_mixin::provider::{ProviderModel, ProviderModelSource};

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let root = root.as_path();
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
        .zip(["0.5x", "0.4x"])
        .map(|(id, ratio)| ProviderModel {
            id: id.clone(),
            display_name: Some(id.clone()),
            ratio: Some(ratio.to_owned()),
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
    let buddy = root.join("workbuddy");
    fs::create_dir_all(&buddy).unwrap();
    let original = json!([{"id":"unrelated","name":"User Model","custom":true}]);
    fs::write(
        buddy.join("models.json"),
        serde_json::to_vec(&original).unwrap(),
    )
    .unwrap();
    let codebuddy = root.join("codebuddy");
    let pi = root.join("pi");
    let mut snapshots = Vec::new();
    for args in [
        vec!["agent", "connect", "workbuddy"],
        vec!["agent", "connect", "codebuddy"],
        vec!["connect", "pi", "--agent-dir", pi.to_str().unwrap()],
    ] {
        let mut command = common::tokio_cli(root);
        command
            .args(&args)
            .env("WORKBUDDY_CONFIG_DIR", &buddy)
            .env("CODEBUDDY_CONFIG_DIR", &codebuddy);
        let output = run_cli(command, &args, root).await;
        if !output.status.success() {
            let mut stop = common::tokio_cli(root);
            stop.args(["service", "stop", "--force"]);
            let _ = run_cli(stop, &["service", "stop", "--force"], root).await;
            panic!("{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        }
    }
    for path in [
        buddy.join("models.json"),
        codebuddy.join("models.json"),
        pi.join("models.json"),
    ] {
        snapshots.push(serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap());
    }
    for id in ["workbuddy", "codebuddy"] {
        let args = ["agent", "disconnect", id];
        let mut command = common::tokio_cli(root);
        command
            .args(args)
            .env("WORKBUDDY_CONFIG_DIR", &buddy)
            .env("CODEBUDDY_CONFIG_DIR", &codebuddy);
        let output = run_cli(command, &args, root).await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut stop = common::tokio_cli(root);
    stop.args(["service", "stop", "--force"]);
    let _ = run_cli(stop, &["service", "stop", "--force"], root).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(buddy.join("models.json")).unwrap()).unwrap(),
        original
    );
    for (index, models) in [
        &snapshots[0],
        &snapshots[1]["models"],
        &snapshots[2]["providers"]["codex-mixin"]["models"],
    ]
    .into_iter()
    .enumerate()
    {
        let models = models.as_array().unwrap();
        for (id, name, ratio) in [
            ("gpt-6.1-sol-baidu-oneapi", "GPT-6.1 Sol", "0.5x"),
            (
                "DeepSeek-V4.1-Flash-baidu-oneapi",
                "DeepSeek V4.1 Flash",
                "0.4x",
            ),
        ] {
            let model = models.iter().find(|model| model["id"] == id).unwrap();
            let name = if index < 2 {
                assert_eq!(model["credits"], ratio);
                id.to_owned()
            } else {
                format!("{name} ({ratio}) \u{b7} oneapi")
            };
            assert_eq!(model["name"], name);
        }
    }
}

#[tokio::test]
async fn official_contexts_reach_clients() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("gateway.json");
    let codex_home = directory.path().join("codex");
    fs::create_dir_all(&codex_home).unwrap();
    // Official models are only advertised when a ChatGPT OAuth login can serve
    // them, so the fixture needs real tokens rather than an empty auth.json.
    fs::write(
        codex_home.join("auth.json"),
        serde_json::to_vec(&json!({
            "tokens":{"access_token":"codex-oauth-token","account_id":"account-1"}
        }))
        .unwrap(),
    )
    .unwrap();
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
                let mut provider = ProviderPreset::Custom.create("unused", "fixture-key");
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
    let library = desktop_root.join("Claude-3p/configLibrary");
    fs::create_dir_all(&library).unwrap();
    let profile_path = library.join("be16e7b5-9352-4cca-a197-32a6e608d930.json");
    let orphan = json!({"sealed": "fixture-desktop-value", "custom": true});
    fs::write(&profile_path, serde_json::to_vec(&orphan).unwrap()).unwrap();
    let metadata_path = library.join("_meta.json");
    let original_metadata = json!({
        "appliedId": "user-profile",
        "entries": [{"id": "user-profile", "name": "User Profile"}],
        "custom": true
    });
    fs::write(
        &metadata_path,
        serde_json::to_vec(&original_metadata).unwrap(),
    )
    .unwrap();
    let desktop_config = desktop_root.join("Claude/claude_desktop_config.json");
    fs::create_dir_all(desktop_config.parent().unwrap()).unwrap();
    let original_config = json!({"theme": "dark", "deploymentMode": "original"});
    fs::write(
        &desktop_config,
        serde_json::to_vec(&original_config).unwrap(),
    )
    .unwrap();
    // Claude Desktop connect restarts a real gateway, which refreshes the
    // official catalog. Block outbound HTTP so the fixture catalog survives.
    let mixin = |args: &[&str]| {
        let mut command = common::tokio_cli(directory.path());
        command
            .kill_on_drop(true)
            .args(args)
            .env("CODEX_GATEWAY_CONFIG", &config_path)
            .env("CODEX_HOME", &codex_home);
        command
    };
    for (round, context) in [Some(1_000_000), Some(512_000), None]
        .into_iter()
        .enumerate()
    {
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
            let output = run_cli(mixin(&args), &args, directory.path()).await;
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
                "desktop_1m":profile["inferenceModels"][0]["supports1m"],
                "desktop_prefer_1m":profile["inferenceModels"][0]["prefer1m"],
                "desktop_behaves_as":profile["inferenceModels"][0]["behavesAs"]
            }),
            json!({"claude_model": if context == Some(1_000_000) { "gpt-test[1m]" } else { "gpt-test" }, "pi_context":context.unwrap_or(272000), "desktop_1m":context == Some(1_000_000), "desktop_prefer_1m":context == Some(1_000_000), "desktop_behaves_as": Value::Null})
        );
        let reconnect_args = [
            "connect",
            "claude-desktop",
            "--config-root",
            desktop_root.to_str().unwrap(),
        ];
        let reconnected = run_cli(mixin(&reconnect_args), &reconnect_args, directory.path()).await;
        assert!(
            reconnected.status.success(),
            "{}",
            String::from_utf8_lossy(&reconnected.stderr)
        );
        // Desktop rewrites the managed profile after it consumes it. A newly
        // created profile must still disappear on disconnect, allowing retry.
        fs::write(&profile_path, br#"{"sealed":"fixture-desktop-rewrite"}"#).unwrap();
        let remove_args = [
            "connect",
            "remove",
            "claude-desktop",
            "--config-root",
            desktop_root.to_str().unwrap(),
        ];
        let removed = run_cli(mixin(&remove_args), &remove_args, directory.path()).await;
        assert!(
            removed.status.success(),
            "{}",
            String::from_utf8_lossy(&removed.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&desktop_config).unwrap()).unwrap(),
            original_config
        );
        assert!(
            !library
                .join("be16e7b5-9352-4cca-a197-32a6e608d930.mixin-backup")
                .exists()
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&metadata_path).unwrap()).unwrap(),
            original_metadata
        );
        if round == 0 {
            assert_eq!(
                serde_json::from_slice::<Value>(&fs::read(&profile_path).unwrap()).unwrap(),
                orphan
            );
            fs::remove_file(&profile_path).unwrap();
        } else {
            assert!(
                !profile_path.exists(),
                "Desktop rewrite must not leave an orphan profile"
            );
        }
    }
    let stop_args = ["service", "stop", "--force"];
    let stopped = run_cli(mixin(&stop_args), &stop_args, directory.path()).await;
    assert!(
        stopped.status.success(),
        "stop test gateway: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
}

#[tokio::test]
async fn desktop_effort_anchors_follow_model_versions() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("gateway.json");
    let codex_home = directory.path().join("codex");
    fs::create_dir_all(&codex_home).unwrap();
    let models = [
        ("Claude Haiku 5.5", 1_000_000, Some("claude-haiku-5-5")),
        ("Claude Haiku 4.5", 200_000, Some("claude-haiku-4-5")),
        ("Claude Sonnet 5.5", 1_000_000, Some("claude-sonnet-5-5")),
        ("claude-sonnet-5", 1_000_000, Some("claude-sonnet-5")),
        ("Opus 4.8", 1_000_000, Some("claude-opus-4-8")),
        ("Opus 5", 1_000_000, Some("claude-opus-5")),
        ("Opus 5.5", 1_000_000, Some("claude-opus-5-5")),
        (
            "claude-opus-4-5-20251101",
            1_000_000,
            Some("claude-opus-4-5"),
        ),
        ("DeepSeek-V4.1-Flash", 1_000_000, None),
    ];
    let mut provider = ProviderPreset::Custom.create("fixture", "fixture-key");
    provider.base_url = "http://127.0.0.1:9".to_owned();
    provider.cached_models = models
        .iter()
        .map(|(id, context_window, _)| ProviderModel {
            id: (*id).to_owned(),
            context_window: Some(*context_window),
            ..ProviderModel::default()
        })
        .collect();
    provider.selected_models = models.iter().map(|(id, _, _)| (*id).to_owned()).collect();
    save_stored_config_to_path(
        &config_path,
        &StoredGatewayConfig {
            providers: vec![provider],
            ..StoredGatewayConfig::default()
        },
    )
    .unwrap();

    let desktop_root = directory.path().join("desktop");
    let args = [
        "connect",
        "claude-desktop",
        "--config-root",
        desktop_root.to_str().unwrap(),
    ];
    let mut command = common::tokio_cli(directory.path());
    command
        .kill_on_drop(true)
        .args(args)
        .env("CODEX_GATEWAY_CONFIG", &config_path)
        .env("CODEX_HOME", &codex_home);
    let output = run_cli(command, &args, directory.path()).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let profile: Value = serde_json::from_slice(
        &fs::read(
            desktop_root.join("Claude-3p/configLibrary/be16e7b5-9352-4cca-a197-32a6e608d930.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let anchors = profile["inferenceModels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| {
            let label = model["labelOverride"].as_str().unwrap();
            let label = label.split(" - ").next().unwrap().to_owned();
            (label, model["behavesAs"].clone())
        })
        .collect::<BTreeMap<_, _>>();
    let expected = models
        .iter()
        .map(|(id, _, anchor)| {
            (
                (*id).to_owned(),
                anchor.map_or(Value::Null, |anchor| json!(anchor)),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(anchors, expected);
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
