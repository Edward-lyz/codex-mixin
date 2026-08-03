mod discovery;
mod external_auth;
mod models_dev;
mod presets;
mod registry;
mod types;

pub use discovery::{apply_discovered_models, discover_provider_models, redact_provider_error};
pub use models_dev::{
    enrich_models_with_models_dev, fetch_models_dev_provider_models,
    parse_models_dev_provider_models, uses_models_dev_capabilities,
};
pub use presets::{
    OPEN_CODE_GO_PRESET_ID, ProviderPreset, baidu_oneapi_provider, custom_provider,
    deepseek_provider, open_code_go_provider, openrouter_provider,
};
pub use registry::{ProviderRegistry, ProviderRuntime, ResolvedProviderModel, catalog_model_slug};
pub use types::{
    BaiduAuthBridge, CONFIG_VERSION, ProviderAuthConfig, ProviderAuthHeader, ProviderDefinition,
    ProviderModel, ProviderModelKey, ProviderModelSource, ProviderProtocol, ProviderQuotaParser,
    ProviderReadiness, ProviderReadinessStatus, ProviderRequestPolicy,
};
