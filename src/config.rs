use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};

use crate::fusion::{FusionProfile, validate_fusion_profiles};
use crate::provider::{CONFIG_VERSION, ProviderDefinition, ProviderRegistry};

mod migration;
mod storage;
pub use storage::{
    delete_stored_config, ensure_compaction_secret, ensure_gateway_client_key,
    export_stored_config, gateway_client_key_exists, load_stored_config,
    load_stored_config_from_path, mutate_stored_config, mutate_stored_config_at_path,
    revoke_gateway_client_key, save_stored_config, save_stored_config_to_path, stored_config_path,
};

pub use crate::provider::{
    ProviderAuthHeader as UpstreamAuthHeader, ProviderPreset, ProviderProtocol as UpstreamKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingMode {
    Off,
    Manual,
    Adaptive,
    Auto,
}

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub bind: SocketAddr,
    pub providers: Vec<ProviderDefinition>,
    pub official_responses_url: String,
    pub codex_auth_path: PathBuf,
    pub gateway_api_key: Option<String>,
    pub gateway_client_keys: crate::gateway_access::GatewayClientKeys,
    pub accept_codex_oauth: bool,
    pub official_selected_models: Option<Vec<String>>,
    pub default_max_tokens: u64,
    pub default_context_window: u64,
    pub request_timeout: Duration,
    pub thinking_mode: ThinkingMode,
    pub enable_web_search_tool: bool,
    pub web_search_tool_type: String,
    pub web_search_max_uses: Option<u64>,
    pub fusion_profiles: Vec<FusionProfile>,
}

impl GatewayConfig {
    pub fn from_stored_config() -> anyhow::Result<Self> {
        let stored_config = load_stored_config()?.ok_or_else(|| {
            anyhow!(
                "provider configuration is missing; run `codex-mixin provider add --preset <preset> --key <key>`"
            )
        })?;
        Self::from_stored_config_value(stored_config)
    }

    /// Fetch the stored gateway key for a client, validating its shape.
    pub fn require_client_key(
        &self,
        client: crate::gateway_access::GatewayClient,
    ) -> anyhow::Result<String> {
        let name = client.display_name();
        let key = self
            .gateway_client_keys
            .get(client)
            .ok_or_else(|| anyhow!("{name} client key is missing"))?;
        anyhow::ensure!(
            !key.trim().is_empty(),
            "{name} client key must not be empty"
        );
        anyhow::ensure!(key == key.trim(), "{name} client key has whitespace");
        Ok(key.to_owned())
    }

    fn from_stored_config_value(stored_config: StoredGatewayConfig) -> anyhow::Result<Self> {
        ensure_config_version(stored_config.config_version)?;
        if stored_config.providers.is_empty() {
            anyhow::bail!(
                "provider configuration is empty; run `codex-mixin provider add --preset <preset> --key <key>`"
            );
        }
        ProviderRegistry::new(stored_config.providers.clone())?;
        let bind = stored_config
            .gateway_bind
            .clone()
            .unwrap_or_else(|| "127.0.0.1:8787".to_owned())
            .parse()
            .context("invalid stored gateway bind")?;
        let mut fusion_profiles = stored_config.fusion_profiles;
        migrate_legacy_fusion_panel_tool_limits(&mut fusion_profiles);
        let config = Self {
            bind,
            providers: stored_config.providers,
            official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
            codex_auth_path: default_codex_auth_path(),
            gateway_api_key: stored_config.gateway_api_key,
            gateway_client_keys: stored_config.gateway_client_keys,
            accept_codex_oauth: true,
            official_selected_models: stored_config.official_selected_models,
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_millis(600_000),
            thinking_mode: ThinkingMode::Auto,
            enable_web_search_tool: true,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            fusion_profiles,
        };
        validate_fusion_profiles(&config.fusion_profiles)?;
        Ok(config)
    }

    pub fn official_image_generation_url(&self) -> anyhow::Result<String> {
        self.official_codex_url("images/generations")
    }

    pub fn official_image_edit_url(&self) -> anyhow::Result<String> {
        self.official_codex_url("images/edits")
    }

    fn official_codex_url(&self, path: &str) -> anyhow::Result<String> {
        let base = self
            .official_responses_url
            .strip_suffix("/responses")
            .ok_or_else(|| {
                anyhow!(
                    "official responses URL must end with /responses: {}",
                    self.official_responses_url
                )
            })?;
        Ok(format!("{base}/{path}"))
    }
}

fn migrate_legacy_fusion_panel_tool_limits(profiles: &mut [FusionProfile]) {
    for profile in profiles {
        if profile.panel_tools.max_rounds == 4 && profile.panel_tools.max_calls_per_model == 8 {
            profile.panel_tools.max_rounds = 16;
            profile.panel_tools.max_calls_per_model = 64;
        }
    }
}

fn default_codex_auth_path() -> PathBuf {
    codex_home_path().join("auth.json")
}

fn codex_home_path() -> PathBuf {
    std::env::var("CODEX_HOME").ok().map_or_else(
        || {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".codex")
        },
        PathBuf::from,
    )
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredGatewayConfig {
    pub config_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_bind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "gateway_client_keys_are_empty")]
    pub gateway_client_keys: crate::gateway_access::GatewayClientKeys,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_selected_models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fusion_profiles: Vec<FusionProfile>,
    pub providers: Vec<ProviderDefinition>,
}

impl Default for StoredGatewayConfig {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            gateway_bind: None,
            gateway_api_key: None,
            gateway_client_keys: crate::gateway_access::GatewayClientKeys::default(),
            compaction_secret: None,
            official_selected_models: None,
            fusion_profiles: Vec::new(),
            providers: Vec::new(),
        }
    }
}

fn gateway_client_keys_are_empty(keys: &crate::gateway_access::GatewayClientKeys) -> bool {
    keys.codex.is_none()
        && keys.claude.is_none()
        && keys.dsh.is_none()
        && keys.opencode.is_none()
        && keys.pi.is_none()
}

pub fn ensure_config_version(version: u32) -> anyhow::Result<()> {
    if version != CONFIG_VERSION {
        anyhow::bail!(
            "unsupported config version {version}; expected {CONFIG_VERSION}. Recreate the provider configuration"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::provider::{ProviderProtocol, ProviderQuotaParser};

    use super::migration::parse_stored_config;
    use super::*;

    #[test]
    fn saves_and_loads_stored_gateway_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = StoredGatewayConfig {
            config_version: CONFIG_VERSION,
            gateway_bind: Some("127.0.0.1:18787".to_owned()),
            gateway_api_key: Some("local-key".to_owned()),
            gateway_client_keys: crate::gateway_access::GatewayClientKeys::default(),
            compaction_secret: None,
            official_selected_models: Some(vec!["gpt-5.6-sol".to_owned()]),
            fusion_profiles: Vec::new(),
            providers: vec![crate::provider::open_code_go_provider(
                "opencode-go",
                "opencode-key",
            )],
        };
        save_stored_config_to_path(&path, &config).unwrap();
        let encrypted = fs::read_to_string(&path).unwrap();
        assert!(encrypted.contains("\"encryption\": \"aes-256-gcm\""));
        assert!(!encrypted.contains("local-key"));
        assert!(!encrypted.contains("opencode-key"));
        assert!(path.with_file_name("config.json.key").exists());
        let loaded = load_stored_config_from_path(&path).unwrap().unwrap();
        assert_eq!(loaded.config_version, CONFIG_VERSION);
        assert_eq!(loaded.gateway_bind.as_deref(), Some("127.0.0.1:18787"));
        assert_eq!(loaded.gateway_api_key.as_deref(), Some("local-key"));
        assert_eq!(
            loaded.official_selected_models.as_ref().unwrap(),
            &["gpt-5.6-sol".to_owned()]
        );
        assert_eq!(loaded.providers[0].id, "opencode-go");
        assert!(!loaded.providers[0].auxiliary_model_upstream);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn encrypted_config_fails_closed_without_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        save_stored_config_to_path(&path, &StoredGatewayConfig::default()).unwrap();
        fs::remove_file(path.with_file_name("config.json.key")).unwrap();

        let error = load_stored_config_from_path(&path).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("config encryption key is missing")
        );
    }

    #[test]
    fn rejects_unrecognized_missing_or_wrong_config_version() {
        assert!(serde_json::from_str::<StoredGatewayConfig>("{}").is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(
            &path,
            r#"{"config_version":1,"providers":[],"fusion_profiles":[]}"#,
        )
        .unwrap();
        assert!(
            load_stored_config_from_path(&path)
                .unwrap_err()
                .to_string()
                .contains("unsupported config version")
        );
    }

    #[test]
    fn reads_and_encrypts_legacy_single_provider_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let legacy = r#"{
          "gateway_bind": "127.0.0.1:18787",
          "provider_preset": "baidu-oneapi",
          "upstream_kind": "anthropic_messages",
          "upstream_base_url": "https://oneapi.example/v1/",
          "upstream_messages_path": "/v1/messages",
          "upstream_models_path": "/v1/models",
          "upstream_image_generation_path": "/v1/images/generations",
          "upstream_api_key": "legacy-secret",
          "gateway_api_key": "gateway-secret",
          "quota_url": "https://oneapi.example/quota",
          "quota_username": "legacy-user"
        }"#;
        fs::write(&path, legacy).unwrap();

        let loaded = load_stored_config_from_path(&path).unwrap().unwrap();

        assert_eq!(loaded.config_version, CONFIG_VERSION);
        assert_eq!(loaded.gateway_bind.as_deref(), Some("127.0.0.1:18787"));
        assert_eq!(loaded.gateway_api_key.as_deref(), Some("gateway-secret"));
        assert_eq!(loaded.providers.len(), 1);
        let provider = &loaded.providers[0];
        assert_eq!(provider.id, "baidu-oneapi");
        assert_eq!(provider.base_url, "https://oneapi.example");
        assert_eq!(provider.api_path, "/v1/messages");
        assert_eq!(provider.auth.api_key, "legacy-secret");
        assert_eq!(provider.quota_username.as_deref(), Some("legacy-user"));
        assert_eq!(provider.models_refreshed_at_ms, None);
        assert!(provider.selected_models.is_empty());
        let encrypted = fs::read_to_string(&path).unwrap();
        assert!(encrypted.contains("\"encryption\": \"aes-256-gcm\""));
        assert!(!encrypted.contains("legacy-secret"));
        let backup = fs::read_to_string(path.with_file_name("config.json.v1.backup")).unwrap();
        assert!(backup.contains("\"encryption\": \"aes-256-gcm\""));
        assert!(!backup.contains("legacy-secret"));
    }

    #[test]
    fn migrates_legacy_ducc_config_to_disabled_reporting() {
        let raw = r#"{
          "config_version": 2,
          "providers": [{
            "id": "baidu-oneapi",
            "display_name": "Baidu OneAPI",
            "enabled": true,
            "preset_id": "baidu-oneapi",
            "protocol": "anthropic_messages",
            "base_url": "https://oneapi.example",
            "api_path": "/v1/messages",
            "model_source": {"kind": "baidu_one_api"},
            "auth": {"header": "authorization_bearer", "api_key": "key"},
            "quota_parser": "baidu_one_api",
            "request_policy": {
              "baidu_auth_bridge": "ducc_loopback",
              "ducc_executable": "/old/ducc",
              "data_report_executable": "/old/data-report",
              "baidu_code_report": true
            }
          }]
        }"#;
        let parsed = parse_stored_config(raw).unwrap();
        let policy = &parsed.providers[0].request_policy;
        assert_eq!(
            policy.effective_baidu_auth_bridge(),
            crate::provider::BaiduAuthBridge::Disabled
        );
        assert!(!policy.baidu_code_report);
        assert!(policy.ducx_executable.is_none());
        assert!(policy.data_report_executable.is_none());
        assert!(policy.data_report_client_token.is_none());
    }

    #[test]
    fn upgrades_existing_deepseek_provider_with_balance_endpoint_once() {
        let mut provider = crate::provider::deepseek_provider("deepseek", "secret");
        provider.quota_url = None;
        provider.quota_parser = ProviderQuotaParser::Generic;
        let stored = StoredGatewayConfig {
            providers: vec![provider],
            ..StoredGatewayConfig::default()
        };

        let mut loaded = parse_stored_config(&serde_json::to_string(&stored).unwrap()).unwrap();
        let provider = &mut loaded.providers[0];
        assert_eq!(
            provider.quota_url.as_deref(),
            Some("https://api.deepseek.com/user/balance")
        );
        assert_eq!(provider.quota_parser, ProviderQuotaParser::DeepSeek);

        provider.quota_url = None;
        let loaded = parse_stored_config(&serde_json::to_string(&loaded).unwrap()).unwrap();
        assert_eq!(loaded.providers[0].quota_url, None);
        assert_eq!(
            loaded.providers[0].quota_parser,
            ProviderQuotaParser::DeepSeek
        );
    }

    #[test]
    fn backfills_report_executable_for_existing_baidu_reporting() {
        let mut provider = crate::provider::baidu_oneapi_provider("baidu-oneapi", "secret");
        provider.quota_username = Some("user@example.com".to_owned());
        provider.request_policy.baidu_code_report = true;
        provider.request_policy.ducx_executable =
            Some("/Users/example/.codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx".into());
        let stored = StoredGatewayConfig {
            providers: vec![provider],
            ..StoredGatewayConfig::default()
        };

        let loaded = parse_stored_config(&serde_json::to_string(&stored).unwrap()).unwrap();

        assert_eq!(
            loaded.providers[0].request_policy.data_report_executable,
            Some(
                "/Users/example/.codex-mixin/ducx/home/.baidu-cx/baidu-cx/hooks/data-report".into()
            )
        );
    }

    #[test]
    fn upgrades_existing_opencode_go_provider_to_dashboard_parser_and_responses_endpoint() {
        let mut provider = crate::provider::open_code_go_provider("opencode-go", "secret");
        provider.quota_url = None;
        provider.quota_currency = None;
        provider.quota_parser = ProviderQuotaParser::Generic;
        provider.protocol = ProviderProtocol::OpenAiChat;
        provider.api_path = "/v1/chat/completions".to_owned();
        let stored = StoredGatewayConfig {
            providers: vec![provider],
            ..StoredGatewayConfig::default()
        };

        let loaded = parse_stored_config(&serde_json::to_string(&stored).unwrap()).unwrap();

        assert_eq!(
            loaded.providers[0].quota_parser,
            ProviderQuotaParser::OpenCodeGo
        );
        assert_eq!(loaded.providers[0].quota_currency.as_deref(), Some("USD"));
        assert_eq!(
            loaded.providers[0].protocol,
            ProviderProtocol::OpenAiResponses
        );
        assert_eq!(loaded.providers[0].api_path, "/v1/responses");
    }

    #[test]
    fn selected_models_bootstrap_an_unrefreshed_empty_cache() {
        let mut provider = crate::provider::baidu_oneapi_provider("baidu-oneapi", "secret");
        provider.quota_username = Some("user@example.com".to_owned());
        provider.selected_models = vec!["GLM-5.2".to_owned(), "gpt-5.6-luna".to_owned()];
        assert!(provider.cached_models.is_empty());
        assert_eq!(provider.models_refreshed_at_ms, None);
        let stored = StoredGatewayConfig {
            providers: vec![provider],
            ..StoredGatewayConfig::default()
        };

        let loaded = parse_stored_config(&serde_json::to_string(&stored).unwrap()).unwrap();
        let provider = &loaded.providers[0];

        assert_eq!(
            provider
                .cached_models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["GLM-5.2", "gpt-5.6-luna"]
        );
        assert_eq!(provider.readiness().routable_model_count, 2);
        let registry = ProviderRegistry::new(loaded.providers).unwrap();
        assert!(registry.resolve("GLM-5.2-baidu-oneapi").is_some());
        assert!(registry.resolve("gpt-5.6-luna-baidu-oneapi").is_some());
    }

    #[test]
    fn refreshed_empty_cache_does_not_restore_unavailable_models() {
        let mut provider = crate::provider::baidu_oneapi_provider("baidu-oneapi", "secret");
        provider.quota_username = Some("user@example.com".to_owned());
        provider.selected_models = vec!["removed-model".to_owned()];
        provider.models_refreshed_at_ms = Some(1);
        let stored = StoredGatewayConfig {
            providers: vec![provider],
            ..StoredGatewayConfig::default()
        };

        let loaded = parse_stored_config(&serde_json::to_string(&stored).unwrap()).unwrap();
        let provider = &loaded.providers[0];

        assert!(provider.cached_models.is_empty());
        assert_eq!(provider.readiness().routable_model_count, 0);
        assert_eq!(provider.readiness().unavailable_selected_model_count, 1);
    }

    #[test]
    fn backs_up_legacy_config_before_first_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let legacy = r#"{
          "provider_preset": "deepseek",
          "upstream_base_url": "https://api.deepseek.com",
          "upstream_api_key": "legacy-secret"
        }"#;
        fs::write(&path, legacy).unwrap();

        mutate_stored_config_at_path(&path, |config| {
            config.gateway_bind = Some("127.0.0.1:18787".to_owned());
            Ok(())
        })
        .unwrap();

        let backup = path.with_file_name("config.json.v1.backup");
        let encrypted_backup = fs::read_to_string(backup).unwrap();
        assert!(encrypted_backup.contains("\"encryption\": \"aes-256-gcm\""));
        assert!(!encrypted_backup.contains("legacy-secret"));
        let stored = load_stored_config_from_path(&path).unwrap().unwrap();
        assert_eq!(stored.config_version, CONFIG_VERSION);
        assert_eq!(stored.providers[0].id, "deepseek");
    }

    #[test]
    fn saves_multiple_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let primary = crate::provider::open_code_go_provider("primary", "one");
        let mut backup = crate::provider::open_code_go_provider("backup", "two");
        backup.auxiliary_model_upstream = true;
        let config = StoredGatewayConfig {
            providers: vec![primary, backup],
            ..StoredGatewayConfig::default()
        };
        save_stored_config_to_path(&path, &config).unwrap();
        let loaded = load_stored_config_from_path(&path).unwrap().unwrap();
        assert_eq!(
            loaded
                .providers
                .iter()
                .map(|provider| provider.id.as_str())
                .collect::<Vec<_>>(),
            ["primary", "backup"]
        );
        assert!(!loaded.providers[0].auxiliary_model_upstream);
        assert!(loaded.providers[1].auxiliary_model_upstream);
    }

    #[test]
    fn serializes_provider_mutations_with_a_config_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        save_stored_config_to_path(&path, &StoredGatewayConfig::default()).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let handles = ["first", "second"].map(|id| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                mutate_stored_config_at_path(&path, |config| {
                    config
                        .providers
                        .push(crate::provider::open_code_go_provider(id, "secret"));
                    Ok(())
                })
                .unwrap();
            })
        });
        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }
        let config = load_stored_config_from_path(&path).unwrap().unwrap();
        assert_eq!(config.providers.len(), 2);
        assert!(
            config
                .providers
                .iter()
                .any(|provider| provider.id == "first")
        );
        assert!(
            config
                .providers
                .iter()
                .any(|provider| provider.id == "second")
        );
    }

    #[test]
    fn rejects_stored_fusion_references_to_unavailable_provider_models() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = StoredGatewayConfig {
            providers: vec![crate::provider::open_code_go_provider(
                "opencode-go",
                "secret",
            )],
            fusion_profiles: vec![FusionProfile {
                id: "invalid".to_owned(),
                panel_models: vec!["missing-opencode-go".to_owned()],
                judge_model: "glm-5.2-opencode-go".to_owned(),
                final_model: "glm-5.2-opencode-go".to_owned(),
                min_successful: 1,
                max_completion_tokens: 2048,
                timeout_ms: 30_000,
                show_intermediate_results: true,
                panel_tools: crate::fusion::PanelToolsConfig::default(),
            }],
            ..StoredGatewayConfig::default()
        };
        let error = save_stored_config_to_path(&path, &config).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("references unavailable provider model missing-opencode-go")
        );
        assert!(!path.exists());
    }

    #[test]
    fn upgrades_legacy_stored_fusion_panel_tool_limits() {
        let stored = StoredGatewayConfig {
            providers: vec![crate::provider::open_code_go_provider("provider", "secret")],
            fusion_profiles: vec![FusionProfile {
                id: "legacy".to_owned(),
                panel_models: vec!["panel-provider".to_owned()],
                judge_model: "judge-provider".to_owned(),
                final_model: "final-provider".to_owned(),
                min_successful: 1,
                max_completion_tokens: 2048,
                timeout_ms: 30_000,
                show_intermediate_results: true,
                panel_tools: crate::fusion::PanelToolsConfig {
                    max_rounds: 4,
                    max_calls_per_model: 8,
                    ..Default::default()
                },
            }],
            ..StoredGatewayConfig::default()
        };

        let config = GatewayConfig::from_stored_config_value(stored).unwrap();

        assert_eq!(config.fusion_profiles[0].panel_tools.max_rounds, 16);
        assert_eq!(
            config.fusion_profiles[0].panel_tools.max_calls_per_model,
            64
        );
    }

    #[test]
    fn resolves_official_image_generation_urls() {
        let config = GatewayConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            providers: vec![crate::provider::open_code_go_provider("opencode-go", "key")],
            official_responses_url: "https://chatgpt.example/backend-api/codex/responses"
                .to_owned(),
            codex_auth_path: PathBuf::from("/tmp/auth.json"),
            gateway_api_key: None,
            gateway_client_keys: crate::gateway_access::GatewayClientKeys::default(),
            accept_codex_oauth: true,
            official_selected_models: None,
            default_max_tokens: 8192,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_secs(30),
            thinking_mode: ThinkingMode::Off,
            enable_web_search_tool: false,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            fusion_profiles: Vec::new(),
        };
        assert_eq!(
            config.official_image_generation_url().unwrap(),
            "https://chatgpt.example/backend-api/codex/images/generations"
        );
        assert_eq!(
            config.official_image_edit_url().unwrap(),
            "https://chatgpt.example/backend-api/codex/images/edits"
        );
    }
}
