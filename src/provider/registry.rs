use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, ensure};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{RequestBuilder, Url};

use super::ducc::{DUCC_HEADER_NAME, DuccReportGuard, DuccReporter, resolve_ducc_integration};
use super::types::{
    ProviderAuthHeader, ProviderDefinition, ProviderModel, ProviderModelKey, ProviderModelSource,
    ProviderProtocol, ProviderQuotaParser,
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
    ducc_auth_header: Option<HeaderValue>,
    ducc_reporter: Option<DuccReporter>,
}

impl ProviderRuntime {
    pub(super) fn new(definition: ProviderDefinition) -> anyhow::Result<Self> {
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
        let ducc_integration = if definition.enabled
            && definition.model_source == ProviderModelSource::BaiduOneApi
            && definition.request_policy.ducc_auth == Some(true)
        {
            resolve_ducc_integration(&definition.request_policy)
                .map(Some)
                .with_context(|| format!("provider {} DUCC authentication", definition.id))?
        } else {
            None
        };
        let ducc_auth_header = ducc_integration
            .as_ref()
            .map(|integration| integration.header.clone());
        let ducc_reporter = ducc_integration.map(|integration| integration.reporter);
        Ok(Self {
            definition,
            api_url,
            openai_responses_url,
            models_url,
            image_generation_url,
            quota_url,
            ducc_auth_header,
            ducc_reporter,
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

    pub fn api_url_for_model(&self, model: &str) -> &Url {
        if self.protocol_for_model(model) == ProviderProtocol::OpenAiResponses
            && let Some(url) = &self.openai_responses_url
        {
            url
        } else {
            &self.api_url
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
        let request = match &self.ducc_auth_header {
            Some(value) => request.header(DUCC_HEADER_NAME, value),
            None => request,
        };
        if protocol == ProviderProtocol::AnthropicMessages {
            request.header(
                "anthropic-version",
                self.definition
                    .anthropic_version
                    .as_deref()
                    .unwrap_or("2023-06-01"),
            )
        } else {
            request
        }
    }

    pub fn apply_ducc_auth_header(&self, headers: &mut HeaderMap) {
        if let Some(value) = &self.ducc_auth_header {
            headers.insert(DUCC_HEADER_NAME, value.clone());
        }
    }

    pub(crate) fn begin_ducc_report(
        &self,
        body: &serde_json::Value,
        session_hint: Option<&str>,
    ) -> Option<DuccReportGuard> {
        self.ducc_reporter
            .as_ref()
            .map(|reporter| reporter.begin_request(body, session_hint))
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
        match (
            self.definition
                .request_policy
                .session_affinity_header
                .as_deref(),
            hash_key,
        ) {
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

    pub fn model_supports_thinking(&self, model: &str) -> Option<bool> {
        self.definition
            .cached_models
            .iter()
            .find(|candidate| candidate.id.eq_ignore_ascii_case(model))
            .and_then(|candidate| candidate.supports_thinking)
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
            let runtime = ProviderRuntime::new(provider)?;
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

    pub fn catalog_slugs(&self) -> impl Iterator<Item = &str> {
        self.routes.keys().map(String::as_str)
    }

    pub fn resolve(&self, catalog_slug: &str) -> Option<ResolvedProviderModel<'_>> {
        self.resolve_from(&self.routes, catalog_slug)
    }

    pub fn resolve_known(&self, catalog_slug: &str) -> Option<ResolvedProviderModel<'_>> {
        self.resolve_from(&self.known_routes, catalog_slug)
    }

    pub fn resolve_auxiliary_model(
        &self,
        upstream_model_id: &str,
    ) -> Option<ResolvedProviderModel<'_>> {
        let provider = self.providers.iter().find(|provider| {
            provider.definition.enabled && provider.definition.auxiliary_model_upstream
        })?;
        let catalog_slug = catalog_model_slug(upstream_model_id, provider.id());
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
        let (catalog_slug, target) = routes.get_key_value(catalog_slug)?;
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

impl ResolvedProviderModel<'_> {
    pub fn key(&self) -> ProviderModelKey {
        ProviderModelKey {
            provider_id: self.provider.id().to_owned(),
            upstream_model_id: self.upstream_model_id.to_owned(),
        }
    }
}

fn validate_catalog_slug(slug: &str) -> anyhow::Result<()> {
    ensure!(
        !slug.starts_with(FUSION_MODEL_PREFIX) && !slug.starts_with(OFFICIAL_MODEL_PREFIX),
        "provider model slug uses a reserved namespace: {slug}"
    );
    Ok(())
}

pub fn catalog_model_slug(upstream_model_id: &str, provider_id: &str) -> String {
    format!("{upstream_model_id}-{provider_id}")
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
    let base_url = base_url.trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    Url::parse(&format!("{base_url}{path}")).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{
        ProviderAuthConfig, ProviderModelSource, ProviderProtocol, baidu_oneapi_provider,
        open_code_go_provider,
    };
    use crate::provider::{ProviderQuotaParser, ProviderRequestPolicy};

    #[test]
    fn resolves_exact_suffix_slug_to_provider_and_upstream_model() {
        let open_code = open_code_go_provider("opencode-go", "secret");
        let mut backup = test_provider("backup-provider");
        backup.selected_models = vec!["glm-5.2".to_owned()];
        backup.cached_models = vec![ProviderModel {
            id: "glm-5.2".to_owned(),
            ..ProviderModel::default()
        }];
        let registry = ProviderRegistry::new(vec![open_code, backup]).unwrap();

        let resolved = registry.resolve("glm-5.2-opencode-go").unwrap();
        assert_eq!(resolved.provider.id(), "opencode-go");
        assert_eq!(resolved.upstream_model_id, "glm-5.2");
        assert!(resolved.model.is_some());
        assert_eq!(
            registry
                .catalog_slugs()
                .filter(|slug| slug.ends_with("glm-5.2-opencode-go"))
                .collect::<Vec<_>>(),
            vec!["glm-5.2-opencode-go"]
        );
    }

    #[test]
    fn does_not_route_unselected_unavailable_or_disabled_models() {
        let mut provider = test_provider("selected");
        provider.selected_models = vec!["selected".to_owned(), "unavailable".to_owned()];
        provider.cached_models = vec![
            ProviderModel {
                id: "selected".to_owned(),
                ..ProviderModel::default()
            },
            ProviderModel {
                id: "not-selected".to_owned(),
                ..ProviderModel::default()
            },
        ];
        let mut disabled = test_provider("disabled");
        disabled.enabled = false;
        disabled.selected_models = vec!["model".to_owned()];
        disabled.cached_models = vec![ProviderModel {
            id: "model".to_owned(),
            ..ProviderModel::default()
        }];
        let registry = ProviderRegistry::new(vec![provider, disabled]).unwrap();

        assert!(registry.resolve("selected-selected").is_some());
        assert!(registry.resolve("unavailable-selected").is_none());
        assert!(registry.resolve_known("unavailable-selected").is_some());
        assert!(registry.resolve("not-selected-selected").is_none());
        assert!(registry.resolve("model-disabled").is_none());
        assert!(registry.resolve_known("model-disabled").is_some());
    }

    #[test]
    fn rejects_ambiguous_suffix_slugs_instead_of_parsing_them() {
        let mut first = test_provider("c");
        first.selected_models = vec!["a-b".to_owned()];
        first.cached_models = vec![ProviderModel {
            id: "a-b".to_owned(),
            ..ProviderModel::default()
        }];
        let mut second = test_provider("b-c");
        second.selected_models = vec!["a".to_owned()];
        second.cached_models = vec![ProviderModel {
            id: "a".to_owned(),
            ..ProviderModel::default()
        }];

        let error = ProviderRegistry::new(vec![first, second]).unwrap_err();
        assert!(error.to_string().contains("slug collision for a-b-c"));
    }

    #[test]
    fn suffix_slug_preserves_upstream_model_slashes() {
        assert_eq!(
            catalog_model_slug("anthropic/claude-sonnet", "openrouter"),
            "anthropic/claude-sonnet-openrouter"
        );
    }

    #[test]
    fn endpoint_url_preserves_base_path() {
        let provider = open_code_go_provider("opencode-go", "secret");
        let registry = ProviderRegistry::new(vec![provider]).unwrap();
        let runtime = registry.provider("opencode-go").unwrap();
        assert_eq!(
            runtime.api_url().as_str(),
            "https://opencode.ai/zen/go/v1/chat/completions"
        );
        assert_eq!(
            runtime.models_url().unwrap().as_str(),
            "https://opencode.ai/zen/go/v1/models"
        );
    }

    #[test]
    fn baidu_routes_gpt_to_responses_and_claude_to_messages() {
        let mut provider = baidu_oneapi_provider("baidu-oneapi", "secret");
        provider.quota_username = Some("quota-user".to_owned());
        let registry = ProviderRegistry::new(vec![provider]).unwrap();
        let runtime = registry.provider("baidu-oneapi").unwrap();

        assert_eq!(
            runtime.protocol_for_model("GPT-5.6-Sol"),
            ProviderProtocol::OpenAiResponses
        );
        assert_eq!(
            runtime.api_url_for_model("gpt-5.6-sol").as_str(),
            "https://oneapi-comate.baidu-int.com/v1/responses"
        );
        assert_eq!(
            runtime.protocol_for_model("Claude Opus 4.6"),
            ProviderProtocol::AnthropicMessages
        );
        assert_eq!(
            runtime.api_url_for_model("Claude Opus 4.6").as_str(),
            "https://oneapi-comate.baidu-int.com/v1/messages"
        );
    }

    #[test]
    fn baidu_ducc_auth_defaults_off_and_only_fails_closed_when_enabled() {
        let missing_ducc = std::path::PathBuf::from("/definitely/missing/codex-mixin-ducc");
        let mut provider = baidu_oneapi_provider("baidu-oneapi", "secret");
        provider.quota_username = Some("quota-user".to_owned());
        provider.request_policy.ducc_executable = Some(missing_ducc.clone());

        provider.request_policy.ducc_auth = None;
        let registry = ProviderRegistry::new(vec![provider.clone()]).unwrap();
        let request = registry
            .provider("baidu-oneapi")
            .unwrap()
            .apply_auth(reqwest::Client::new().get("https://example.test"))
            .build()
            .unwrap();
        assert!(!request.headers().contains_key(DUCC_HEADER_NAME));

        provider.request_policy.ducc_auth = Some(false);
        let registry = ProviderRegistry::new(vec![provider.clone()]).unwrap();
        let request = registry
            .provider("baidu-oneapi")
            .unwrap()
            .apply_auth(reqwest::Client::new().get("https://example.test"))
            .build()
            .unwrap();
        assert!(!request.headers().contains_key(DUCC_HEADER_NAME));

        provider.request_policy.ducc_auth = Some(true);
        let error = ProviderRegistry::new(vec![provider]).unwrap_err();
        assert!(format!("{error:#}").contains("configured DUCC executable does not exist"));
    }

    #[test]
    fn non_baidu_provider_keeps_configured_protocol_for_gpt() {
        let provider = test_provider("custom");
        let registry = ProviderRegistry::new(vec![provider]).unwrap();
        let runtime = registry.provider("custom").unwrap();

        assert_eq!(
            runtime.protocol_for_model("gpt-5.6-sol"),
            ProviderProtocol::OpenAiChat
        );
        assert_eq!(
            runtime.api_url_for_model("gpt-5.6-sol").as_str(),
            "https://example.test/v1/chat/completions"
        );
    }

    #[test]
    fn rejects_multiple_auxiliary_model_upstreams() {
        let mut first = test_provider("first");
        first.auxiliary_model_upstream = true;
        let mut second = test_provider("second");
        second.auxiliary_model_upstream = true;

        assert!(
            ProviderRegistry::new(vec![first, second])
                .unwrap_err()
                .to_string()
                .contains("multiple auxiliary model upstreams configured: first, second")
        );
    }

    fn test_provider(id: &str) -> ProviderDefinition {
        ProviderDefinition {
            id: id.to_owned(),
            display_name: id.to_owned(),
            enabled: true,
            auxiliary_model_upstream: false,
            preset_id: None,
            protocol: ProviderProtocol::OpenAiChat,
            base_url: "https://example.test".to_owned(),
            api_path: "/v1/chat/completions".to_owned(),
            model_source: ProviderModelSource::OpenAiCompatible {
                path: "/v1/models".to_owned(),
            },
            auth: ProviderAuthConfig {
                header: ProviderAuthHeader::AuthorizationBearer,
                api_key: "secret".to_owned(),
            },
            anthropic_version: None,
            anthropic_beta: None,
            image_generation_path: None,
            quota_url: None,
            quota_username: None,
            quota_currency: None,
            quota_parser: ProviderQuotaParser::Generic,
            request_policy: ProviderRequestPolicy::default(),
            selected_models: Vec::new(),
            new_models: Vec::new(),
            cached_models: Vec::new(),
            models_refreshed_at_ms: None,
            models_refresh_error: None,
        }
    }
}
