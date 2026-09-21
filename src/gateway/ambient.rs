use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::Value;

use super::GatewayExecutor;
use crate::error::GatewayError;
use crate::provider::{ProviderRegistry, ProviderRuntime, catalog_model_slug};

const TURN_METADATA_KEY: &str = "x-codex-turn-metadata";
const SOURCE_PROVIDER_KEY: &str = "mixin_source_provider";

#[derive(Default, Deserialize)]
struct TurnMetadata {
    turn_trigger: Option<String>,
    thread_source: Option<String>,
    request_kind: Option<String>,
    thread_id: Option<String>,
}

#[derive(Debug)]
enum AmbientSource<'a> {
    Official,
    Provider(&'a ProviderRuntime),
}

impl GatewayExecutor {
    /// Route only Codex's ambient suggestion work through the explicitly chosen
    /// provider. A normal turn using the same model keeps its original route.
    pub(crate) fn prepare_ambient_suggestion_request(
        &self,
        headers: &HeaderMap,
        body: &mut Value,
    ) -> Result<(), GatewayError> {
        if self.providers.ambient_suggestions_provider().is_none()
            && body
                .get("client_metadata")
                .and_then(|metadata| metadata.get(SOURCE_PROVIDER_KEY))
                .is_none()
        {
            return Ok(());
        }
        let Some(metadata) = request_turn_metadata(headers, body)? else {
            return Ok(());
        };
        let Some(trigger) = ambient_trigger(&metadata) else {
            return Ok(());
        };
        let source =
            resolve_source_provider(&self.providers, self.config.accept_codex_oauth, body)?;
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
            .to_owned();
        // The source selector is a caller-owned routing contract. Consume it
        // before forwarding the request so provider internals never see it.
        if let Some(client_metadata) = body
            .get_mut("client_metadata")
            .and_then(Value::as_object_mut)
        {
            client_metadata.remove(SOURCE_PROVIDER_KEY);
        }
        // Explicit provider-qualified or fusion choices remain explicit, even
        // when the chosen provider is currently unavailable.
        if self.providers.resolve_known(&model).is_some() {
            if self.providers.resolve(&model).is_none() {
                return Err(GatewayError::BadRequest(
                    "explicit ambient suggestions model is unavailable".to_owned(),
                ));
            }
            return Ok(());
        }
        if !super::is_official_model_slug(&model) {
            return Ok(());
        }
        let provider = match source {
            Some(AmbientSource::Official) => return Ok(()),
            Some(AmbientSource::Provider(provider)) => provider,
            None => {
                let Some(provider) = self.providers.ambient_suggestions_provider() else {
                    return Ok(());
                };
                provider
            }
        };
        if !provider.definition().enabled {
            return Err(GatewayError::BadRequest(
                "ambient suggestions provider is disabled".to_owned(),
            ));
        }
        let upstream_model = provider
            .definition()
            .cached_models
            .iter()
            .find(|candidate| candidate.id.eq_ignore_ascii_case(&model))
            .ok_or_else(|| {
                GatewayError::BadRequest(
                    "ambient suggestions model is unavailable on the selected provider".to_owned(),
                )
            })?;
        let catalog_slug = catalog_model_slug(&upstream_model.id, provider.id());
        if self.providers.resolve(&catalog_slug).is_none() {
            return Err(GatewayError::BadRequest(
                "ambient suggestions model is not selected on the configured provider".to_owned(),
            ));
        }
        tracing::info!(
            turn_trigger = trigger,
            request_kind = metadata.request_kind.as_deref().unwrap_or(""),
            thread_id = metadata.thread_id.as_deref().unwrap_or(""),
            requested_model = %model,
            catalog_slug = %catalog_slug,
            provider_id = provider.id(),
            "routing ambient suggestion request to configured provider"
        );
        body["model"] = Value::String(catalog_slug);
        Ok(())
    }
}

fn resolve_source_provider<'a>(
    providers: &'a ProviderRegistry,
    accept_codex_oauth: bool,
    body: &Value,
) -> Result<Option<AmbientSource<'a>>, GatewayError> {
    let Some(client_metadata) = body.get("client_metadata") else {
        return Ok(None);
    };
    let Some(value) = client_metadata.get(SOURCE_PROVIDER_KEY) else {
        return Ok(None);
    };
    let provider = value
        .as_str()
        .filter(|value| !value.is_empty() && value.trim() == *value)
        .ok_or_else(|| {
            GatewayError::BadRequest("mixin_source_provider must be a non-empty string".to_owned())
        })?;
    if provider == "official" {
        if !accept_codex_oauth {
            return Err(GatewayError::BadRequest(
                "official source provider is unavailable".to_owned(),
            ));
        }
        return Ok(Some(AmbientSource::Official));
    }
    let provider = providers
        .provider(provider)
        .ok_or_else(|| GatewayError::BadRequest("unknown source provider".to_owned()))?;
    if !provider.definition().enabled {
        return Err(GatewayError::BadRequest(
            "source provider is disabled".to_owned(),
        ));
    }
    Ok(Some(AmbientSource::Provider(provider)))
}

fn request_turn_metadata(
    headers: &HeaderMap,
    body: &Value,
) -> Result<Option<TurnMetadata>, GatewayError> {
    let client_metadata = body.get("client_metadata");
    // Body metadata belongs to this request. A persistent WebSocket's upgrade
    // header may describe an earlier turn and must not override it.
    if let Some(value) = client_metadata.and_then(|value| value.get(TURN_METADATA_KEY)) {
        return decode_metadata(value).map(Some);
    }
    if let Some(value) = client_metadata
        && (value.get("turn_trigger").is_some() || value.get("thread_source").is_some())
    {
        return decode_metadata(value).map(Some);
    }
    let Some(value) = headers.get(TURN_METADATA_KEY) else {
        return Ok(None);
    };
    let raw = value.to_str().map_err(|_| invalid_metadata())?;
    serde_json::from_str(raw)
        .map(Some)
        .map_err(|_| invalid_metadata())
}

fn decode_metadata(value: &Value) -> Result<TurnMetadata, GatewayError> {
    if let Some(raw) = value.as_str() {
        return serde_json::from_str(raw).map_err(|_| invalid_metadata());
    }
    if !value.is_object() {
        return Err(invalid_metadata());
    }
    let field = |name| match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid_metadata()),
    };
    Ok(TurnMetadata {
        turn_trigger: field("turn_trigger")?,
        thread_source: field("thread_source")?,
        request_kind: field("request_kind")?,
        thread_id: field("thread_id")?,
    })
}

fn ambient_trigger(metadata: &TurnMetadata) -> Option<&str> {
    let trigger = match metadata.turn_trigger.as_deref() {
        Some(trigger) => trigger,
        // A startup prewarm precedes the first turn and has no turn trigger.
        // Restrict the thread-source fallback to this precise request kind so
        // later user turns cannot inherit the thread's original ambient purpose.
        None if metadata.request_kind.as_deref() == Some("prewarm") => {
            metadata.thread_source.as_deref()?
        }
        None => return None,
    };
    matches!(trigger, "ambient_suggestions" | "ambient_suggestion_safety").then_some(trigger)
}

fn invalid_metadata() -> GatewayError {
    GatewayError::BadRequest("invalid Codex turn metadata".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explicit_source_provider_needs_no_ambient_default() {
        let mut provider = crate::provider::custom_provider("source", "test-key");
        provider.base_url = "http://127.0.0.1:1".to_owned();
        let providers = ProviderRegistry::new(vec![provider]).unwrap();
        assert!(providers.ambient_suggestions_provider().is_none());

        let source = resolve_source_provider(
            &providers,
            false,
            &json!({"client_metadata": {"mixin_source_provider": "source"}}),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(source, AmbientSource::Provider(provider) if provider.id() == "source"));
        assert!(matches!(
            resolve_source_provider(
                &providers,
                true,
                &json!({"client_metadata": {"mixin_source_provider": "official"}})
            )
            .unwrap(),
            Some(AmbientSource::Official)
        ));
        assert!(
            resolve_source_provider(
                &providers,
                false,
                &json!({"client_metadata": {"mixin_source_provider": "official"}})
            )
            .is_err()
        );
    }

    #[test]
    fn explicit_source_provider_rejects_malformed_unknown_and_disabled_values() {
        let mut disabled = crate::provider::custom_provider("disabled", "test-key");
        disabled.base_url = "http://127.0.0.1:1".to_owned();
        disabled.enabled = false;
        let providers = ProviderRegistry::new(vec![disabled]).unwrap();
        for value in [
            Value::Null,
            json!(false),
            json!(7),
            json!({}),
            json!([]),
            json!(""),
            json!(" "),
            json!(" official "),
            json!("unknown"),
            json!("disabled"),
        ] {
            assert!(
                resolve_source_provider(
                    &providers,
                    true,
                    &json!({"client_metadata": {"mixin_source_provider": value}})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn source_provider_is_only_taken_from_current_body_metadata() {
        let providers = ProviderRegistry::new(Vec::new()).unwrap();
        for body in [
            json!({}),
            json!({"mixin_source_provider": "official"}),
            json!({"client_metadata": {
                "x-codex-turn-metadata": {
                    "turn_trigger": "ambient_suggestions",
                    "mixin_source_provider": "official"
                }
            }}),
        ] {
            assert!(
                resolve_source_provider(&providers, true, &body)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn canonical_body_metadata_overrides_stale_websocket_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            TURN_METADATA_KEY,
            r#"{"turn_trigger":"ambient_suggestions"}"#.parse().unwrap(),
        );
        let body = json!({
            "client_metadata": {
                "x-codex-turn-metadata": r#"{"turn_trigger":"user","thread_id":"current"}"#
            }
        });
        let metadata = request_turn_metadata(&headers, &body).unwrap().unwrap();
        assert_eq!(metadata.turn_trigger.as_deref(), Some("user"));
        assert_eq!(metadata.thread_id.as_deref(), Some("current"));
    }

    #[test]
    fn object_and_flat_metadata_are_supported_without_prompt_inspection() {
        for body in [
            json!({"client_metadata": {
                "x-codex-turn-metadata": {"turn_trigger":"ambient_suggestion_safety"}
            }}),
            json!({"client_metadata": {"turn_trigger":"ambient_suggestion_safety"}}),
        ] {
            assert_eq!(
                request_turn_metadata(&HeaderMap::new(), &body)
                    .unwrap()
                    .unwrap()
                    .turn_trigger
                    .as_deref(),
                Some("ambient_suggestion_safety")
            );
        }
        assert!(
            request_turn_metadata(
                &HeaderMap::new(),
                &json!({"input":"Generate ambient_suggestions"})
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn malformed_canonical_metadata_does_not_fall_back_to_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            TURN_METADATA_KEY,
            r#"{"turn_trigger":"ambient_suggestions"}"#.parse().unwrap(),
        );
        assert!(
            request_turn_metadata(
                &headers,
                &json!({"client_metadata":{"x-codex-turn-metadata":"invalid"}})
            )
            .is_err()
        );
    }

    #[test]
    fn startup_prewarm_uses_thread_source_only_when_turn_trigger_is_absent() {
        let mut metadata = TurnMetadata {
            thread_source: Some("ambient_suggestions".to_owned()),
            request_kind: Some("prewarm".to_owned()),
            ..TurnMetadata::default()
        };
        assert_eq!(ambient_trigger(&metadata), Some("ambient_suggestions"));
        metadata.turn_trigger = Some("user".to_owned());
        assert_eq!(ambient_trigger(&metadata), None);
        metadata.turn_trigger = None;
        metadata.request_kind = Some("turn".to_owned());
        assert_eq!(ambient_trigger(&metadata), None);
    }
}
