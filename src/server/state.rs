use super::websocket_proxy::ProxyEnv;
use super::*;
use std::sync::Arc;

use crate::gateway::GatewayExecutor;
use crate::upstream::UpstreamAccess;

mod catalog;

#[cfg(test)]
pub(super) use catalog::provider_model_display_name;

pub use crate::upstream::AnthropicByteStream;

const CATALOG_SOURCE_CACHE_TTL: Duration = Duration::from_secs(60);
const CATALOG_RESPONSE_CACHE_TTL: Duration = Duration::from_secs(30);

struct CatalogSources {
    template: Option<Value>,
    metadata: MetadataResolver,
}

struct CachedCatalogSources {
    loaded_at: Instant,
    sources: Arc<CatalogSources>,
}

struct CachedCatalogResponse {
    generated_at: Instant,
    body: Bytes,
}

/// Composition of the components a server handler needs. It owns no upstream
/// send logic, no auth runtime, and no routing; those live in `upstream` and
/// `gateway`.
#[derive(Clone)]
pub struct AppState {
    pub(crate) config: Arc<GatewayConfig>,
    pub(crate) providers: Arc<ProviderRegistry>,
    /// Shared upstream transport and auth runtimes (official + DUCX).
    pub(crate) upstream: Arc<UpstreamAccess>,
    /// Gateway execution: routing, request plans, single-request streaming.
    pub(crate) gateway: Arc<GatewayExecutor>,
    websocket_proxy_env: ProxyEnv,
    pub(super) image_routes: ImageRouteRegistry,
    pub(super) benchmarks: ModelBenchmarkManager,
    /// Per-session provider prompt-prefix shapes, used to report where the
    /// upstream prefix cache was lost.
    pub(crate) cache_shapes: Arc<CacheShapeTracker>,
    web_search_capabilities: WebSearchCapabilities,
    catalog_sources_cache: Arc<tokio::sync::Mutex<Option<CachedCatalogSources>>>,
    catalog_response_cache: Arc<tokio::sync::Mutex<Option<CachedCatalogResponse>>>,
}

impl AppState {
    pub fn new(mut config: GatewayConfig) -> anyhow::Result<Self> {
        ProviderCapabilities::from_default_path(&config)?.annotate_config(&mut config);
        let web_search_capabilities = WebSearchCapabilities::from_default_path(&config)?;
        Self::with_web_search_capabilities(config, web_search_capabilities)
    }

    pub fn with_env_lookup(
        mut config: GatewayConfig,
        env_lookup: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        ProviderCapabilities::from_default_path(&config)?.annotate_config(&mut config);
        let web_search_capabilities = WebSearchCapabilities::from_default_path(&config)?;
        Self::with_web_search_capabilities_and_env(config, web_search_capabilities, env_lookup)
    }

    pub fn with_web_search_capabilities(
        config: GatewayConfig,
        web_search_capabilities: WebSearchCapabilities,
    ) -> anyhow::Result<Self> {
        Self::with_web_search_capabilities_and_env(config, web_search_capabilities, |name| {
            std::env::var(name).ok()
        })
    }

    #[cfg(test)]
    pub(crate) fn with_usage_aggregator(
        config: GatewayConfig,
        usage: crate::gateway::TokenUsageAggregator,
    ) -> anyhow::Result<Self> {
        let mut config = config;
        ProviderCapabilities::from_default_path(&config)?.annotate_config(&mut config);
        let web_search_capabilities = WebSearchCapabilities::from_default_path(&config)?;
        Self::with_web_search_capabilities_and_env_and_usage(
            config,
            web_search_capabilities,
            |name| std::env::var(name).ok(),
            usage,
        )
    }

    fn with_web_search_capabilities_and_env(
        config: GatewayConfig,
        web_search_capabilities: WebSearchCapabilities,
        env_lookup: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        Self::with_web_search_capabilities_and_env_and_usage(
            config,
            web_search_capabilities,
            env_lookup,
            crate::gateway::TokenUsageAggregator::try_from_default_path()?,
        )
    }

    fn with_web_search_capabilities_and_env_and_usage(
        config: GatewayConfig,
        web_search_capabilities: WebSearchCapabilities,
        env_lookup: impl Fn(&str) -> Option<String>,
        usage: crate::gateway::TokenUsageAggregator,
    ) -> anyhow::Result<Self> {
        validate_fusion_profiles(&config.fusion_profiles)?;
        let websocket_proxy_env = ProxyEnv::from_lookup(&env_lookup);
        let providers = Arc::new(ProviderRegistry::new_with_env(
            config.providers.clone(),
            env_lookup,
        )?);
        crate::fusion::validate_fusion_model_references(&config.fusion_profiles, &providers)?;
        let client = Client::builder()
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(64)
            .build()?;
        let config = Arc::new(config);
        let upstream = Arc::new(UpstreamAccess::new(Arc::clone(&config), client));
        let image_routes = ImageRouteRegistry::default();
        let cache_shapes = Arc::new(CacheShapeTracker::with_usage(usage));
        let gateway = Arc::new(GatewayExecutor::new(
            Arc::clone(&config),
            Arc::clone(&providers),
            Arc::clone(&upstream),
            Arc::clone(&cache_shapes),
            web_search_capabilities.clone(),
            image_routes.clone(),
        ));
        Ok(Self {
            config,
            providers,
            upstream,
            gateway,
            websocket_proxy_env,
            image_routes,
            benchmarks: ModelBenchmarkManager::from_default_path(),
            cache_shapes,
            web_search_capabilities,
            catalog_sources_cache: Arc::new(tokio::sync::Mutex::new(None)),
            catalog_response_cache: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    pub(super) fn websocket_proxy_env(&self) -> &ProxyEnv {
        &self.websocket_proxy_env
    }

    pub fn provider(&self, provider_id: &str) -> Option<&ProviderRuntime> {
        self.providers.provider(provider_id)
    }

    pub async fn probe_web_search_capabilities(
        &self,
        models: &mut [crate::anthropic::ModelInfo],
        force: bool,
    ) -> anyhow::Result<WebSearchProbeSummary> {
        self.web_search_capabilities
            .probe_models(models, &self.config, &self.providers, force)
            .await
    }

    // ---- CLI-facing catalog and auth delegates (migrate with application) ----

    pub async fn fetch_official_models_catalog(
        &self,
        client_version: &str,
    ) -> anyhow::Result<Value> {
        self.upstream
            .fetch_official_models_catalog(client_version)
            .await
    }
}
