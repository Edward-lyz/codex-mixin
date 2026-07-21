use super::*;

pub(super) fn forward_official_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for name in [
        "openai-beta",
        "x-codex-installation-id",
        "x-codex-beta-features",
        "originator",
        "x-codex-originator",
        "x-openai-subagent",
        "x-openai-memgen-request",
        "x-codex-turn-state",
        "x-codex-turn-metadata",
        "x-codex-parent-thread-id",
        "x-oai-attestation",
        "x-responsesapi-include-timing-metrics",
        "x-openai-internal-codex-responses-lite",
        "openai-organization",
        "openai-project",
        "user-agent",
        "accept-language",
        "session-id",
        "thread-id",
        "x-client-request-id",
        "x-codex-window-id",
    ] {
        if let Some(value) = headers.get(name) {
            request = request.header(name, value);
        }
    }
    request
}

pub(super) fn should_forward_to_official(body: &Value) -> bool {
    let Some(model) = body.get("model").and_then(Value::as_str) else {
        return false;
    };
    model_route(model) == ModelRoute::Official
}

pub(super) fn normalize_custom_model_alias(body: &mut Value) {
    let Some(model) = body.get("model").and_then(Value::as_str) else {
        return;
    };
    let canonical = canonical_upstream_model_alias(model);
    if canonical != model {
        body["model"] = Value::String(canonical.to_owned());
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

pub(super) fn check_gateway_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), GatewayError> {
    use subtle::ConstantTimeEq;

    let Some(expected) = &state.config.gateway_api_key else {
        return Ok(());
    };
    let actual = bearer_token(headers);
    let accepts_codex_oauth =
        state.config.accept_codex_oauth && state.config.bind.ip().is_loopback() && actual.is_some();
    let gateway_key_matches =
        actual.is_some_and(|actual| actual.as_bytes().ct_eq(expected.as_bytes()).into());
    if accepts_codex_oauth || gateway_key_matches {
        Ok(())
    } else {
        Err(GatewayError::Unauthorized)
    }
}

pub(super) fn stable_oneapi_routing(
    headers: &HeaderMap,
    body: &Value,
) -> Result<Option<UpstreamRouting>, GatewayError> {
    let mut route_key = None;
    for header_name in ["session-id", "thread-id", "x-client-request-id"] {
        if let Some(value) = headers.get(header_name) {
            let value = value.to_str().map_err(|error| {
                GatewayError::BadRequest(format!("invalid {header_name} header: {error}"))
            })?;
            if !value.is_empty() {
                route_key = Some(value);
                break;
            }
        }
    }
    if route_key.is_none() {
        match body.get("prompt_cache_key") {
            None | Some(Value::Null) => {}
            Some(Value::String(prompt_cache_key)) if !prompt_cache_key.is_empty() => {
                route_key = Some(prompt_cache_key);
            }
            Some(Value::String(_)) => {}
            Some(_) => {
                return Err(GatewayError::BadRequest(
                    "prompt_cache_key must be a string".to_owned(),
                ));
            }
        }
    }
    Ok(route_key.map(|session_id| UpstreamRouting {
        session_id: session_id.to_owned(),
        hash_key: Uuid::new_v5(&Uuid::NAMESPACE_URL, session_id.as_bytes()).to_string(),
    }))
}
