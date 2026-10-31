use super::*;
use crate::platform::proxy::ProxySelection;
use std::sync::Arc;

use crate::catalog::CatalogService;
use crate::gateway::GatewayExecutor;
use crate::upstream::UpstreamAccess;

pub use crate::upstream::AnthropicByteStream;

/// Composition of the components a server handler needs. It owns no upstream
/// send logic, no auth runtime, and no routing; those live in `upstream` and
/// `gateway`.
#[derive(Clone)]
pub struct AppState {
    pub(super) activity: Arc<super::activity::RequestActivity>,
    pub(crate) config: Arc<GatewayConfig>,
    pub(crate) providers: Arc<ProviderRegistry>,
    /// Shared upstream transport and auth runtimes (official + DUCX).
    pub(crate) upstream: Arc<UpstreamAccess>,
    /// Gateway execution: routing, request plans, single-request streaming.
    pub(crate) gateway: Arc<GatewayExecutor>,
    websocket_proxy_env: ProxySelection,
    pub(super) image_routes: ImageRouteRegistry,
    pub(super) benchmarks: ModelBenchmarkManager,
    /// Per-session provider prompt-prefix shapes, used to report where the
    /// upstream prefix cache was lost.
    pub(crate) cache_shapes: Arc<CacheShapeTracker>,
    web_search_capabilities: WebSearchCapabilities,
    /// Catalog generation, sources, and cached catalog responses.
    pub(crate) catalog: Arc<CatalogService>,
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
        crate::config::validate_cors_origins(&config.cors_allowed_origins)?;
        validate_fusion_profiles(&config.fusion_profiles)?;
        let websocket_proxy_env = ProxySelection::from_lookup(&env_lookup);
        // An archived session stays readable after the feature is switched off,
        // so the engine also opens when the database already exists.
        let context_database = crate::context::database_path_with(&env_lookup)?;
        let context_database =
            (config.context.enabled || context_database.exists()).then_some(context_database);
        let providers = Arc::new(ProviderRegistry::new_with_env(
            config.providers.clone(),
            env_lookup,
        )?);
        crate::config::validate_fusion_model_references(&config.fusion_profiles, &providers)?;
        let client = websocket_proxy_env
            .clone()
            .apply(Client::builder())
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(64)
            .build()?;
        let config = Arc::new(config);
        let upstream = Arc::new(UpstreamAccess::new(Arc::clone(&config), client));
        let image_routes = ImageRouteRegistry::default();
        let cache_shapes = Arc::new(CacheShapeTracker::with_usage(
            usage.with_providers(&config.providers),
        ));
        let context = match context_database {
            Some(path) => Some(Arc::new(crate::context::ContextEngine::open(
                config.context.clone(),
                &path,
            )?)),
            None => None,
        };
        let gateway = Arc::new(GatewayExecutor::new(
            Arc::clone(&config),
            Arc::clone(&providers),
            Arc::clone(&upstream),
            Arc::clone(&cache_shapes),
            web_search_capabilities.clone(),
            image_routes.clone(),
            context,
        ));
        let catalog = Arc::clone(&gateway.catalog);
        Ok(Self {
            activity: Arc::default(),
            config,
            providers,
            upstream,
            gateway,
            websocket_proxy_env,
            image_routes,
            benchmarks: ModelBenchmarkManager::from_default_path(),
            cache_shapes,
            web_search_capabilities,
            catalog,
        })
    }

    pub(super) fn websocket_proxy_env(&self) -> &ProxySelection {
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

    pub async fn fetch_models(&self) -> Result<Vec<crate::anthropic::ModelInfo>, GatewayError> {
        self.catalog.fetch_models().await
    }

    pub async fn fetch_official_models_catalog(&self, source_url: &str) -> anyhow::Result<Value> {
        let cache_path = crate::config::stored_config_path().with_file_name("official-models.json");
        let cached = match tokio::fs::read(&cache_path).await {
            Ok(bytes) => Some(serde_json::from_slice::<Value>(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        crate::catalog::official::fetch_catalog(
            source_url,
            crate::catalog::official::OFFICIAL_CATALOG_TIMEOUT,
            cached.as_ref(),
        )
        .await
    }
}
