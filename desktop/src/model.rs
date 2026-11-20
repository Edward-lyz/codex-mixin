//! Typed views of the CLI JSON contract. Fields the shell does not use are
//! ignored; required fields fail decoding so unknown states never render as
//! "off" or "zero".

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServiceStatus {
    pub configured: bool,
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub gateway_version: Option<String>,
    #[serde(default)]
    pub provider_readiness: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub official_ech: Option<EchStatus>,
    #[serde(default)]
    providers: ServiceProviders,
}

impl ServiceStatus {
    pub fn is_running(&self) -> bool {
        self.gateway.as_deref() == Some("running")
    }

    pub fn degraded_providers(&self) -> usize {
        match &self.providers {
            ServiceProviders::List(providers) => providers
                .iter()
                .filter(|provider| provider.enabled && provider.readiness.status == "degraded")
                .count(),
            ServiceProviders::Summary { degraded, .. } => *degraded,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum ServiceProviders {
    List(Vec<ServiceProvider>),
    Summary {
        #[serde(rename = "total")]
        _total: usize,
        #[serde(rename = "healthy")]
        _healthy: usize,
        degraded: usize,
        #[serde(rename = "disabled")]
        _disabled: usize,
    },
}

impl Default for ServiceProviders {
    fn default() -> Self {
        Self::List(Vec::new())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServiceProvider {
    pub enabled: bool,
    pub readiness: ServiceReadiness,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServiceReadiness {
    pub status: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct EchStatus {
    pub enabled: bool,
    #[serde(default)]
    pub fallback_reason: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProviderList {
    pub providers: Vec<Provider>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Provider {
    pub id: String,
    pub display_name: String,
    pub kind: String,
    pub enabled: bool,
    pub readiness: String,
    pub icon: String,
    pub protocol: String,
    pub selected_models: Vec<String>,
    pub cached_models: Vec<ProviderModel>,
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub website_url: Option<String>,
    #[serde(default)]
    pub api_path: Option<String>,
    #[serde(default)]
    pub image_generation_path: Option<String>,
    #[serde(default)]
    pub api_key_configured: bool,
    #[serde(default)]
    pub model_order: Option<String>,
    #[serde(default)]
    pub model_context_overrides: BTreeMap<String, u64>,
    #[serde(default)]
    pub models_refreshed_at_ms: Option<i64>,
    #[serde(default)]
    pub last_model_refresh_error: Option<String>,
    #[serde(default)]
    pub unavailable_selected_models: Vec<String>,
    #[serde(default)]
    pub model_refresh: Option<ModelRefresh>,
    #[serde(default)]
    pub quota_username: Option<String>,
    #[serde(default)]
    pub quota_workspace_id: Option<String>,
    #[serde(default)]
    pub aws_region: Option<String>,
    #[serde(default)]
    pub baidu_auth_bridge: Option<String>,
    #[serde(default)]
    pub baidu_code_report: bool,
    #[serde(default)]
    pub auto_review_model: Option<String>,
    #[serde(default)]
    pub codex_thread_title_model: Option<String>,
}

impl Provider {
    pub fn is_official(&self) -> bool {
        self.kind == "official"
    }

    pub fn is_healthy(&self) -> bool {
        self.enabled && self.readiness == "healthy"
    }
}

/// Background model-list refresh policy for one provider.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ModelRefresh {
    pub enabled: bool,
    pub interval_seconds: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProviderModel {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub supports_image: Option<bool>,
    #[serde(default)]
    pub supports_function_tools: Option<bool>,
    #[serde(default)]
    pub supports_thinking: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Interface {
    pub paths: InterfacePaths,
    pub provider_catalog: ProviderCatalog,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct InterfacePaths {
    pub state: String,
    pub gateway_log: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProviderCatalog {
    pub default_preset: String,
    pub default_aws_region: String,
    pub oauth_timeout_seconds: u64,
    pub presets: Vec<ProviderPreset>,
    #[serde(default)]
    pub protocols: Vec<ProtocolOption>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProviderPreset {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub icon: String,
    pub auth_mode: String,
    pub allow_multiple: bool,
    pub api_key_required: bool,
    pub custom_endpoint: bool,
    pub quota_credentials_required: bool,
    pub quota_username_required: bool,
    #[serde(default)]
    pub browser_oauth: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProtocolOption {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct QuotaEntry {
    pub label: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub limit: Option<f64>,
    #[serde(default)]
    pub used: Option<f64>,
    #[serde(default)]
    pub remaining: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub reset_at: Option<String>,
}

impl QuotaEntry {
    /// Used fraction in 0..=1 when both bounds are known.
    pub fn used_fraction(&self) -> Option<f32> {
        let limit = self.limit.filter(|limit| *limit > 0.0)?;
        let used = self
            .used
            .or_else(|| self.remaining.map(|remaining| limit - remaining))?;
        Some((used / limit).clamp(0.0, 1.0) as f32)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct BenchmarkStatus {
    #[serde(default)]
    pub snapshot: Option<BenchmarkSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct BenchmarkSnapshot {
    pub status: String,
    #[serde(default)]
    pub total_models: usize,
    #[serde(default)]
    pub results: Vec<BenchmarkResult>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct BenchmarkResult {
    pub provider_id: String,
    pub upstream_model: String,
    pub status: String,
    #[serde(default)]
    pub ttft_ms: Option<f64>,
    #[serde(default)]
    pub tps: Option<f64>,
}
