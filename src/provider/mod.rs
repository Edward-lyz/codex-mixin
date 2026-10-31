pub use auth::official::{
    account_id as official_account_id, available as official_auth_available,
    fetch_resource as fetch_official_resource,
};
pub(crate) mod agy;
pub(crate) mod agy_business;
pub(crate) mod auth;
pub(crate) mod kiro;
pub use agy::{AgyQuotaWindow, fetch_agy_quota};
pub use auth::oauth::OAUTH_LOGIN_TIMEOUT_SECONDS;
pub use kiro::fetch_kiro_quota;
pub mod capabilities;
mod discovery;
pub(crate) mod local_oauth;
mod model_alias;
mod model_order;
pub(crate) use model_alias::flat_model_id;
pub use model_order::{ModelOrder, compare_model_ids};
mod quota;
mod registry;
mod resolver;
mod spec;
mod types;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::header::HeaderMap;

pub use discovery::{
    AUTO_SELECT_NEW_MODEL_LIMIT, ModelDiscoveryChanges, apply_discovered_models,
    discover_provider_models, redact_provider_error,
};
pub use quota::{QuotaUsageSummary, quota_usage};
pub use registry::{
    ProviderRegistry, ProviderRuntime, ResolvedProviderModel, auxiliary_auto_review_slug,
    catalog_model_slug,
};
pub use resolver::{
    MODELS_DEV_API_URL, MetadataResolver, ModelMetadata, default_metadata_cache_path,
};
pub use spec::{
    AWS_BEDROCK_DEFAULT_REGION, AWS_BEDROCK_MANTLE_SERVICE, AWS_BEDROCK_RUNTIME_SERVICE,
    OPEN_CODE_GO_PRESET_ID, ProviderPreset, ProviderSpec, aws_bedrock_aksk_provider,
    aws_bedrock_mantle_base_url, aws_bedrock_runtime_base_url, spec_for,
};
pub use types::{
    AwsSigV4AuthConfig, BaiduAuthBridge, CONFIG_VERSION, MANUAL_MODEL_CONTEXT_WINDOW,
    ProviderAuthConfig, ProviderAuthHeader, ProviderDefinition, ProviderModel, ProviderModelSource,
    ProviderProtocol, ProviderQuotaParser, ProviderReadiness, ProviderReadinessStatus,
    ProviderRequestPolicy,
};

pub(crate) use auth::aws_sigv4::sign_request as sign_aws_request;
pub(crate) use types::AUTO_REVIEW_MODEL_ID;

pub(crate) const OPENCODE_SESSION_HEADER: &str = "x-opencode-session";

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
