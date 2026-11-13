//! Catalog service: model enumeration, benchmark targets, and the cached
//! Codex catalog response. Owns the catalog caches that used to live on the
//! server composition; it never depends on the server or CLI layers.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::Value;

use crate::anthropic::ModelInfo;
use crate::benchmark::BenchmarkTarget;
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::provider::{
    MetadataResolver, ProviderRegistry, catalog_model_slug, provider_model_display_name,
};
use crate::web_search::WebSearchCapabilities;

use super::{
    apply_auto_review_override, codex_catalog_from_models_with_metadata, load_template_catalog,
};

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
pub(crate) struct CatalogService {
    config: Arc<GatewayConfig>,
    providers: Arc<ProviderRegistry>,
    web_search_capabilities: WebSearchCapabilities,
    sources_cache: Arc<tokio::sync::Mutex<Option<CachedCatalogSources>>>,
    response_cache: Arc<tokio::sync::Mutex<Option<CachedCatalogResponse>>>,
}

impl CatalogService {
    pub(crate) async fn context_window(&self, model: &str) -> Result<u64, GatewayError> {
        let resolved = self.providers.resolve_known(model);
        if let Some(window) = resolved
            .as_ref()
            .and_then(|resolved| resolved.model)
            .and_then(|model| model.context_window)
        {
            return Ok(window);
        }
        let sources = self.catalog_sources().await?;
        let upstream_model = resolved
            .as_ref()
            .map_or(model, |resolved| resolved.upstream_model_id);
        if resolved.is_none()
            && let Some(window) = sources.template.as_ref().and_then(|template| {
                super::template::template_model_context_window(Some(template), upstream_model)
            })
        {
            return Ok(window);
        }
        Ok(sources
            .metadata
            .resolve(upstream_model, self.config.default_context_window)
            .context_window)
    }

    pub(crate) fn new(
        config: Arc<GatewayConfig>,
        providers: Arc<ProviderRegistry>,
        web_search_capabilities: WebSearchCapabilities,
    ) -> Self {
        Self {
            config,
            providers,
            web_search_capabilities,
            sources_cache: Arc::new(tokio::sync::Mutex::new(None)),
            response_cache: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub async fn fetch_models(&self) -> Result<Vec<ModelInfo>, GatewayError> {
        let mut models = Vec::new();
        for provider in self.providers.providers() {
            for upstream_model_id in &provider.definition().selected_models {
                let slug = catalog_model_slug(upstream_model_id, provider.id());
                let Some(resolved) = self.providers.resolve(&slug) else {
                    continue;
                };
                let model = resolved.model.expect("routable model has cached metadata");
                models.push(ModelInfo {
                    // Public /v1/models IDs stay provider-qualified catalog slugs.
                    // Catalog generation strips the provider suffix via owned_by when it
                    // needs the bare upstream model id for markers/metadata.
                    id: slug,
                    display_name: Some(provider_model_display_name(model, provider.definition())),
                    object: Some("model".to_owned()),
                    created: None,
                    owned_by: Some(provider.id().to_owned()),
                    description: model.description.clone().or_else(|| {
                        Some(format!(
                            "{} model {}",
                            provider.display_name(),
                            upstream_model_id
                        ))
                    }),
                    ratio: model.ratio.clone(),
                    price_type: model.price_type.clone(),
                    context_window: model.context_window,
                    max_output_tokens: model.max_output_tokens,
                    protocol: model.protocol,
                    api_path: model.api_path.clone(),
                    supports_image: model.supports_image,
                    supports_thinking: model.supports_thinking,
                    supports_web_search: model.supports_web_search,
                    supports_tool_search: model.supports_tool_search,
                    supports_function_tools: model.supports_function_tools,
                    capability_probe_error: model.capability_probe_error.clone(),
                    capabilities_probed_at_ms: model.capabilities_probed_at_ms,
                    architecture: None,
                    supported_parameters: Vec::new(),
                    reasoning: None,
                });
            }
        }
        self.web_search_capabilities.annotate_models(&mut models);
        self.append_fusion_models(&mut models);
        Ok(models)
    }

    pub(crate) async fn benchmark_targets(
        &self,
        provider_filters: &[String],
        model_filters: &[String],
    ) -> Result<Vec<BenchmarkTarget>, GatewayError> {
        let official_requested =
            provider_filters.is_empty() || provider_filters.iter().any(|id| id == "official");
        let official_models = if official_requested && self.config.accept_codex_oauth {
            let config = Arc::clone(&self.config);
            tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<String>> {
                if !crate::provider::official_auth_available(&config.codex_auth_path)? {
                    return Ok(Vec::new());
                }
                let cache =
                    crate::config::stored_config_path().with_file_name("official-models.json");
                let catalog = match load_template_catalog(Some(&cache))? {
                    Some(catalog) => Some(catalog),
                    None => load_template_catalog(None)?,
                };
                if let Some(account) = catalog
                    .as_ref()
                    .and_then(|value| value.get("codex_mixin_account_id"))
                    .and_then(Value::as_str)
                    && crate::provider::official_account_id(&config.codex_auth_path)?.as_deref()
                        != Some(account)
                {
                    return Ok(Vec::new());
                }
                let available = catalog
                    .as_ref()
                    .and_then(|value| value.get("models"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|model| model.get("slug").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                match &config.official_selected_models {
                    Some(selected) => Ok(selected
                        .iter()
                        .filter(|model| available.contains(model))
                        .cloned()
                        .collect()),
                    None => Ok(available),
                }
            })
            .await
            .map_err(|error| anyhow::anyhow!("official catalog loader failed: {error}"))??
        } else {
            Vec::new()
        };
        for provider_id in provider_filters {
            if provider_id != "official" && self.providers.provider(provider_id).is_none() {
                return Err(GatewayError::BadRequest(format!(
                    "unknown benchmark provider: {provider_id}"
                )));
            }
            if provider_id == "official" && official_models.is_empty() {
                return Err(GatewayError::BadRequest(
                    "official benchmark requires an enabled OAuth account and selected models"
                        .to_owned(),
                ));
            }
        }
        for model in model_filters {
            if self.providers.resolve(model).is_none() && !official_models.contains(model) {
                return Err(GatewayError::BadRequest(format!(
                    "benchmark model is not routable: {model}"
                )));
            }
        }
        let mut targets = Vec::new();
        for provider in self.providers.providers() {
            if !provider_filters.is_empty()
                && !provider_filters
                    .iter()
                    .any(|provider_id| provider_id == provider.id())
            {
                continue;
            }
            for upstream_model_id in &provider.definition().selected_models {
                let catalog_slug = catalog_model_slug(upstream_model_id, provider.id());
                let Some(resolved) = self.providers.resolve(&catalog_slug) else {
                    continue;
                };
                if !model_filters.is_empty()
                    && !model_filters.iter().any(|model| model == &catalog_slug)
                {
                    continue;
                }
                targets.push(BenchmarkTarget {
                    catalog_slug,
                    provider_id: provider.id().to_owned(),
                    provider_name: provider.display_name().to_owned(),
                    upstream_model_id: resolved.upstream_model_id.to_owned(),
                    provider: Some(provider.clone()),
                });
            }
        }
        for model in official_models {
            if !model_filters.is_empty() && !model_filters.contains(&model) {
                continue;
            }
            targets.push(BenchmarkTarget {
                catalog_slug: model.clone(),
                provider_id: "official".to_owned(),
                provider_name: "OpenAI".to_owned(),
                upstream_model_id: model,
                provider: None,
            });
        }
        Ok(targets)
    }

    fn append_fusion_models(&self, models: &mut Vec<ModelInfo>) {
        models.extend(
            self.config
                .route_models()
                .into_iter()
                .map(|route| ModelInfo {
                    display_name: route.display_name,
                    object: Some("model".to_owned()),
                    owned_by: Some("codex-mixin".to_owned()),
                    description: route.description,
                    supports_image: route.supports_image,
                    supports_thinking: route.supports_thinking,
                    supports_web_search: route.supports_web_search,
                    id: route.id,
                    ..ModelInfo::default()
                }),
        );
    }

    async fn catalog_sources(&self) -> Result<Arc<CatalogSources>, GatewayError> {
        let mut cache = self.sources_cache.lock().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.loaded_at.elapsed() < CATALOG_SOURCE_CACHE_TTL)
        {
            return Ok(Arc::clone(&cached.sources));
        }
        let sources = tokio::task::spawn_blocking(|| -> anyhow::Result<CatalogSources> {
            Ok(CatalogSources {
                template: load_template_catalog(None)?,
                metadata: MetadataResolver::from_default_files()?,
            })
        })
        .await
        .map_err(|error| anyhow::anyhow!("catalog source loader failed: {error}"))??;
        let sources = Arc::new(sources);
        *cache = Some(CachedCatalogSources {
            loaded_at: Instant::now(),
            sources: Arc::clone(&sources),
        });
        Ok(sources)
    }

    pub(crate) async fn catalog_response(&self) -> Result<Bytes, GatewayError> {
        let mut cache = self.response_cache.lock().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.generated_at.elapsed() < CATALOG_RESPONSE_CACHE_TTL)
        {
            return Ok(cached.body.clone());
        }

        let models = self.fetch_models().await?;
        let sources = self.catalog_sources().await?;
        let default_context_window = self.config.default_context_window;
        let auto_review_slug = crate::provider::auxiliary_auto_review_slug(&self.config.providers);
        let body = tokio::task::spawn_blocking(move || {
            let mut catalog = codex_catalog_from_models_with_metadata(
                &models,
                default_context_window,
                sources.template.as_ref(),
                &sources.metadata,
            );
            apply_auto_review_override(&mut catalog, auto_review_slug.as_deref())?;
            Ok::<Bytes, anyhow::Error>(Bytes::from(serde_json::to_vec(&catalog)?))
        })
        .await
        .map_err(|error| anyhow::anyhow!("catalog response generator failed: {error}"))??;
        *cache = Some(CachedCatalogResponse {
            generated_at: Instant::now(),
            body: body.clone(),
        });
        Ok(body)
    }
}
