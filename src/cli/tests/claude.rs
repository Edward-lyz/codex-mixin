use std::fs;

use clap::Parser;

use codex_mixin::config::{GatewayConfig, ThinkingMode};
use codex_mixin::provider::{ProviderModel, ProviderPreset, ProviderProtocol};

use crate::cli::Cli;
use crate::cli::claude::*;

#[test]
fn claude_install_writes_base_url_and_uninstall_restores_settings() {
    let directory = tempfile::tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let mut baidu = ProviderPreset::BaiduOneApi.create("baidu", "key");
    baidu.quota_username = Some("test-user".to_owned());
    baidu.selected_models = vec![
        "Claude Opus 5".to_owned(),
        "Claude Sonnet 5".to_owned(),
        "Claude Haiku 5".to_owned(),
    ];
    baidu.cached_models = baidu
        .selected_models
        .iter()
        .map(|id| ProviderModel {
            id: id.clone(),
            display_name: Some(format!("{id} marketing description")),
            protocol: Some(ProviderProtocol::AnthropicMessages),
            ..ProviderModel::default()
        })
        .collect();
    let gateway_config = GatewayConfig {
        bind: "127.0.0.1:8787".parse().unwrap(),
        providers: vec![baidu],
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        codex_auth_path: directory.path().join("auth.json"),
        gateway_api_key: None,
        gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys {
            claude: Some("claude-client-key".to_owned()),
            ..Default::default()
        },
        accept_codex_oauth: false,
        official_selected_models: None,
        default_max_tokens: 4096,
        default_context_window: 128_000,
        request_timeout: std::time::Duration::from_secs(30),
        thinking_mode: ThinkingMode::Auto,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search".to_owned(),
        web_search_max_uses: None,
        fusion_profiles: Vec::new(),
    };
    fs::write(
        &settings,
        r#"{
            "existing": true,
            "model": "old-default",
            "modelPicker": {
                "replaceBuiltInOptions": false,
                "options": [{"model": "old-picker", "label": "Old picker"}]
            },
            "modelOverrides": {
                "claude-opus-4-6": "old-opus-route",
                "unrelated-model": "keep-route"
            },
            "env": {
                "ANTHROPIC_BASE_URL": "https://old",
                "ANTHROPIC_AUTH_TOKEN": "old-token",
                "ANTHROPIC_MODEL": "old-model",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "old-opus",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "old-sonnet",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "old-haiku",
                "CLAUDE_CODE_USE_GATEWAY": "old-gateway",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "old-traffic",
                "CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT": "old-window-enforcement",
                "DISABLE_LOGIN_COMMAND": "old-login"
            }
        }"#,
    )
    .unwrap();

    install_claude_with_config(Some(settings.clone()), &gateway_config).unwrap();
    let installed: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_installed_claude_settings(&installed);

    install_claude_with_config(Some(settings.clone()), &gateway_config).unwrap();

    uninstall_claude(Some(settings.clone())).unwrap();
    let restored: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_restored_claude_settings(&restored);

    install_claude_with_config(Some(settings.clone()), &gateway_config).unwrap();
    uninstall_claude(Some(settings)).unwrap();
}

fn assert_installed_claude_settings(installed: &serde_json::Value) {
    assert_eq!(installed["existing"], true);
    assert_eq!(
        installed["env"]["ANTHROPIC_BASE_URL"].as_str().unwrap(),
        "http://127.0.0.1:8787"
    );
    assert_eq!(
        installed["env"]["ANTHROPIC_AUTH_TOKEN"].as_str().unwrap(),
        "claude-client-key"
    );
    assert_eq!(installed["env"]["CLAUDE_CODE_USE_GATEWAY"], "old-gateway");
    assert_eq!(
        installed["env"]["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"],
        "1"
    );
    assert_eq!(
        installed["env"]["CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT"],
        "1"
    );
    assert_eq!(installed["env"]["DISABLE_LOGIN_COMMAND"], "1");
    assert!(installed["env"].get("ANTHROPIC_MODEL").is_none());
    assert!(
        installed["env"]
            .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
            .is_none()
    );
    assert!(
        installed["env"]
            .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .is_none()
    );
    assert!(
        installed["env"]
            .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .is_none()
    );
    assert_eq!(installed["model"], "Claude Haiku 5-baidu");
    assert_eq!(
        installed["modelPicker"],
        serde_json::json!({
            "replaceBuiltInOptions": true,
            "options": [
                {
                    "model": "Claude Haiku 5-baidu",
                    "label": "Claude Haiku 5",
                    "description": "Baidu OneAPI"
                },
                {
                    "model": "Claude Opus 5-baidu",
                    "label": "Claude Opus 5",
                    "description": "Baidu OneAPI"
                },
                {
                    "model": "Claude Sonnet 5-baidu",
                    "label": "Claude Sonnet 5",
                    "description": "Baidu OneAPI"
                }
            ]
        })
    );
    assert_eq!(
        installed["modelOverrides"],
        serde_json::json!({
            "claude-opus-4-6": "old-opus-route",
            "unrelated-model": "keep-route"
        })
    );
    assert_eq!(
        installed["codex_mixin_managed"]["marker"].as_str().unwrap(),
        MANAGED_CLAUDE_MARKER
    );
    assert!(installed["codex_mixin_managed"].get("models").is_none());
    assert_eq!(
        installed["codex_mixin_managed"]["model_override_keys"],
        serde_json::json!([])
    );
}

fn assert_restored_claude_settings(restored: &serde_json::Value) {
    assert_eq!(restored["existing"], true);
    assert_eq!(
        restored["env"]["ANTHROPIC_BASE_URL"].as_str().unwrap(),
        "https://old"
    );
    assert_eq!(restored["env"]["ANTHROPIC_AUTH_TOKEN"], "old-token");
    assert_eq!(restored["env"]["CLAUDE_CODE_USE_GATEWAY"], "old-gateway");
    assert_eq!(
        restored["env"]["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"],
        "old-traffic"
    );
    assert_eq!(
        restored["env"]["CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT"],
        "old-window-enforcement"
    );
    assert_eq!(restored["env"]["DISABLE_LOGIN_COMMAND"], "old-login");
    assert_eq!(restored["env"]["ANTHROPIC_MODEL"], "old-model");
    assert_eq!(restored["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "old-opus");
    assert_eq!(
        restored["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"],
        "old-sonnet"
    );
    assert_eq!(
        restored["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"],
        "old-haiku"
    );
    assert_eq!(restored["model"], "old-default");
    assert_eq!(
        restored["modelPicker"],
        serde_json::json!({
            "replaceBuiltInOptions": false,
            "options": [{"model": "old-picker", "label": "Old picker"}]
        })
    );
    assert_eq!(
        restored["modelOverrides"],
        serde_json::json!({
            "claude-opus-4-6": "old-opus-route",
            "unrelated-model": "keep-route"
        })
    );
    assert!(restored.get("codex_mixin_managed").is_none());
}

#[test]
fn claude_install_uses_dedicated_client_key_as_auth_token() {
    let directory = tempfile::tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let mut provider = ProviderPreset::BaiduOneApi.create("baidu", "provider-key");
    provider.quota_username = Some("test-user".to_owned());
    provider.selected_models = vec!["Claude Sonnet 5".to_owned()];
    provider.cached_models = vec![ProviderModel {
        id: "Claude Sonnet 5".to_owned(),
        protocol: Some(ProviderProtocol::AnthropicMessages),
        ..ProviderModel::default()
    }];
    let gateway_config = GatewayConfig {
        bind: "127.0.0.1:8787".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        codex_auth_path: directory.path().join("auth.json"),
        gateway_api_key: Some("gateway-secret".to_owned()),
        gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys {
            claude: Some("claude-client-key".to_owned()),
            ..Default::default()
        },
        accept_codex_oauth: false,
        official_selected_models: None,
        default_max_tokens: 4096,
        default_context_window: 128_000,
        request_timeout: std::time::Duration::from_secs(30),
        thinking_mode: ThinkingMode::Auto,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search".to_owned(),
        web_search_max_uses: None,
        fusion_profiles: Vec::new(),
    };

    install_claude_with_config(Some(settings.clone()), &gateway_config).unwrap();

    let mut installed: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(
        installed["env"]["ANTHROPIC_AUTH_TOKEN"],
        "claude-client-key"
    );
    assert!(installed.get("modelOverrides").is_none());
    assert_eq!(installed["model"], "Claude Sonnet 5-baidu");

    installed["codex_mixin_managed"]["model_override_keys"] =
        serde_json::json!(["sonnet", "claude-sonnet-4-6"]);
    installed["modelOverrides"] = serde_json::json!({
        "sonnet": "Claude Sonnet 5-baidu",
        "claude-sonnet-4-6": "Claude Sonnet 5-baidu"
    });
    installed["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"] = "Claude Sonnet 5-baidu".into();
    fs::write(&settings, serde_json::to_vec_pretty(&installed).unwrap()).unwrap();

    install_claude_with_config(Some(settings.clone()), &gateway_config).unwrap();
    let migrated: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert!(migrated.get("modelOverrides").is_none());
    assert!(
        migrated["env"]
            .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .is_none()
    );
    uninstall_claude(Some(settings.clone())).unwrap();
    let restored: serde_json::Value = serde_json::from_slice(&fs::read(settings).unwrap()).unwrap();
    assert!(restored.get("env").is_none());
    assert!(restored.get("model").is_none());
    assert!(restored.get("modelPicker").is_none());
    assert!(restored.get("modelOverrides").is_none());
}

#[test]
fn claude_install_migrates_legacy_managed_env_backup() {
    let directory = tempfile::tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let mut provider = ProviderPreset::BaiduOneApi.create("baidu", "provider-key");
    provider.quota_username = Some("test-user".to_owned());
    provider.selected_models = vec!["Claude Sonnet 5".to_owned()];
    provider.cached_models = vec![ProviderModel {
        id: "Claude Sonnet 5".to_owned(),
        protocol: Some(ProviderProtocol::AnthropicMessages),
        ..ProviderModel::default()
    }];
    let gateway_config = GatewayConfig {
        bind: "127.0.0.1:8787".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        codex_auth_path: directory.path().join("auth.json"),
        gateway_api_key: Some("gateway-secret".to_owned()),
        gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys {
            claude: Some("claude-client-key".to_owned()),
            ..Default::default()
        },
        accept_codex_oauth: false,
        official_selected_models: None,
        default_max_tokens: 4096,
        default_context_window: 128_000,
        request_timeout: std::time::Duration::from_secs(30),
        thinking_mode: ThinkingMode::Auto,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search".to_owned(),
        web_search_max_uses: None,
        fusion_profiles: Vec::new(),
    };

    install_claude_with_config(Some(settings.clone()), &gateway_config).unwrap();
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    legacy["codex_mixin_managed"]
        .as_object_mut()
        .unwrap()
        .remove("env_keys");
    legacy["codex_mixin_managed"]
        .as_object_mut()
        .unwrap()
        .remove("model_override_keys");
    legacy["codex_mixin_managed"]
        .as_object_mut()
        .unwrap()
        .remove("model_picker_managed");
    legacy["codex_mixin_managed"]
        .as_object_mut()
        .unwrap()
        .remove("previous_model_picker");
    legacy["env"]["ANTHROPIC_AUTH_TOKEN"] = "manual-token".into();
    legacy["modelOverrides"]["claude-sonnet-4-6"] = "manual-route".into();
    legacy["modelPicker"] = serde_json::json!({
        "replaceBuiltInOptions": false,
        "options": [{"model": "manual-picker", "label": "Manual picker"}]
    });
    fs::write(&settings, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();

    install_claude_with_config(Some(settings.clone()), &gateway_config).unwrap();
    uninstall_claude(Some(settings.clone())).unwrap();

    let restored: serde_json::Value = serde_json::from_slice(&fs::read(settings).unwrap()).unwrap();
    assert_eq!(restored["env"]["ANTHROPIC_AUTH_TOKEN"], "manual-token");
    assert_eq!(
        restored["modelOverrides"]["claude-sonnet-4-6"],
        "manual-route"
    );
    assert_eq!(
        restored["modelPicker"],
        serde_json::json!({
            "replaceBuiltInOptions": false,
            "options": [{"model": "manual-picker", "label": "Manual picker"}]
        })
    );
}

#[test]
fn claude_connect_has_no_model_mapping_options() {
    assert!(Cli::try_parse_from(["codex-mixin", "connect", "claude"]).is_ok());
    assert!(Cli::try_parse_from(["codex-mixin", "install-claude"]).is_ok());
    assert!(
        Cli::try_parse_from(["codex-mixin", "connect", "claude", "--model", "one-backend",])
            .is_err()
    );
}

#[test]
fn claude_install_accepts_any_routable_model_protocol() {
    let directory = tempfile::tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let mut provider = ProviderPreset::BaiduOneApi.create("baidu", "key");
    provider.quota_username = Some("test-user".to_owned());
    provider.selected_models = vec!["gpt-5.6-sol".to_owned()];
    provider.cached_models = vec![ProviderModel {
        id: "gpt-5.6-sol".to_owned(),
        context_window: Some(272_000),
        protocol: Some(ProviderProtocol::OpenAiResponses),
        ..ProviderModel::default()
    }];
    let gateway_config = GatewayConfig {
        bind: "127.0.0.1:8787".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        codex_auth_path: directory.path().join("auth.json"),
        gateway_api_key: None,
        gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys {
            claude: Some("claude-client-key".to_owned()),
            ..Default::default()
        },
        accept_codex_oauth: false,
        official_selected_models: None,
        default_max_tokens: 4096,
        default_context_window: 128_000,
        request_timeout: std::time::Duration::from_secs(30),
        thinking_mode: ThinkingMode::Auto,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search".to_owned(),
        web_search_max_uses: None,
        fusion_profiles: Vec::new(),
    };

    install_claude_with_config(Some(settings.clone()), &gateway_config).unwrap();

    let installed: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(installed["model"], "gpt-5.6-sol-baidu");
    assert_eq!(
        installed["modelPicker"]["options"][0],
        serde_json::json!({
            "model": "gpt-5.6-sol-baidu",
            "label": "gpt-5.6-sol",
            "description": "Baidu OneAPI · 272K context"
        })
    );
    assert!(installed.get("modelOverrides").is_none());
    assert!(
        installed["env"]
            .as_object()
            .unwrap()
            .keys()
            .all(|key| !key.starts_with("ANTHROPIC_DEFAULT_"))
    );
}

#[test]
fn claude_install_marks_extended_context_models_without_family_mappings() {
    let directory = tempfile::tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let mut provider = ProviderPreset::BaiduOneApi.create("baidu", "provider-key");
    provider.quota_username = Some("test-user".to_owned());
    provider.selected_models = vec!["DeepSeek-V4-Flash".to_owned(), "GLM-5.3".to_owned()];
    provider.cached_models = vec![
        ProviderModel {
            id: "DeepSeek-V4-Flash".to_owned(),
            context_window: Some(1_024_000),
            protocol: Some(ProviderProtocol::AnthropicMessages),
            ..ProviderModel::default()
        },
        ProviderModel {
            id: "GLM-5.3".to_owned(),
            context_window: Some(1_000_000),
            protocol: Some(ProviderProtocol::AnthropicMessages),
            ..ProviderModel::default()
        },
    ];
    let gateway_config = GatewayConfig {
        bind: "127.0.0.1:8787".parse().unwrap(),
        providers: vec![provider],
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        codex_auth_path: directory.path().join("auth.json"),
        gateway_api_key: None,
        gateway_client_keys: codex_mixin::gateway_access::GatewayClientKeys {
            claude: Some("claude-client-key".to_owned()),
            ..Default::default()
        },
        accept_codex_oauth: false,
        official_selected_models: None,
        default_max_tokens: 4096,
        default_context_window: 128_000,
        request_timeout: std::time::Duration::from_secs(30),
        thinking_mode: ThinkingMode::Auto,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search".to_owned(),
        web_search_max_uses: None,
        fusion_profiles: Vec::new(),
    };

    install_claude_with_config(Some(settings.clone()), &gateway_config).unwrap();

    let installed: serde_json::Value =
        serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(
        installed["modelPicker"],
        serde_json::json!({
            "replaceBuiltInOptions": true,
            "options": [
                {
                    "model": "DeepSeek-V4-Flash-baidu[1m]",
                    "label": "DeepSeek-V4-Flash",
                    "description": "Baidu OneAPI · 1024K context"
                },
                {
                    "model": "GLM-5.3-baidu[1m]",
                    "label": "GLM-5.3",
                    "description": "Baidu OneAPI · 1M context"
                }
            ]
        })
    );
    assert_eq!(installed["model"], "DeepSeek-V4-Flash-baidu[1m]");
    assert!(installed.get("modelOverrides").is_none());
    assert!(
        installed["env"]
            .as_object()
            .unwrap()
            .keys()
            .all(|key| !key.starts_with("ANTHROPIC_DEFAULT_"))
    );
}
