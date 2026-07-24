mod discovery;
mod presets;
mod registry;
mod types;

pub use discovery::{apply_discovered_models, discover_provider_models, redact_provider_error};
pub use presets::{
    ProviderPreset, baidu_oneapi_provider, custom_provider, deepseek_provider,
    open_code_go_provider, openrouter_provider,
};
pub use registry::{ProviderRegistry, ProviderRuntime, ResolvedProviderModel, catalog_model_slug};
pub use types::{
    CONFIG_VERSION, ProviderAuthConfig, ProviderAuthHeader, ProviderDefinition, ProviderModel,
    ProviderModelKey, ProviderModelSource, ProviderProtocol, ProviderQuotaParser,
    ProviderReadiness, ProviderReadinessStatus, ProviderRequestPolicy,
};
