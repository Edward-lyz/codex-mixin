use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, ensure};
use reqwest::header::HeaderMap;
use reqwest::{RequestBuilder, Url};

use super::auth::external::resolve_custom_headers_from_env;
use super::types::{
    BaiduAuthBridge, ProviderAuthHeader, ProviderDefinition, ProviderModel, ProviderModelSource,
    ProviderProtocol, ProviderQuotaParser, is_auto_review_model_id,
};

const FUSION_MODEL_PREFIX: &str = "mixin/fusion/";
const OFFICIAL_MODEL_PREFIX: &str = "official:";

#[derive(Clone, Debug)]
struct ProviderRouteTarget {
    provider_index: usize,
    upstream_model_id: String,
    model_index: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct ProviderRuntime {
    definition: ProviderDefinition,
    api_url: Url,
    openai_responses_url: Option<Url>,
    models_url: Option<Url>,
    image_generation_url: Option<Url>,
    quota_url: Option<Url>,
    custom_headers: HeaderMap,
}

impl ProviderRuntime {
    pub(super) fn new(
        definition: ProviderDefinition,
        env_lookup: &dyn Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        definition.validate()?;
        let api_url = endpoint_url(&definition.base_url, &definition.api_path)
            .with_context(|| format!("provider {} API URL", definition.id))?;
        let openai_responses_url = match &definition.model_source {
            ProviderModelSource::BaiduOneApi => Some(
                endpoint_url(&definition.base_url, "/v1/responses")
                    .with_context(|| format!("provider {} Responses URL", definition.id))?,
            ),
            _ => None,
        };
        let models_url = match &definition.model_source {
            ProviderModelSource::OpenAiCompatible { path } => Some(
                endpoint_url(&definition.base_url, path)
                    .with_context(|| format!("provider {} models URL", definition.id))?,
            ),
            ProviderModelSource::BaiduOneApi => Some(
                endpoint_url(&definition.base_url, "/openapi/v2/available_models")
                    .with_context(|| format!("provider {} available-models URL", definition.id))?,
            ),
            ProviderModelSource::AwsBedrock => None,
            ProviderModelSource::Static => None,
        };
        let image_generation_url = definition
            .image_generation_path
            .as_deref()
            .map(|path| endpoint_url(&definition.base_url, path))
            .transpose()
            .with_context(|| format!("provider {} image generation URL", definition.id))?;
        let quota_url = definition
            .quota_url
            .as_deref()
            .map(Url::parse)
            .transpose()
            .with_context(|| format!("provider {} quota URL", definition.id))?;
        let custom_headers = if definition.enabled {
            resolve_custom_headers_from_env(
                &definition.request_policy.custom_headers_from_env,
                env_lookup,
            )
            .with_context(|| format!("provider {} custom headers", definition.id))?
        } else {
            HeaderMap::new()
        };
        Ok(Self {
            definition,
            api_url,
            openai_responses_url,
            models_url,
            image_generation_url,
            quota_url,
            custom_headers,
        })
    }

    pub fn definition(&self) -> &ProviderDefinition {
        &self.definition
    }

    pub fn id(&self) -> &str {
        &self.definition.id
    }

    pub fn display_name(&self) -> &str {
        &self.definition.display_name
    }

    pub fn protocol(&self) -> ProviderProtocol {
        self.definition.protocol
    }

    pub fn protocol_for_model(&self, model: &str) -> ProviderProtocol {
        if self.is_baidu_model_source() && model.trim().to_ascii_lowercase().starts_with("gpt-") {
            ProviderProtocol::OpenAiResponses
        } else {
            self.protocol()
        }
    }

    pub fn api_url(&self) -> &Url {
        &self.api_url
    }

    pub fn api_url_for_model(&self, model: &str) -> Cow<'_, Url> {
        if self.protocol_for_model(model) == ProviderProtocol::OpenAiResponses
            && let Some(url) = &self.openai_responses_url
        {
            Cow::Borrowed(url)
        } else {
            let url_str = self.api_url.as_str();
            if url_str.contains("{model}") || url_str.contains("%7Bmodel%7D") {
                let replaced = url_str
                    .replace("{model}", model)
                    .replace("%7Bmodel%7D", model);
                match Url::parse(&replaced) {
                    Ok(parsed) => Cow::Owned(parsed),
                    Err(_) => Cow::Borrowed(&self.api_url),
                }
            } else {
                Cow::Borrowed(&self.api_url)
            }
        }
    }

    pub fn models_url(&self) -> Option<&Url> {
        self.models_url.as_ref()
    }

    pub fn image_generation_url(&self) -> Option<&Url> {
        self.image_generation_url.as_ref()
    }

    pub fn quota_url(&self) -> Option<Url> {
        let mut url = self.quota_url.clone()?;
        if !url.query_pairs().any(|(key, _)| key == "username")
            && let Some(username) = &self.definition.quota_username
        {
            url.query_pairs_mut().append_pair("username", username);
        }
        Some(url)
    }

    pub fn quota_currency(&self) -> Option<&str> {
        self.definition.quota_currency.as_deref()
    }

    pub fn quota_parser(&self) -> ProviderQuotaParser {
        self.definition.quota_parser
    }

    pub fn apply_auth(&self, request: RequestBuilder) -> RequestBuilder {
        self.apply_auth_for_protocol(request, self.protocol())
    }

    pub fn apply_auth_for_protocol(
        &self,
        request: RequestBuilder,
        protocol: ProviderProtocol,
    ) -> RequestBuilder {
        let request = match self.definition.auth.header {
            ProviderAuthHeader::AuthorizationBearer => {
                request.bearer_auth(&self.definition.auth.api_key)
            }
            ProviderAuthHeader::XApiKey => {
                request.header("x-api-key", &self.definition.auth.api_key)
            }
        };
        self.apply_protocol_headers(request, protocol)
    }

    pub fn apply_protocol_headers(
        &self,
        request: RequestBuilder,
        protocol: ProviderProtocol,
    ) -> RequestBuilder {
        let request = request.headers(self.custom_headers.clone());
        if protocol == ProviderProtocol::AnthropicMessages {
            request.header(
                "anthropic-version",
                self.definition
                    .anthropic_version
                    .as_deref()
                    .unwrap_or("2023-06-01"),
            )
        } else if protocol == ProviderProtocol::GoogleGemini {
            if !self.definition.auth.api_key.trim().is_empty() {
                request.header("x-goog-api-key", &self.definition.auth.api_key)
            } else {
                request
            }
        } else {
            request
        }
    }

    pub fn aws_sigv4(&self) -> Option<&super::AwsSigV4AuthConfig> {
        self.definition.auth.aws_sigv4.as_ref()
    }

    pub fn apply_custom_headers(&self, headers: &mut HeaderMap) {
        headers.extend(self.custom_headers.clone());
    }

    pub fn apply_anthropic_beta(
        &self,
        request: RequestBuilder,
        beta: Option<&str>,
    ) -> RequestBuilder {
        match beta.filter(|value| !value.trim().is_empty()) {
            Some(value) => request.header("anthropic-beta", value),
            None => request,
        }
    }

    pub fn apply_session_affinity(
        &self,
        request: RequestBuilder,
        hash_key: Option<&str>,
    ) -> RequestBuilder {
        let header = self
            .definition
            .request_policy
            .session_affinity_header
            .as_deref();
        if header.is_some_and(|header| header.eq_ignore_ascii_case(super::OPENCODE_SESSION_HEADER))
        {
            // Go requires a session even for standalone calls without client
            // identity. Never combine those calls under a provider-wide ID.
            let session = hash_key
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            return request
                .header(super::OPENCODE_SESSION_HEADER, session)
                .header(
                    reqwest::header::USER_AGENT,
                    concat!("codex-mixin/", env!("CARGO_PKG_VERSION")),
                );
        }
        match (header, hash_key) {
            (Some(header), Some(hash_key)) => request.header(header, hash_key),
            _ => request,
        }
    }

    pub fn uses_session_affinity(&self) -> bool {
        self.definition
            .request_policy
            .session_affinity_header
            .is_some()
    }

    pub fn uses_mcp_bridge_names(&self, model: &str) -> bool {
        self.definition.request_policy.mcp_bridge_for_fable
            && model.to_ascii_lowercase().contains("fable")
    }

    pub fn uses_ducx_loopback(&self) -> bool {
        self.definition.request_policy.effective_baidu_auth_bridge()
            == BaiduAuthBridge::DucxLoopback
    }

    pub fn ducx_executable(&self) -> Option<&std::path::Path> {
        self.definition.request_policy.ducx_executable.as_deref()
    }

    pub fn baidu_code_report(&self) -> bool {
        self.definition.request_policy.baidu_code_report
    }

    pub fn model_supports_thinking(&self, model: &str) -> Option<bool> {
        self.definition
            .cached_models
            .iter()
            .find(|candidate| candidate.id.eq_ignore_ascii_case(model))
            .and_then(|candidate| candidate.supports_thinking)
    }

    pub fn model_max_output_tokens(&self, model: &str) -> Option<u64> {
        self.definition
            .cached_models
            .iter()
            .find(|candidate| candidate.id.eq_ignore_ascii_case(model))
            .and_then(|candidate| {
                candidate
                    .max_output_tokens
                    .filter(|output| *output > 0)
                    .map(|output| {
                        candidate
                            .context_window
                            .map_or(output, |context| output.min(context))
                    })
            })
    }

    pub fn is_baidu_model_source(&self) -> bool {
        self.definition.model_source == ProviderModelSource::BaiduOneApi
    }
}

#[derive(Clone, Debug)]
pub struct ProviderRegistry {
    providers: Vec<ProviderRuntime>,
    provider_indices: HashMap<String, usize>,
    routes: BTreeMap<String, ProviderRouteTarget>,
    known_routes: BTreeMap<String, ProviderRouteTarget>,
}

impl ProviderRegistry {
    pub fn new(providers: Vec<ProviderDefinition>) -> anyhow::Result<Self> {
        Self::new_with_env(providers, |name| std::env::var(name).ok())
    }

    pub fn new_with_env(
        providers: Vec<ProviderDefinition>,
        env_lookup: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        ensure!(
            providers
                .iter()
                .filter(|provider| provider.codex_thread_title_model.is_some())
                .count()
                <= 1,
            "only one provider can map Codex thread-title requests"
        );
        ensure!(
            providers
                .iter()
                .filter(|provider| provider.auto_review_model.is_some())
                .count()
                <= 1,
            "only one provider can override Codex review requests"
        );
        let auxiliary_providers = providers
            .iter()
            .filter(|provider| provider.auxiliary_model_upstream)
            .map(|provider| provider.id.as_str())
            .collect::<Vec<_>>();
        ensure!(
            auxiliary_providers.len() <= 1,
            "multiple auxiliary model upstreams configured: {}",
            auxiliary_providers.join(", ")
        );
        let mut provider_ids = HashSet::with_capacity(providers.len());
        let mut runtimes = Vec::with_capacity(providers.len());
        let mut provider_indices = HashMap::with_capacity(providers.len());
        let mut routes = BTreeMap::new();
        let mut known_routes = BTreeMap::new();
        for provider in providers {
            ensure!(
                provider_ids.insert(provider.id.clone()),
                "duplicate provider id: {}",
                provider.id
            );
            let provider_index = runtimes.len();
            provider_indices.insert(provider.id.clone(), provider_index);
            let runtime = ProviderRuntime::new(provider, &env_lookup)?;
            for (model_index, model) in runtime.definition.cached_models.iter().enumerate() {
                let slug = catalog_model_slug(&model.id, runtime.id());
                validate_catalog_slug(&slug)?;
                let target = ProviderRouteTarget {
                    provider_index,
                    upstream_model_id: model.id.clone(),
                    model_index: Some(model_index),
                };
                insert_route(&runtimes, &runtime, &mut known_routes, &slug, &target)?;
            }
            for upstream_model_id in &runtime.definition.selected_models {
                let slug = catalog_model_slug(upstream_model_id, runtime.id());
                validate_catalog_slug(&slug)?;
                let model_index = runtime
                    .definition
                    .cached_models
                    .iter()
                    .position(|model| model.id == *upstream_model_id);
                let target = ProviderRouteTarget {
                    provider_index,
                    upstream_model_id: upstream_model_id.clone(),
                    model_index,
                };
                if model_index.is_none() {
                    insert_route(&runtimes, &runtime, &mut known_routes, &slug, &target)?;
                }
                if runtime.definition.enabled && model_index.is_some() {
                    insert_route(&runtimes, &runtime, &mut routes, &slug, &target)?;
                }
            }
            runtimes.push(runtime);
        }
        Ok(Self {
            providers: runtimes,
            provider_indices,
            routes,
            known_routes,
        })
    }

    pub fn providers(&self) -> &[ProviderRuntime] {
        &self.providers
    }

    pub fn provider(&self, provider_id: &str) -> Option<&ProviderRuntime> {
        self.provider_indices
            .get(provider_id)
            .and_then(|index| self.providers.get(*index))
    }

    pub fn auxiliary_image_provider(&self) -> Option<&ProviderRuntime> {
        self.providers.iter().find(|provider| {
            provider.definition().auxiliary_model_upstream
                && provider.image_generation_url().is_some()
        })
    }

    pub fn has_enabled_auxiliary_model_upstream(&self) -> bool {
        self.providers.iter().any(|provider| {
            provider.definition().enabled && provider.definition().auxiliary_model_upstream
        })
    }

    pub fn resolve(&self, catalog_slug: &str) -> Option<ResolvedProviderModel<'_>> {
        self.resolve_from(&self.routes, catalog_slug)
    }

    pub fn resolve_known(&self, catalog_slug: &str) -> Option<ResolvedProviderModel<'_>> {
        self.resolve_from(&self.known_routes, catalog_slug)
    }

    pub fn resolve_auxiliary_model<'a>(
        &'a self,
        upstream_model_id: &str,
    ) -> Option<ResolvedProviderModel<'a>> {
        if is_auto_review_model_id(upstream_model_id) {
            let provider = self.providers.iter().find(|provider| {
                provider.definition.enabled && provider.definition.auto_review_model.is_some()
            })?;
            let configured = configured_auto_review_model(&provider.definition)?;
            let catalog_slug = catalog_model_slug(configured, provider.id());
            return self
                .resolve(&catalog_slug)
                .filter(|resolved| resolved.model.is_some());
        }
        let provider = self.providers.iter().find(|provider| {
            provider.definition.enabled && provider.definition.auxiliary_model_upstream
        })?;
        let model = provider
            .definition
            .cached_models
            .iter()
            .find(|candidate| candidate.id.eq_ignore_ascii_case(upstream_model_id))?;
        let catalog_slug = catalog_model_slug(&model.id, provider.id());
        self.resolve_known(&catalog_slug)
            .filter(|resolved| resolved.model.is_some())
    }

    pub fn resolve_available_model(
        &self,
        upstream_model_id: &str,
    ) -> Option<ResolvedProviderModel<'_>> {
        let mut fallback = None;
        for provider in &self.providers {
            if !provider.definition.enabled {
                continue;
            }
            let catalog_slug = catalog_model_slug(upstream_model_id, provider.id());
            let Some(resolved) = self
                .resolve_known(&catalog_slug)
                .filter(|resolved| resolved.model.is_some())
            else {
                continue;
            };
            if provider
                .definition
                .selected_models
                .iter()
                .any(|selected| selected == upstream_model_id)
            {
                return Some(resolved);
            }
            fallback.get_or_insert(resolved);
        }
        fallback
    }

    /// Resolve a bare upstream model name to the unique configured provider.
    ///
    /// Anthropic-native clients such as Claude Code use model IDs without the
    /// Codex `-provider` suffix. This lookup first accepts the catalog slug and
    /// then falls back to a case-insensitive match across selected models.
    pub fn resolve_native_model(&self, model: &str) -> Option<ResolvedProviderModel<'_>> {
        if let Some(resolved) = self.resolve(model) {
            return Some(resolved);
        }
        let mut matched: Option<(&ProviderRuntime, &ProviderModel)> = None;
        for provider in &self.providers {
            if !provider.definition.enabled {
                continue;
            }
            for candidate in &provider.definition.cached_models {
                if !provider
                    .definition
                    .selected_models
                    .iter()
                    .any(|selected| selected == &candidate.id)
                    || !candidate.id.eq_ignore_ascii_case(model)
                {
                    continue;
                }
                if matched.is_some() {
                    return None;
                }
                matched = Some((provider, candidate));
            }
        }
        let (provider, candidate) = matched?;
        let catalog_slug = catalog_model_slug(&candidate.id, provider.id());
        self.resolve(&catalog_slug)
    }

    pub fn routable_models(&self) -> impl Iterator<Item = ResolvedProviderModel<'_>> {
        self.routes
            .keys()
            .filter_map(|slug| self.resolve(slug.as_str()))
    }

    fn resolve_from<'a>(
        &'a self,
        routes: &'a BTreeMap<String, ProviderRouteTarget>,
        catalog_slug: &str,
    ) -> Option<ResolvedProviderModel<'a>> {
        let (catalog_slug, target) = match routes.get_key_value(catalog_slug) {
            Some(route) => route,
            None => {
                let decoded = super::model_alias::unflatten_model_id(catalog_slug)?;
                routes.get_key_value(&decoded)?
            }
        };
        let provider = self.providers.get(target.provider_index)?;
        let model = target
            .model_index
            .and_then(|index| provider.definition.cached_models.get(index));
        Some(ResolvedProviderModel {
            catalog_slug: catalog_slug.as_str(),
            provider,
            upstream_model_id: &target.upstream_model_id,
            model,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResolvedProviderModel<'a> {
    pub catalog_slug: &'a str,
    pub provider: &'a ProviderRuntime,
    pub upstream_model_id: &'a str,
    pub model: Option<&'a ProviderModel>,
}

fn validate_catalog_slug(slug: &str) -> anyhow::Result<()> {
    ensure!(
        !slug.starts_with(FUSION_MODEL_PREFIX)
            && !slug.starts_with(OFFICIAL_MODEL_PREFIX)
            && !slug.starts_with(super::model_alias::FLAT_MODEL_PREFIX),
        "provider model slug uses a reserved namespace: {slug}"
    );
    Ok(())
}

pub fn catalog_model_slug(upstream_model_id: &str, provider_id: &str) -> String {
    format!("{upstream_model_id}-{provider_id}")
}

/// Catalog slug Codex must use for guardian auto review.
///
/// Only an explicit review override changes Codex's default model and quota.
pub fn auxiliary_auto_review_slug(providers: &[ProviderDefinition]) -> Option<String> {
    let provider = providers
        .iter()
        .find(|provider| provider.enabled && provider.auto_review_model.is_some())?;
    let model = configured_auto_review_model(provider)?;
    Some(catalog_model_slug(model, &provider.id))
}

fn configured_auto_review_model(provider: &ProviderDefinition) -> Option<&str> {
    provider
        .auto_review_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
}

fn insert_route(
    existing_providers: &[ProviderRuntime],
    current_provider: &ProviderRuntime,
    routes: &mut BTreeMap<String, ProviderRouteTarget>,
    slug: &str,
    target: &ProviderRouteTarget,
) -> anyhow::Result<()> {
    if let Some(existing) = routes.insert(slug.to_owned(), target.clone()) {
        let existing_provider = existing_providers
            .get(existing.provider_index)
            .map(ProviderRuntime::id)
            .unwrap_or("<current>");
        anyhow::bail!(
            "provider model slug collision for {slug}: {}/{} and {}/{}",
            existing_provider,
            existing.upstream_model_id,
            current_provider.id(),
            target.upstream_model_id
        );
    }
    Ok(())
}

fn endpoint_url(base_url: &str, path: &str) -> anyhow::Result<Url> {
    let mut base_url = Url::parse(base_url)?;
    let base_path = base_url.path().trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let endpoint_path = if base_path.is_empty()
        || base_path == "/"
        || path == base_path
        || path.starts_with(&format!("{base_path}/"))
    {
        path
    } else if let Some(base_without_version) = base_path.strip_suffix("/v1")
        && (path == "/v1" || path.starts_with("/v1/"))
    {
        format!("{base_without_version}{path}")
    } else {
        format!("{base_path}{path}")
    };
    base_url.set_path(&endpoint_path);
    Ok(base_url)
}
