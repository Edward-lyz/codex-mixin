use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, ensure};
use reqwest::{RequestBuilder, Url};

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
    model_index: usize,
}

#[derive(Clone, Debug)]
pub struct ProviderRuntime {
    definition: ProviderDefinition,
    api_url: Url,
    models_url: Option<Url>,
    image_generation_url: Option<Url>,
    quota_url: Option<Url>,
}

impl ProviderRuntime {
    pub(super) fn new(definition: ProviderDefinition) -> anyhow::Result<Self> {
        definition.validate()?;
        let api_url = endpoint_url(&definition.base_url, &definition.api_path)
            .with_context(|| format!("provider {} API URL", definition.id))?;
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
        Ok(Self {
            definition,
            api_url,
            models_url,
            image_generation_url,
            quota_url,
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

    pub fn api_url(&self) -> &Url {
        &self.api_url
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
        let request = match self.definition.auth.header {
            ProviderAuthHeader::AuthorizationBearer => {
                request.bearer_auth(&self.definition.auth.api_key)
            }
            ProviderAuthHeader::XApiKey => {
                request.header("x-api-key", &self.definition.auth.api_key)
            }
        };
        if self.protocol() == ProviderProtocol::AnthropicMessages {
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
                    model_index,
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
                    .position(|model| model.id == *upstream_model_id)
                    .unwrap_or(usize::MAX);
                let target = ProviderRouteTarget {
                    provider_index,
                    upstream_model_id: upstream_model_id.clone(),
                    model_index,
                };
                if model_index == usize::MAX {
                    insert_route(&runtimes, &runtime, &mut known_routes, &slug, &target)?;
                }
                if runtime.definition.enabled && model_index != usize::MAX {
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
        let model = (target.model_index != usize::MAX)
            .then(|| provider.definition.cached_models.get(target.model_index))
            .flatten();
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
        ProviderAuthConfig, ProviderModelSource, ProviderProtocol, open_code_go_provider,
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

    fn test_provider(id: &str) -> ProviderDefinition {
        ProviderDefinition {
            id: id.to_owned(),
            display_name: id.to_owned(),
            enabled: true,
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
