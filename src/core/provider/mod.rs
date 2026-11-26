pub use auth::official::{
    account_id as official_account_id, available as official_auth_available,
    fetch_resource as fetch_official_resource,
};
pub(crate) mod agy;
pub(crate) mod agy_business;
pub(crate) mod auth;
pub(crate) mod claude_code;
pub(crate) mod kiro;
pub use agy::{AgyQuotaWindow, fetch_agy_quota};
pub use auth::oauth::OAUTH_LOGIN_TIMEOUT_SECONDS;
pub use claude_code::fetch_claude_code_quota;
pub use kiro::fetch_kiro_quota;
mod adapter;
pub mod baidu;
pub mod capabilities;
mod discovery;
mod endpoint;
pub(crate) mod local_oauth;
mod model_alias;
mod model_order;
mod pricing;
pub(crate) use model_alias::flat_model_id;
pub use model_order::{ModelOrder, compare_model_ids};
pub(crate) use pricing::{LivePrices, ModelPrices};
mod models_dev;
mod quota;
mod registry;
mod resolver;
mod spec;
mod types;

pub(crate) use adapter::{ProviderAdapter, adapter_for};
pub use adapter::{provider_reports_usage_telemetry, provider_uses_managed_auth_bridge};
pub use discovery::{
    AUTO_SELECT_NEW_MODEL_LIMIT, ModelDiscoveryChanges, apply_discovered_models,
    discover_provider_models, official_models_from_catalog, redact_provider_error,
};
pub use endpoint::{EndpointSpec, WirePolicy};
pub use models_dev::{MODELS_DEV_API_URL, default_metadata_cache_path, metadata_catalog_path};
pub use quota::{QuotaUsageSummary, quota_usage, quota_usage_pointers};
pub use registry::{
    ProviderRegistry, ProviderRuntime, ResolvedProviderModel, auxiliary_auto_review_slug,
    catalog_model_slug,
};
pub use resolver::{MetadataResolver, ModelMetadata};
pub use spec::{
    AWS_BEDROCK_DEFAULT_REGION, AWS_BEDROCK_MANTLE_SERVICE, AWS_BEDROCK_RUNTIME_SERVICE, AuthMode,
    CLAUDE_CODE_PRESET_ID, OPEN_CODE_GO_PRESET_ID, ProviderPreset, ProviderSpec, QuotaLogin,
    aws_bedrock_aksk_provider, aws_bedrock_mantle_base_url, aws_bedrock_runtime_base_url, spec_for,
};
pub use types::{
    AwsSigV4AuthConfig, BaiduAuthBridge, CONFIG_VERSION, MANUAL_MODEL_CONTEXT_WINDOW,
    MIN_MODEL_REFRESH_INTERVAL_SECONDS, ModelRefreshPolicy, ProviderAuthConfig, ProviderAuthHeader,
    ProviderDefinition, ProviderModel, ProviderModelSource, ProviderProtocol, ProviderQuotaParser,
    ProviderReadiness, ProviderReadinessStatus, ProviderRequestPolicy, is_managed_pi_provider,
};

pub(crate) use auth::aws_sigv4::sign_request as sign_aws_request;
pub(crate) use types::{AUTO_REVIEW_MODEL_ID, PI_BRIDGE_PLACEHOLDER_URL, PI_BRIDGE_PROVIDER_ID};

pub(crate) const OPENCODE_SESSION_HEADER: &str = "x-opencode-session";

/// Client-facing label; routing continues to use the separate catalog slug.
pub fn provider_model_display_name(model: &ProviderModel, provider: &ProviderDefinition) -> String {
    adapter::adapter_for(provider).format_model_display_name(provider, model)
}
