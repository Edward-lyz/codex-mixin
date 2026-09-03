use super::websocket_proxy::ProxyEnv;
use super::*;
use std::collections::HashMap;

mod catalog;
mod ducx;
mod official_auth;

#[cfg(test)]
pub(super) use catalog::provider_model_display_name;
#[cfg(test)]
pub(super) use official_auth::read_codex_official_auth;

pub type AnthropicByteStream = BoxStream<'static, Result<Bytes, reqwest::Error>>;
const CATALOG_SOURCE_CACHE_TTL: Duration = Duration::from_secs(60);
const CATALOG_RESPONSE_CACHE_TTL: Duration = Duration::from_secs(30);
const ANTHROPIC_FAST_BETA: &str = "fast-mode-2026-02-01";

enum AnthropicStreamDisposition {
    Ready(AnthropicByteStream),
    RetryHostedWebSearch,
}

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

pub(super) struct CachedOfficialAuth {
    modified_at: SystemTime,
    file_len: u64,
    authorization: axum::http::HeaderValue,
    account_id: axum::http::HeaderValue,
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) config: Arc<GatewayConfig>,
    pub(crate) providers: Arc<ProviderRegistry>,
    pub(crate) client: Client,
    websocket_proxy_env: ProxyEnv,
    pub(super) image_routes: ImageRouteRegistry,
    pub(super) benchmarks: ModelBenchmarkManager,
    /// Per-session provider prompt-prefix shapes, used to report where the
    /// upstream prefix cache was lost.
    pub(crate) cache_shapes: Arc<CacheShapeTracker>,
    web_search_capabilities: WebSearchCapabilities,
    catalog_sources_cache: Arc<tokio::sync::Mutex<Option<CachedCatalogSources>>>,
    catalog_response_cache: Arc<tokio::sync::Mutex<Option<CachedCatalogResponse>>>,
    official_auth_cache: Arc<tokio::sync::Mutex<Option<CachedOfficialAuth>>>,
    ducx_runtimes:
        Arc<tokio::sync::Mutex<HashMap<String, Arc<crate::provider::auth::ducx::DucxRuntime>>>>,
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
        Ok(Self {
            config: Arc::new(config),
            providers,
            client,
            websocket_proxy_env,
            image_routes: ImageRouteRegistry::default(),
            benchmarks: ModelBenchmarkManager::from_default_path(),
            cache_shapes: Arc::new(CacheShapeTracker::with_usage(usage)),
            web_search_capabilities,
            catalog_sources_cache: Arc::new(tokio::sync::Mutex::new(None)),
            catalog_response_cache: Arc::new(tokio::sync::Mutex::new(None)),
            official_auth_cache: Arc::new(tokio::sync::Mutex::new(None)),
            ducx_runtimes: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
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

    pub async fn send_anthropic_request(
        &self,
        provider: &ProviderRuntime,
        request: &MessageRequest,
        hash_key: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let beta = if request.speed.as_deref() == Some("fast") {
            Some(match provider.definition().anthropic_beta.as_deref() {
                Some(configured)
                    if configured
                        .split(',')
                        .any(|item| item.trim() == ANTHROPIC_FAST_BETA) =>
                {
                    configured.to_owned()
                }
                Some(configured) if !configured.trim().is_empty() => {
                    format!("{configured},{ANTHROPIC_FAST_BETA}")
                }
                _ => ANTHROPIC_FAST_BETA.to_owned(),
            })
        } else {
            provider.definition().anthropic_beta.clone()
        };
        let mut refreshed_ducx_auth = false;
        loop {
            // DUCX acts as a header generator. Merge its native headers instead
            // of the stored key.
            let native = self.baidu_native_headers(provider).await?;
            let base_request = self.client.post(provider.api_url().clone());
            let mut upstream_request = match &native {
                Some(native) => base_request.headers(native.clone()),
                None if provider.aws_sigv4().is_some() => provider.apply_protocol_headers(
                    base_request,
                    crate::provider::ProviderProtocol::AnthropicMessages,
                ),
                None => provider.apply_auth(base_request),
            };
            upstream_request = provider.apply_anthropic_beta(upstream_request, beta.as_deref());
            let upstream_request = provider
                .apply_session_affinity(upstream_request, hash_key)
                .header(header::ACCEPT, "text/event-stream");
            let response = if let Some(aws) = provider.aws_sigv4() {
                let prepared = crate::request_body::prepare_signed_json(request.clone()).await?;
                let content_length = header::HeaderValue::from_str(&prepared.length.to_string())
                    .map_err(|error| GatewayError::Other(error.into()))?;
                let mut request = upstream_request
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::CONTENT_LENGTH, content_length)
                    .body(prepared.file)
                    .build()
                    .map_err(GatewayError::Http)?;
                crate::provider::sign_aws_request(
                    &mut request,
                    aws,
                    prepared.sha256,
                    std::time::SystemTime::now(),
                )?;
                self.client
                    .execute(request)
                    .await
                    .map_err(GatewayError::Http)
            } else {
                crate::request_body::send_json(upstream_request, request.clone()).await
            }
            .inspect_err(|error| {
                tracing::error!(
                    provider_id = provider.id(),
                    upstream_model_id = %request.model,
                    error = %crate::error::format_error_chain(error),
                    "provider messages request failed before receiving a response"
                );
            })?;
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED
                && provider.uses_ducx_loopback()
                && !refreshed_ducx_auth
            {
                tracing::warn!(
                    provider_id = provider.id(),
                    upstream_model_id = %request.model,
                    "refreshing DUCX authentication after upstream rejected cached headers"
                );
                self.invalidate_ducx_headers(provider).await?;
                refreshed_ducx_auth = true;
                continue;
            }
            if !status.is_success() {
                let body = crate::request_body::read_error_text(response).await?;
                return Err(GatewayError::UpstreamStatus {
                    status,
                    message: format!(
                        "provider {} messages endpoint returned {status}: {body}",
                        provider.id()
                    ),
                });
            }
            return Ok(response.bytes_stream().boxed());
        }
    }

    pub(crate) async fn anthropic_stream_with_web_search_retry(
        &self,
        provider: &ProviderRuntime,
        mut request: MessageRequest,
        hash_key: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let has_hosted_web_search = request.tools.iter().any(|tool| {
            tool.get("name").and_then(Value::as_str) == Some("web_search")
                && tool
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|tool_type| tool_type.starts_with("web_search_"))
        });
        let upstream = self
            .send_anthropic_request(provider, &request, hash_key)
            .await?;
        if !has_hosted_web_search {
            return Ok(upstream);
        }
        match inspect_anthropic_stream(upstream).await? {
            AnthropicStreamDisposition::Ready(upstream) => Ok(upstream),
            AnthropicStreamDisposition::RetryHostedWebSearch => {
                tracing::warn!(
                    model = %request.model,
                    "retrying client-style web_search call as an Anthropic server tool"
                );
                request.tool_choice = Some(json!({"type":"tool","name":"web_search"}));
                let retry_hash_key = hash_key.map(|_| Uuid::new_v4().to_string());
                if let Some(retry_hash_key) = retry_hash_key.as_ref()
                    && let Some(metadata) = request.metadata.as_mut().and_then(Value::as_object_mut)
                {
                    metadata.insert("session_id".to_owned(), json!(retry_hash_key));
                }
                let retry = self
                    .send_anthropic_request(
                        provider,
                        &request,
                        retry_hash_key.as_deref().or(hash_key),
                    )
                    .await?;
                match inspect_anthropic_stream(retry).await? {
                    AnthropicStreamDisposition::Ready(retry) => Ok(retry),
                    AnthropicStreamDisposition::RetryHostedWebSearch => {
                        Err(GatewayError::Upstream(format!(
                            "model {} returned a client-style web_search call after a forced hosted-tool retry",
                            request.model
                        )))
                    }
                }
            }
        }
    }
}

async fn inspect_anthropic_stream(
    mut upstream: AnthropicByteStream,
) -> Result<AnthropicStreamDisposition, GatewayError> {
    let mut buffered_chunks = Vec::new();
    let mut decoder = SseDecoder::default();
    while let Some(chunk) = upstream.next().await {
        let chunk = chunk?;
        let events = decoder.push(&chunk);
        buffered_chunks.push(chunk);
        let mut retry_hosted_web_search = None;
        for event in events {
            if event.data == "[DONE]" {
                retry_hosted_web_search = Some(false);
                break;
            }
            let Ok(payload) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            match payload.get("type").and_then(Value::as_str) {
                Some("content_block_start") => {
                    let block = payload.get("content_block").unwrap_or(&Value::Null);
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_use") => {
                            retry_hosted_web_search = Some(
                                block.get("name").and_then(Value::as_str) == Some("web_search"),
                            );
                        }
                        Some("server_tool_use") => retry_hosted_web_search = Some(false),
                        _ => {}
                    }
                }
                Some("content_block_delta") => {
                    let delta = payload.get("delta").unwrap_or(&Value::Null);
                    if delta.get("type").and_then(Value::as_str) == Some("text_delta")
                        && delta
                            .get("text")
                            .and_then(Value::as_str)
                            .is_some_and(|text| !text.is_empty())
                    {
                        retry_hosted_web_search = Some(false);
                    }
                }
                Some("message_stop" | "error") => retry_hosted_web_search = Some(false),
                _ => {}
            }
            if retry_hosted_web_search.is_some() {
                break;
            }
        }
        if let Some(retry_hosted_web_search) = retry_hosted_web_search {
            if retry_hosted_web_search {
                return Ok(AnthropicStreamDisposition::RetryHostedWebSearch);
            }
            let prefix = stream::iter(buffered_chunks.into_iter().map(Ok));
            return Ok(AnthropicStreamDisposition::Ready(
                prefix.chain(upstream).boxed(),
            ));
        }
    }
    Ok(AnthropicStreamDisposition::Ready(
        stream::iter(buffered_chunks.into_iter().map(Ok)).boxed(),
    ))
}
