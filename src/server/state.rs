use super::*;

type AnthropicByteStream = BoxStream<'static, Result<Bytes, reqwest::Error>>;
const HOSTED_WEB_SEARCH_RETRY_ATTEMPTS: usize = 3;
const MODEL_CACHE_TTL: Duration = Duration::from_secs(30);
const CATALOG_SOURCE_CACHE_TTL: Duration = Duration::from_secs(60);
const CATALOG_RESPONSE_CACHE_TTL: Duration = Duration::from_secs(30);
const ANTHROPIC_FAST_BETA: &str = "fast-mode-2026-02-01";

enum AnthropicStreamDisposition {
    Ready(AnthropicByteStream),
    RetryHostedWebSearch,
}

struct CachedModels {
    fetched_at: Instant,
    models: Vec<ModelInfo>,
}

struct CatalogSources {
    template: Option<Value>,
    metadata: ModelMetadataResolver,
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
    pub(crate) client: Client,
    pub(super) image_routes: ImageRouteRegistry,
    pub(super) benchmarks: ModelBenchmarkManager,
    web_search_capabilities: WebSearchCapabilities,
    models_cache: Arc<tokio::sync::Mutex<Option<CachedModels>>>,
    catalog_sources_cache: Arc<tokio::sync::Mutex<Option<CachedCatalogSources>>>,
    catalog_response_cache: Arc<tokio::sync::Mutex<Option<CachedCatalogResponse>>>,
    official_auth_cache: Arc<tokio::sync::Mutex<Option<CachedOfficialAuth>>>,
}

impl AppState {
    pub fn new(config: GatewayConfig) -> anyhow::Result<Self> {
        let web_search_capabilities = WebSearchCapabilities::from_default_path(&config)?;
        Self::with_web_search_capabilities(config, web_search_capabilities)
    }

    pub fn with_web_search_capabilities(
        config: GatewayConfig,
        web_search_capabilities: WebSearchCapabilities,
    ) -> anyhow::Result<Self> {
        validate_fusion_profiles(&config.fusion_profiles)?;
        let client = Client::builder()
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(64)
            .build()?;
        Ok(Self {
            config: Arc::new(config),
            client,
            image_routes: ImageRouteRegistry::default(),
            benchmarks: ModelBenchmarkManager::from_default_path(),
            web_search_capabilities,
            models_cache: Arc::new(tokio::sync::Mutex::new(None)),
            catalog_sources_cache: Arc::new(tokio::sync::Mutex::new(None)),
            catalog_response_cache: Arc::new(tokio::sync::Mutex::new(None)),
            official_auth_cache: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    pub(crate) fn custom_image_routes(&self) -> Option<ImageRouteRegistry> {
        self.config
            .upstream_image_generation_path
            .is_some()
            .then(|| self.image_routes.clone())
    }

    pub async fn fetch_models(&self) -> Result<Vec<ModelInfo>, GatewayError> {
        let mut cache = self.models_cache.lock().await;
        if let Some(cached) = cache
            .as_ref()
            .filter(|cached| cached.fetched_at.elapsed() < MODEL_CACHE_TTL)
        {
            let mut models = cached.models.clone();
            drop(cache);
            self.web_search_capabilities.annotate_models(&mut models);
            self.append_fusion_models(&mut models);
            return Ok(models);
        }

        let mut models = self.fetch_models_uncached().await?;
        *cache = Some(CachedModels {
            fetched_at: Instant::now(),
            models: models.clone(),
        });
        drop(cache);
        self.web_search_capabilities.annotate_models(&mut models);
        self.append_fusion_models(&mut models);
        Ok(models)
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
        }));
    }

    async fn fetch_models_uncached(&self) -> Result<Vec<ModelInfo>, GatewayError> {
        if self.config.provider_preset == ProviderPreset::BaiduOneApi {
            return self.fetch_baidu_models().await;
        }

        let response = self
            .apply_upstream_auth(self.client.get(self.config.upstream_models_url()))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(GatewayError::Upstream(format!(
                "models endpoint returned {status}: {body}"
            )));
        }
        let parsed: ModelsResponse = serde_json::from_str(&body)?;
        Ok(parsed.data)
    }

    async fn fetch_baidu_models(&self) -> Result<Vec<ModelInfo>, GatewayError> {
        let response = self
            .apply_upstream_auth(self.client.post(format!(
                "{}/openapi/v2/available_models",
                self.config.upstream_base_url
            )))
            .json(&json!({}))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(GatewayError::Upstream(format!(
                "available models endpoint returned {status}: {body}"
            )));
        }
        let available: BaiduAvailableModelsResponse =
            serde_json::from_str(&body).map_err(|err| {
                GatewayError::Upstream(format!(
                    "available models endpoint returned invalid JSON: {err}"
                ))
            })?;
        if !available.success {
            return Err(GatewayError::Upstream(format!(
                "available models endpoint failed: {}",
                available.message
            )));
        }

        let mut available_by_model = Vec::with_capacity(available.data.len());
        let mut model_indices = HashMap::with_capacity(available.data.len());
        for model in available.data {
            let (id, is_internal) = match model.model.strip_suffix("-内部") {
                Some(canonical) => (canonical.to_owned(), true),
                None => (model.model.clone(), false),
            };
            if let Some(&index) = model_indices.get(&id) {
                if !is_internal {
                    available_by_model[index] = (id, model);
                }
            } else {
                model_indices.insert(id.clone(), available_by_model.len());
                available_by_model.push((id, model));
            }
        }

        Ok(available_by_model
            .into_iter()
            .filter_map(|(id, model)| {
                let Some(capability) = model.capability else {
                    tracing::warn!(
                        model = %model.model,
                        "excluding available models entry without capability metadata"
                    );
                    return None;
                };
                Some(ModelInfo {
                    id,
                    object: Some("model".to_owned()),
                    owned_by: Some("custom".to_owned()),
                    description: Some(capability.model_description),
                    ratio: Some(capability.ratio),
                    price_type: Some(model.price_type),
                    context_window: Some(capability.context_window),
                    supports_image: Some(capability.supports_image),
                    supports_thinking: Some(capability.supports_thinking),
                    ..ModelInfo::default()
                })
            })
            .collect())
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

    pub(super) async fn catalog_response(&self) -> Result<Bytes, GatewayError> {
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

    pub(crate) async fn official_auth(
        &self,
    ) -> anyhow::Result<(axum::http::HeaderValue, axum::http::HeaderValue)> {
        read_codex_official_auth(&self.config.codex_auth_path, &self.official_auth_cache).await
    }

    pub async fn fetch_official_models_catalog(
        &self,
        client_version: &str,
    ) -> anyhow::Result<Value> {
        let url = official_models_url(&self.config.official_responses_url, client_version)?;
        let (authorization, account_id) = self.official_auth().await?;
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            self.client
                .get(url)
                .header(header::AUTHORIZATION, authorization)
                .header("chatgpt-account-id", account_id)
                .header(header::ACCEPT, "application/json")
                .send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("official models endpoint timed out"))??;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("official models endpoint returned {status}");
        }
        let catalog: Value = response.json().await?;
        if catalog.get("models").and_then(Value::as_array).is_none() {
            anyhow::bail!("official models endpoint returned no models array");
        }
        Ok(catalog)
    }

    pub async fn probe_web_search_capabilities(
        &self,
        models: &mut [crate::anthropic::ModelInfo],
        force: bool,
    ) -> anyhow::Result<WebSearchProbeSummary> {
        self.web_search_capabilities
            .probe_models(models, &self.config, force)
            .await
    }

    pub(crate) fn web_search_enabled_for_custom_request(&self, body: &Value) -> bool {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.config.enable_web_search_tool && self.web_search_capabilities.supports_model(model)
    }

    pub(crate) fn apply_upstream_auth(
        &self,
        request: reqwest::RequestBuilder,
    ) -> reqwest::RequestBuilder {
        let request = match self.config.upstream_auth_header {
            UpstreamAuthHeader::AuthorizationBearer => {
                request.bearer_auth(&self.config.upstream_api_key)
            }
            UpstreamAuthHeader::XApiKey => {
                request.header("x-api-key", &self.config.upstream_api_key)
            }
        };
        request.header("anthropic-version", &self.config.anthropic_version)
    }

    pub(crate) fn apply_oneapi_affinity(
        &self,
        request: reqwest::RequestBuilder,
        hash_key: Option<&str>,
    ) -> reqwest::RequestBuilder {
        if let Some(hash_key) = hash_key {
            request.header("x-hash-key", hash_key)
        } else {
            request
        }
    }

    async fn send_anthropic_request(
        &self,
        request: &MessageRequest,
        hash_key: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let mut upstream_request =
            self.apply_upstream_auth(self.client.post(self.config.upstream_messages_url()));
        let beta = if request.speed.as_deref() == Some("fast") {
            Some(match self.config.anthropic_beta.as_deref() {
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
            self.config.anthropic_beta.clone()
        };
        if let Some(beta) = beta {
            upstream_request = upstream_request.header("anthropic-beta", beta);
        }
        let response = self
            .apply_oneapi_affinity(upstream_request, hash_key)
            .header(header::ACCEPT, "text/event-stream")
            .json(request)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(GatewayError::Upstream(format!(
                "messages endpoint returned {status}: {body}"
            )));
        }
        Ok(response.bytes_stream().boxed())
    }

    pub(crate) async fn anthropic_stream_with_web_search_retry(
        &self,
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
        let upstream = self.send_anthropic_request(&request, hash_key).await?;
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
                let original_session_id = request
                    .metadata
                    .as_ref()
                    .and_then(Value::as_object)
                    .and_then(|metadata| metadata.get("session_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                for attempt in 1..=HOSTED_WEB_SEARCH_RETRY_ATTEMPTS {
                    if let Some(metadata) = request.metadata.as_mut().and_then(Value::as_object_mut)
                    {
                        metadata.insert(
                            "session_id".to_owned(),
                            json!(format!(
                                "{}-web-search-retry-{}",
                                original_session_id.as_deref().unwrap_or("codex-mixin"),
                                Uuid::new_v4().simple()
                            )),
                        );
                    }
                    let retry = self.send_anthropic_request(&request, hash_key).await?;
                    match inspect_anthropic_stream(retry).await? {
                        AnthropicStreamDisposition::Ready(retry) => return Ok(retry),
                        AnthropicStreamDisposition::RetryHostedWebSearch => tracing::warn!(
                            model = %request.model,
                            attempt,
                            "forced hosted web_search was still returned as a client tool"
                        ),
                    }
                }
                Err(GatewayError::Upstream(format!(
                    "model {} returned a client-style web_search call after {} hosted-tool retries",
                    request.model, HOSTED_WEB_SEARCH_RETRY_ATTEMPTS
                )))
            }
        }
    }
}

fn official_models_url(
    official_responses_url: &str,
    client_version: &str,
) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(official_responses_url)?;
    let path = url.path().trim_end_matches('/');
    let prefix = path
        .strip_suffix("/responses")
        .ok_or_else(|| anyhow::anyhow!("official responses URL must end with /responses"))?;
    url.set_path(&format!("{prefix}/models"));
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("client_version", client_version);
    Ok(url)
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

pub(super) async fn read_codex_official_auth(
    auth_path: &std::path::Path,
    cache: &tokio::sync::Mutex<Option<CachedOfficialAuth>>,
) -> anyhow::Result<(axum::http::HeaderValue, axum::http::HeaderValue)> {
    let metadata = tokio::fs::metadata(auth_path).await.map_err(|err| {
        anyhow::anyhow!("read Codex auth metadata {}: {err}", auth_path.display())
    })?;
    let modified_at = metadata.modified().map_err(|err| {
        anyhow::anyhow!(
            "read Codex auth modification time {}: {err}",
            auth_path.display()
        )
    })?;
    let mut cache = cache.lock().await;
    if let Some(cached) = cache
        .as_ref()
        .filter(|cached| cached.modified_at == modified_at && cached.file_len == metadata.len())
    {
        return Ok((cached.authorization.clone(), cached.account_id.clone()));
    }

    let raw = tokio::fs::read_to_string(auth_path)
        .await
        .map_err(|err| anyhow::anyhow!("read Codex auth file {}: {err}", auth_path.display()))?;
    let auth: Value = serde_json::from_str(&raw)
        .map_err(|err| anyhow::anyhow!("parse Codex auth file {}: {err}", auth_path.display()))?;
    let tokens = auth
        .get("tokens")
        .ok_or_else(|| anyhow::anyhow!("Codex auth file does not contain tokens"))?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Codex auth file does not contain access_token"))?;
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .filter(|account_id| !account_id.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Codex auth file does not contain account_id"))?;
    let authorization: axum::http::HeaderValue = format!("Bearer {access_token}").parse()?;
    let account_id: axum::http::HeaderValue = account_id.parse()?;
    *cache = Some(CachedOfficialAuth {
        modified_at,
        file_len: metadata.len(),
        authorization: authorization.clone(),
        account_id: account_id.clone(),
    });
    Ok((authorization, account_id))
}
