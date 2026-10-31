use crate::config::{FUSION_MODEL_PREFIX, FusionProfile};
use crate::error::GatewayError;
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
        if model.eq_ignore_ascii_case(AUTO_REVIEW_MODEL_SLUG)
            && self.providers.providers().iter().any(|provider| {
                provider.definition().enabled && provider.definition().auto_review_model.is_some()
            })
        {
            return Err(GatewayError::BadRequest(
                "configured Codex review model is unavailable".to_owned(),
            ));
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
    let opencode_session = read_header(crate::provider::OPENCODE_SESSION_HEADER)?;
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
            && Some(prompt_cache_key) != opencode_session
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

    let session_id = opencode_session
        .or(prompt_cache_key)
        .or(x_session_id)
        .or(session_id)
        .map(str::to_owned)
        .or_else(|| claude_metadata_session(body));
    Ok(session_id.map(|session_id| super::UpstreamRouting {
        hash_key: uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, session_id.as_bytes()).to_string(),
        session_id,
    }))
}

fn claude_metadata_session(body: &serde_json::Value) -> Option<String> {
    let user_id = body.pointer("/metadata/user_id")?.as_str()?;
    // Claude Code has shipped both a structured user_id string and the older
    // user_*_account_*_session_UUID form. Ordinary Anthropic user IDs are not
    // session identities and must not merge conversations.
    let id = if user_id.starts_with('{') {
        let metadata: serde_json::Value = serde_json::from_str(user_id).ok()?;
        uuid::Uuid::parse_str(metadata.get("session_id")?.as_str()?).ok()?
    } else {
        if !user_id.starts_with("user_") {
            return None;
        }
        uuid::Uuid::parse_str(user_id.rsplit_once("_session_")?.1).ok()?
    };
    Some(id.to_string())
}

pub(super) fn is_codex_thread_title(
    headers: &axum::http::HeaderMap,
    body: &serde_json::Value,
) -> Result<bool, crate::error::GatewayError> {
    const METADATA_HEADER: &str = "x-codex-turn-metadata";
    const MAX_METADATA_BYTES: usize = 64 * 1024;
    let raw = if let Some(value) = body
        .get("client_metadata")
        .and_then(|metadata| metadata.get(METADATA_HEADER))
    {
        Some(value.as_str().ok_or_else(|| {
            crate::error::GatewayError::BadRequest(
                "Codex turn metadata must be a JSON string".to_owned(),
            )
        })?)
    } else {
        headers
            .get(METADATA_HEADER)
            .map(|value| {
                value.to_str().map_err(|_| {
                    crate::error::GatewayError::BadRequest(
                        "Codex turn metadata must be UTF-8".to_owned(),
                    )
                })
            })
            .transpose()?
    };
    let Some(raw) = raw else {
        return Ok(false);
    };
    if raw.len() > MAX_METADATA_BYTES {
        return Err(crate::error::GatewayError::BadRequest(
            "Codex turn metadata exceeds 64 KiB".to_owned(),
        ));
    }
    let metadata: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
        crate::error::GatewayError::BadRequest(format!("invalid Codex turn metadata: {error}"))
    })?;
    Ok(["thread_source", "turn_trigger"].iter().any(|field| {
        metadata.get(*field).and_then(serde_json::Value::as_str) == Some("thread_title")
    }))
}
