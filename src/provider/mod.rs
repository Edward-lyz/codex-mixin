mod discovery;
mod external_auth;
mod models_dev;
mod presets;
mod registry;
mod types;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::header::HeaderMap;

pub use discovery::{apply_discovered_models, discover_provider_models, redact_provider_error};
pub use models_dev::{
    enrich_models_with_models_dev, fetch_models_dev_provider_models,
    parse_models_dev_provider_models, uses_models_dev_capabilities,
};
pub use presets::{
    AWS_BEDROCK_MANTLE_BASE_URL, AWS_BEDROCK_PRESET_ID, OPEN_CODE_GO_PRESET_ID, ProviderPreset,
    aws_bedrock_provider, baidu_oneapi_provider, custom_provider, deepseek_provider,
    open_code_go_provider, openrouter_provider,
};
pub use registry::{ProviderRegistry, ProviderRuntime, ResolvedProviderModel, catalog_model_slug};
pub use types::{
    BaiduAuthBridge, CONFIG_VERSION, MANUAL_MODEL_CONTEXT_WINDOW, ProviderAuthConfig,
    ProviderAuthHeader, ProviderDefinition, ProviderModel, ProviderModelKey, ProviderModelSource,
    ProviderProtocol, ProviderQuotaParser, ProviderReadiness, ProviderReadinessStatus,
    ProviderRequestPolicy, is_auto_review_model_id,
};

/// Mint the native auth headers for the selected Baidu auth core.
///
/// Mint the DUCX native auth headers without keeping a carrier process alive.
pub(crate) async fn native_baidu_headers(provider: &ProviderRuntime) -> anyhow::Result<HeaderMap> {
    let executable = provider
        .ducx_executable()
        .map(PathBuf::from)
        .or_else(crate::ducx::default_ducx_executable)
        .context("Baidu auth bridge is enabled but no managed executable is configured")?;
    if provider.uses_ducx_loopback() {
        crate::ducx::DucxRuntime::spawn(executable)
            .await?
            .native_headers(Duration::from_secs(30))
            .await
    } else {
        bail!("Baidu provider has no native auth bridge configured")
    }
}
