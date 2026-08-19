use super::*;

impl AppState {
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
                    display_name: Some(provider_model_display_name(
                        upstream_model_id,
                        provider.display_name(),
                    )),
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

    pub(crate) fn benchmark_targets(
        &self,
        provider_filters: &[String],
        model_filters: &[String],
    ) -> Result<Vec<BenchmarkTarget>, GatewayError> {
        for provider_id in provider_filters {
            if self.providers.provider(provider_id).is_none() {
                return Err(GatewayError::BadRequest(format!(
                    "unknown benchmark provider: {provider_id}"
                )));
            }
        }
        for model in model_filters {
            if self.providers.resolve(model).is_none() {
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
                    provider: provider.clone(),
                });
            }
        }
        Ok(targets)
    }

    fn append_fusion_models(&self, models: &mut Vec<ModelInfo>) {
        models.extend(self.config.fusion_profiles.iter().map(|profile| ModelInfo {
            id: profile.model_slug(),
            display_name: Some(format!(
                "Fusion ({}): {} → judge {}",
                profile.id,
                profile.panel_models.join("+"),
                profile.judge_model
            )),
            object: Some("model".to_owned()),
            created: None,
            owned_by: Some("codex-mixin".to_owned()),
            description: Some(format!(
                "Fusion pipeline: {} panel models in parallel, judged by {}, finalized by {}",
                profile.panel_models.len(),
                profile.judge_model,
                profile.final_model
            )),
            ratio: None,
            price_type: None,
            context_window: None,
            supports_image: Some(false),
            supports_thinking: Some(true),
            supports_web_search: Some(false),
            ..ModelInfo::default()
        }));
    }

    async fn catalog_sources(&self) -> Result<Arc<CatalogSources>, GatewayError> {
        let mut cache = self.catalog_sources_cache.lock().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.loaded_at.elapsed() < CATALOG_SOURCE_CACHE_TTL)
        {
            return Ok(Arc::clone(&cached.sources));
        }
        let sources = tokio::task::spawn_blocking(|| -> anyhow::Result<CatalogSources> {
            Ok(CatalogSources {
                template: load_template_catalog(None)?,
                metadata: ModelMetadataResolver::from_default_files()?,
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
        let mut cache = self.catalog_response_cache.lock().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.generated_at.elapsed() < CATALOG_RESPONSE_CACHE_TTL)
        {
            return Ok(cached.body.clone());
        }

        let models = self.fetch_models().await?;
        let sources = self.catalog_sources().await?;
        let default_context_window = self.config.default_context_window;
        let body = tokio::task::spawn_blocking(move || {
            let catalog = codex_catalog_from_models_with_metadata(
                &models,
                default_context_window,
                sources.template.as_ref(),
                &sources.metadata,
            );
            serde_json::to_vec(&catalog).map(Bytes::from)
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

pub(crate) fn provider_model_display_name(
    upstream_model_id: &str,
    provider_display_name: &str,
) -> String {
    format!("{upstream_model_id} · {provider_display_name}")
}
