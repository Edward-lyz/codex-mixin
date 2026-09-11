use crate::error::GatewayError;
use crate::fusion::{FUSION_MODEL_PREFIX, FusionProfile};
use crate::provider::{AUTO_REVIEW_MODEL_ID, ProviderRegistry};

pub(crate) const AUTO_REVIEW_MODEL_SLUG: &str = AUTO_REVIEW_MODEL_ID;

pub(crate) fn is_official_model_slug(model: &str) -> bool {
    model
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("gpt-"))
        || model.eq_ignore_ascii_case(AUTO_REVIEW_MODEL_SLUG)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedModelRoute {
    Official,
    Fusion {
        profile_id: String,
    },
    Provider {
        catalog_slug: String,
        provider_id: String,
        upstream_model_id: String,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct ModelRouter<'a> {
    fusion_profiles: &'a [FusionProfile],
    providers: &'a ProviderRegistry,
    official_routes_enabled: bool,
}

impl<'a> ModelRouter<'a> {
    pub(crate) fn new(
        fusion_profiles: &'a [FusionProfile],
        providers: &'a ProviderRegistry,
        official_routes_enabled: bool,
    ) -> Self {
        Self {
            fusion_profiles,
            providers,
            official_routes_enabled,
        }
    }

    pub(crate) fn resolve(self, model: &str) -> Result<ResolvedModelRoute, GatewayError> {
        if let Some(profile_id) = model.strip_prefix(FUSION_MODEL_PREFIX) {
            if self
                .fusion_profiles
                .iter()
                .any(|profile| profile.id == profile_id)
            {
                return Ok(ResolvedModelRoute::Fusion {
                    profile_id: profile_id.to_owned(),
                });
            }
            return Err(GatewayError::BadRequest(format!(
                "unknown fusion profile: {profile_id}"
            )));
        }
        if let Some(resolved) = self.providers.resolve(model) {
            return Ok(ResolvedModelRoute::Provider {
                catalog_slug: model.to_owned(),
                provider_id: resolved.provider.id().to_owned(),
                upstream_model_id: resolved.upstream_model_id.to_owned(),
            });
        }
        if model.eq_ignore_ascii_case(AUTO_REVIEW_MODEL_SLUG)
            && let Some(resolved) = self
                .providers
                .resolve_auxiliary_model(AUTO_REVIEW_MODEL_SLUG)
        {
            return Ok(ResolvedModelRoute::Provider {
                catalog_slug: resolved.catalog_slug.to_owned(),
                provider_id: resolved.provider.id().to_owned(),
                upstream_model_id: resolved.upstream_model_id.to_owned(),
            });
        }
        if let Some(known) = self.providers.resolve_known(model) {
            let reason = if !known.provider.definition().enabled {
                "provider is disabled"
            } else if known.model.is_none() {
                "model is currently unavailable"
            } else {
                "model is not routable"
            };
            return Err(GatewayError::BadRequest(format!(
                "model {model} is not available: provider {} {reason}",
                known.provider.id()
            )));
        }
        if self.official_routes_enabled && is_official_model_slug(model) {
            return Ok(ResolvedModelRoute::Official);
        }
        Err(GatewayError::BadRequest(format!(
            "unknown model slug: {model}"
        )))
    }
}

/// Derive the per-session upstream routing (cache namespace) from the client
/// headers and body, mirroring how the official client keys its sessions.
pub(crate) fn stable_oneapi_routing(
    headers: &axum::http::HeaderMap,
    body: &serde_json::Value,
) -> Result<Option<super::UpstreamRouting>, GatewayError> {
    let read_header = |header_name: &'static str| -> Result<Option<&str>, GatewayError> {
        let Some(value) = headers.get(header_name) else {
            return Ok(None);
        };
        let value = value.to_str().map_err(|error| {
            GatewayError::BadRequest(format!("invalid {header_name} header: {error}"))
        })?;
        Ok((!value.is_empty()).then_some(value))
    };
    let thread_id = read_header("thread-id")?;
    let x_session_id = read_header("x-session-id")?;
    let session_id = read_header("session-id")?;
    let subagent = read_header("x-openai-subagent")?;
    let prompt_cache_key = match body.get("prompt_cache_key") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value)) if !value.is_empty() => Some(value.as_str()),
        Some(serde_json::Value::String(_)) => None,
        Some(_) => {
            return Err(GatewayError::BadRequest(
                "prompt_cache_key must be a string".to_owned(),
            ));
        }
    };

    if let Some(thread_id) = thread_id {
        let mut cache_namespace = format!("thread-id\0{thread_id}");
        if let Some(prompt_cache_key) = prompt_cache_key
            && Some(prompt_cache_key) != session_id
            && Some(prompt_cache_key) != x_session_id
        {
            cache_namespace.push_str("\0prompt-cache-key\0");
            cache_namespace.push_str(prompt_cache_key);
        }
        if let Some(subagent) = subagent {
            cache_namespace.push_str("\0subagent\0");
            cache_namespace.push_str(subagent);
        }
        return Ok(Some(super::UpstreamRouting {
            session_id: thread_id.to_owned(),
            hash_key: uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, cache_namespace.as_bytes())
                .to_string(),
        }));
    }

    let session_id = prompt_cache_key.or(x_session_id).or(session_id);
    Ok(session_id.map(|session_id| super::UpstreamRouting {
        session_id: session_id.to_owned(),
        hash_key: uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, session_id.as_bytes()).to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fusion::PanelToolsConfig;
    use crate::provider::{ProviderModel, custom_provider};

    fn fusion_profile(id: &str) -> FusionProfile {
        FusionProfile {
            id: id.to_owned(),
            mode: Default::default(),
            time_routes: Vec::new(),
            panel_models: vec!["panel-provider".to_owned()],
            judge_model: "judge-provider".to_owned(),
            final_model: "final-provider".to_owned(),
            min_successful: 1,
            max_completion_tokens: 1024,
            timeout_ms: 30_000,
            show_intermediate_results: true,
            panel_tools: PanelToolsConfig::default(),
        }
    }

    fn provider_registry_with_model(
        model_id: &str,
        enabled: bool,
        selected: bool,
    ) -> ProviderRegistry {
        let mut provider = custom_provider("provider", "secret");
        provider.base_url = "https://provider.example".to_owned();
        provider.enabled = enabled;
        provider.cached_models = vec![ProviderModel {
            id: model_id.to_owned(),
            ..ProviderModel::default()
        }];
        provider.selected_models = if selected {
            vec![model_id.to_owned()]
        } else {
            Vec::new()
        };
        ProviderRegistry::new(vec![provider]).unwrap()
    }

    fn provider_registry(enabled: bool, selected: bool) -> ProviderRegistry {
        provider_registry_with_model("gpt-5.6-sol", enabled, selected)
    }

    #[test]
    fn resolves_fusion_provider_and_official_routes_in_priority_order() {
        let profiles = [fusion_profile("default")];
        let providers = provider_registry(true, true);
        let router = ModelRouter::new(&profiles, &providers, true);

        assert_eq!(
            router.resolve("mixin/fusion/default").unwrap(),
            ResolvedModelRoute::Fusion {
                profile_id: "default".to_owned()
            }
        );
        assert_eq!(
            router.resolve("gpt-5.6-sol-provider").unwrap(),
            ResolvedModelRoute::Provider {
                catalog_slug: "gpt-5.6-sol-provider".to_owned(),
                provider_id: "provider".to_owned(),
                upstream_model_id: "gpt-5.6-sol".to_owned(),
            }
        );
        assert_eq!(
            router.resolve("GPT-5.6-SOL").unwrap(),
            ResolvedModelRoute::Official
        );
    }

    #[test]
    fn reports_unknown_fusion_disabled_provider_and_unknown_model_distinctly() {
        let profiles = [fusion_profile("default")];
        let providers = provider_registry(false, true);
        let router = ModelRouter::new(&profiles, &providers, true);

        assert_eq!(
            router
                .resolve("mixin/fusion/missing")
                .unwrap_err()
                .to_string(),
            "bad request: unknown fusion profile: missing"
        );
        assert_eq!(
            router
                .resolve("gpt-5.6-sol-provider")
                .unwrap_err()
                .to_string(),
            "bad request: model gpt-5.6-sol-provider is not available: provider provider provider is disabled"
        );
        assert_eq!(
            router.resolve("claude-unknown").unwrap_err().to_string(),
            "bad request: unknown model slug: claude-unknown"
        );
    }

    #[test]
    fn reports_selected_but_uncached_provider_model_as_unavailable() {
        let profiles = [];
        let mut provider = custom_provider("provider", "secret");
        provider.base_url = "https://provider.example".to_owned();
        provider.selected_models = vec!["missing".to_owned()];
        let providers = ProviderRegistry::new(vec![provider]).unwrap();

        assert_eq!(
            ModelRouter::new(&profiles, &providers, true)
                .resolve("missing-provider")
                .unwrap_err()
                .to_string(),
            "bad request: model missing-provider is not available: provider provider model is currently unavailable"
        );
    }

    #[test]
    fn oauth_routes_auto_review_official_and_keeps_custom_model_qualified() {
        let profiles = [];
        let providers = provider_registry_with_model(AUTO_REVIEW_MODEL_SLUG, true, true);
        let router = ModelRouter::new(&profiles, &providers, true);

        assert_eq!(
            router.resolve(AUTO_REVIEW_MODEL_SLUG).unwrap(),
            ResolvedModelRoute::Official
        );
        assert_eq!(
            router.resolve("codex-auto-review-provider").unwrap(),
            ResolvedModelRoute::Provider {
                catalog_slug: "codex-auto-review-provider".to_owned(),
                provider_id: "provider".to_owned(),
                upstream_model_id: AUTO_REVIEW_MODEL_SLUG.to_owned(),
            }
        );
    }

    #[test]
    fn custom_only_leaves_auto_review_unclaimed_and_routes_qualified_custom_model() {
        let profiles = [];
        let providers = provider_registry_with_model(AUTO_REVIEW_MODEL_SLUG, true, true);
        let router = ModelRouter::new(&profiles, &providers, false);

        assert_eq!(
            router
                .resolve(AUTO_REVIEW_MODEL_SLUG)
                .unwrap_err()
                .to_string(),
            "bad request: unknown model slug: codex-auto-review"
        );
        assert_eq!(
            router.resolve("codex-auto-review-provider").unwrap(),
            ResolvedModelRoute::Provider {
                catalog_slug: "codex-auto-review-provider".to_owned(),
                provider_id: "provider".to_owned(),
                upstream_model_id: AUTO_REVIEW_MODEL_SLUG.to_owned(),
            }
        );
    }

    #[test]
    fn auxiliary_provider_routes_auto_review_and_overrides_official() {
        let profiles = [];
        let mut provider = custom_provider("provider", "secret");
        provider.base_url = "https://provider.example".to_owned();
        provider.auxiliary_model_upstream = true;
        provider.cached_models = vec![ProviderModel {
            id: AUTO_REVIEW_MODEL_SLUG.to_owned(),
            ..ProviderModel::default()
        }];
        let providers = ProviderRegistry::new(vec![provider]).unwrap();

        for official_routes_enabled in [false, true] {
            assert_eq!(
                ModelRouter::new(&profiles, &providers, official_routes_enabled)
                    .resolve(AUTO_REVIEW_MODEL_SLUG)
                    .unwrap(),
                ResolvedModelRoute::Provider {
                    catalog_slug: "codex-auto-review-provider".to_owned(),
                    provider_id: "provider".to_owned(),
                    upstream_model_id: AUTO_REVIEW_MODEL_SLUG.to_owned(),
                }
            );
        }
    }
}
