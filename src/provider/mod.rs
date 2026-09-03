pub(crate) mod auth;
pub mod capabilities;
mod discovery;
mod quota;
mod registry;
mod resolver;
mod spec;
mod types;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::header::HeaderMap;

pub use discovery::{apply_discovered_models, discover_provider_models, redact_provider_error};
pub use quota::{QuotaUsageSummary, quota_usage};
pub use registry::{ProviderRegistry, ProviderRuntime, ResolvedProviderModel, catalog_model_slug};
pub use resolver::{
    MODELS_DEV_API_URL, MetadataResolver, ModelMetadata, default_metadata_cache_path,
};
pub use spec::{
    AWS_BEDROCK_DEFAULT_REGION, AWS_BEDROCK_MANTLE_BASE_URL, AWS_BEDROCK_MANTLE_SERVICE,
    AWS_BEDROCK_PRESET_ID, OPEN_CODE_GO_PRESET_ID, ProviderPreset, ProviderSpec,
    aws_bedrock_aksk_provider, aws_bedrock_mantle_base_url, aws_bedrock_provider,
    baidu_oneapi_provider, custom_provider, deepseek_provider, open_code_go_provider,
    openrouter_provider, spec_for,
};
pub use types::{
    AwsSigV4AuthConfig, BaiduAuthBridge, CONFIG_VERSION, MANUAL_MODEL_CONTEXT_WINDOW,
    ProviderAuthConfig, ProviderAuthHeader, ProviderDefinition, ProviderModel, ProviderModelKey,
    ProviderModelSource, ProviderProtocol, ProviderQuotaParser, ProviderReadiness,
    ProviderReadinessStatus, ProviderRequestPolicy, is_auto_review_model_id,
};

pub(crate) use auth::aws_sigv4::sign_request as sign_aws_request;
pub(crate) use types::AUTO_REVIEW_MODEL_ID;

/// Mint the native auth headers for the selected Baidu auth core.
///
/// Mint the DUCX native auth headers without keeping a carrier process alive.
pub(crate) async fn native_baidu_headers(provider: &ProviderRuntime) -> anyhow::Result<HeaderMap> {
    let executable = provider
        .ducx_executable()
        .map(PathBuf::from)
        .or_else(auth::ducx::default_ducx_executable)
        .context("Baidu auth bridge is enabled but no managed executable is configured")?;
    if provider.uses_ducx_loopback() {
        auth::ducx::DucxRuntime::spawn(executable)
            .await?
            .native_headers(Duration::from_secs(30))
            .await
    } else {
        bail!("Baidu provider has no native auth bridge configured")
    }
}
