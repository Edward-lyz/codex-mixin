use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};

use crate::provider::{CONFIG_VERSION, ProviderDefinition, ProviderRegistry};

mod context;
mod cors;
mod fusion;
pub use context::ContextSettings;
pub(crate) use cors::{valid_cors_origin, validate_cors_origins};
pub use fusion::{
    FUSION_MODEL_PREFIX, FusionMode, FusionProfile, FusionTimeRoute, OFFICIAL_MODEL_PREFIX,
    PanelToolsConfig, validate_fusion_model_references, validate_fusion_profiles,
};
mod migration;
mod storage;
pub use storage::{
    delete_stored_config, ensure_compaction_secret, ensure_gateway_client_key,
    export_stored_config, gateway_client_key_exists, import_stored_config, load_stored_config,
    load_stored_config_from_path, mutate_stored_config, mutate_stored_config_at_path,
    revoke_gateway_client_key, save_stored_config, save_stored_config_to_path, stored_config_path,
};

pub use crate::provider::{
    ProviderAuthHeader as UpstreamAuthHeader, ProviderPreset, ProviderProtocol as UpstreamKind,
};

pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 16_384;

/// Default listener: all interfaces with an automatically chosen port.
pub const DEFAULT_GATEWAY_BIND: &str = "0.0.0.0:0";

/// Address clients should dial for a gateway listening on `bind`.
///
/// The unspecified address (`0.0.0.0` / `::`) is valid for listening but not
/// a portable destination, so clients on this host use loopback instead.
pub fn client_address(bind: SocketAddr) -> SocketAddr {
    match bind.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bind.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bind.port())
        }
        _ => bind,
    }
}

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
    pub official_ech_proxy: bool,
    pub official_ech_fallback_reason: Option<String>,
    pub codex_auth_path: PathBuf,
    pub gateway_api_key: Option<String>,
    pub cors_allowed_origins: Vec<String>,
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
    pub context: ContextSettings,
}

impl GatewayConfig {
    pub fn from_stored_config() -> anyhow::Result<Self> {
        let stored_config = match load_stored_config()? {
            Some(config) => config,
            None => {
                anyhow::ensure!(
                    crate::provider::official_auth_available(&default_codex_auth_path())?
                        || !crate::provider::local_oauth::discover(&[])?.is_empty(),
                    "provider configuration is missing; run providers add --preset openai or add another provider"
                );
                StoredGatewayConfig::default()
            }
        };
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

    fn from_stored_config_value(mut stored_config: StoredGatewayConfig) -> anyhow::Result<Self> {
        ensure_config_version(stored_config.config_version)?;
        validate_cors_origins(&stored_config.cors_allowed_origins)?;
        // Local AGY/Kiro logins join the runtime view without being saved.
        stored_config.providers =
            crate::provider::local_oauth::runtime_providers(stored_config.providers)?;
        stored_config.apply_model_order();
        if stored_config.providers.is_empty()
            && !crate::provider::official_auth_available(&default_codex_auth_path())?
        {
            anyhow::bail!(
                "provider configuration is empty; run `codex-mixin provider add --preset <preset> --key <key>`"
            );
        }
        ProviderRegistry::new(stored_config.providers.clone())?;
        let bind = stored_config
            .gateway_bind
            .clone()
            .unwrap_or_else(|| DEFAULT_GATEWAY_BIND.to_owned())
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
            cors_allowed_origins: stored_config.cors_allowed_origins,
            gateway_client_keys: stored_config.gateway_client_keys,
            accept_codex_oauth: true,
            official_ech_fallback_reason: stored_config.official_ech_fallback_reason,
            official_ech_proxy: stored_config.official_ech_proxy,
            official_selected_models: stored_config.official_selected_models,
            default_max_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            default_context_window: 1_000_000,
            request_timeout: Duration::from_millis(600_000),
            thinking_mode: ThinkingMode::Auto,
            enable_web_search_tool: true,
            web_search_tool_type: "web_search_20250305".to_owned(),
            web_search_max_uses: Some(3),
            fusion_profiles,
            context: stored_config.context,
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
    crate::platform::oauth::official_auth_path()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredGatewayConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_ech_fallback_reason: Option<String>,
    #[serde(default)]
    pub official_ech_proxy: bool,
    pub config_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_bind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_api_key: Option<String>,
    #[serde(default = "cors::default_cors_origins")]
    pub cors_allowed_origins: Vec<String>,
    #[serde(default, skip_serializing_if = "gateway_client_keys_are_empty")]
    pub gateway_client_keys: crate::gateway_access::GatewayClientKeys,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_selected_models: Option<Vec<String>>,
    #[serde(default)]
    pub official_model_order: crate::provider::ModelOrder,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub official_model_contexts: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fusion_profiles: Vec<FusionProfile>,
    #[serde(default, skip_serializing_if = "ContextSettings::is_default")]
    pub context: ContextSettings,
    pub providers: Vec<ProviderDefinition>,
}

impl StoredGatewayConfig {
    pub(crate) fn apply_model_order(&mut self) {
        for provider in &mut self.providers {
            provider.apply_model_order();
        }
        if let Some(models) = &mut self.official_selected_models {
            self.official_model_order.apply(models);
        }
    }
}

impl Default for StoredGatewayConfig {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            gateway_bind: None,
            gateway_api_key: None,
            cors_allowed_origins: cors::default_cors_origins(),
            gateway_client_keys: crate::gateway_access::GatewayClientKeys::default(),
            compaction_secret: None,
            official_ech_fallback_reason: None,
            official_ech_proxy: false,
            official_selected_models: None,
            official_model_order: Default::default(),
            official_model_contexts: BTreeMap::new(),
            fusion_profiles: Vec::new(),
            context: ContextSettings::default(),
            providers: Vec::new(),
        }
    }
}

fn gateway_client_keys_are_empty(keys: &crate::gateway_access::GatewayClientKeys) -> bool {
    keys.codex.is_none()
        && keys.claude.is_none()
        && keys.claude_desktop.is_none()
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
