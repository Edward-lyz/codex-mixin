use super::websocket_proxy::ProxyEnv;
use super::*;
use std::sync::Arc;

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

#[derive(Clone)]
pub struct AppState {
    pub(crate) config: Arc<GatewayConfig>,
    pub(crate) providers: Arc<ProviderRegistry>,
    /// Shared upstream transport and auth runtimes (official + DUCX).
    pub(crate) upstream: Arc<UpstreamAccess>,
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
        Ok(Self {
            config,
            providers,
            upstream,
            websocket_proxy_env,
            image_routes: ImageRouteRegistry::default(),
            benchmarks: ModelBenchmarkManager::from_default_path(),
            cache_shapes: Arc::new(CacheShapeTracker::with_usage(usage)),
            web_search_capabilities,
            catalog_sources_cache: Arc::new(tokio::sync::Mutex::new(None)),
            catalog_response_cache: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    pub(super) fn websocket_proxy_env(&self) -> &ProxyEnv {
        &self.websocket_proxy_env
    }

    pub(crate) fn custom_image_routes(
        &self,
        provider: &ProviderRuntime,
    ) -> Option<ImageRouteRegistry> {
        provider
            .image_generation_url()
            .is_some()
            .then(|| self.image_routes.for_provider(provider.id()))
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

    pub(crate) fn web_search_enabled_for_custom_request(&self, body: &Value) -> bool {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.config.enable_web_search_tool && self.web_search_capabilities.supports_model(model)
    }

    pub(crate) async fn resolve_model_route(
        &self,
        model: &str,
    ) -> Result<ResolvedModelRoute, GatewayError> {
        let custom_result =
            ModelRouter::new(&self.config.fusion_profiles, &self.providers, false).resolve(model);
        if custom_result.is_ok() || !crate::gateway::is_official_model_slug(model) {
            return custom_result;
        }
        if !model.eq_ignore_ascii_case(crate::gateway::AUTO_REVIEW_MODEL_SLUG)
            && self
                .config
                .official_selected_models
                .as_ref()
                .is_some_and(|selected| {
                    !selected
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(model))
                })
        {
            return Err(GatewayError::BadRequest(format!(
                "official model is not selected: {model}"
            )));
        }
        if self.config.accept_codex_oauth && self.official_auth().await.is_ok() {
            return Ok(ResolvedModelRoute::Official);
        }
        if model.eq_ignore_ascii_case(crate::gateway::AUTO_REVIEW_MODEL_SLUG)
            && let Some(resolved) = self
                .providers
                .resolve_available_model(crate::gateway::AUTO_REVIEW_MODEL_SLUG)
        {
            return Ok(ResolvedModelRoute::Provider {
                catalog_slug: resolved.catalog_slug.to_owned(),
                provider_id: resolved.provider.id().to_owned(),
                upstream_model_id: resolved.upstream_model_id.to_owned(),
            });
        }
        custom_result
    }

    pub(crate) fn resolved_provider_model(
        &self,
        catalog_slug: &str,
    ) -> Result<ResolvedProviderModel<'_>, GatewayError> {
        self.providers
            .resolve(catalog_slug)
            .or_else(|| {
                self.providers
                    .resolve_known(catalog_slug)
                    .filter(|resolved| {
                        resolved.provider.definition().enabled
                            && resolved.provider.definition().auxiliary_model_upstream
                            && resolved.model.is_some()
                    })
            })
            .ok_or_else(|| {
                GatewayError::BadRequest(format!("model is not routable: {catalog_slug}"))
            })
    }

    pub(crate) fn resolve_native_provider_model(
        &self,
        model: &str,
    ) -> Result<ResolvedProviderModel<'_>, GatewayError> {
        self.providers
            .resolve_native_model(model)
            .ok_or_else(|| GatewayError::BadRequest(format!("model is not routable: {model}")))
    }

    // ---- Upstream delegates (transport lives in crate::upstream) ----

    pub async fn send_anthropic_request(
        &self,
        provider: &ProviderRuntime,
        request: &MessageRequest,
        hash_key: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        self.upstream
            .send_anthropic_request(provider, request, hash_key)
            .await
    }

    pub(crate) async fn anthropic_stream_with_web_search_retry(
        &self,
        provider: &ProviderRuntime,
        request: MessageRequest,
        hash_key: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        self.upstream
            .anthropic_stream_with_web_search_retry(provider, request, hash_key)
            .await
    }

    pub(crate) async fn baidu_native_headers(
        &self,
        provider: &ProviderRuntime,
    ) -> Result<Option<axum::http::HeaderMap>, GatewayError> {
        self.upstream.baidu_native_headers(provider).await
    }

    pub(crate) async fn prewarm_ducx(&self) -> Result<(), GatewayError> {
        self.upstream.prewarm_ducx(&self.providers).await
    }

    pub(crate) async fn official_auth(
        &self,
    ) -> anyhow::Result<(axum::http::HeaderValue, axum::http::HeaderValue)> {
        self.upstream.official_auth().await
    }

    pub async fn fetch_official_models_catalog(
        &self,
        client_version: &str,
    ) -> anyhow::Result<Value> {
        self.upstream
            .fetch_official_models_catalog(client_version)
            .await
    }
}
