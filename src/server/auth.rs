use super::*;

pub(super) const FORWARDED_OFFICIAL_HEADERS: &[&str] = &[
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
    "x-session-id",
    "thread-id",
    "x-client-request-id",
    "x-codex-window-id",
];

pub(super) fn forward_official_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for &name in FORWARDED_OFFICIAL_HEADERS {
        if let Some(value) = headers.get(name) {
            request = request.header(name, value);
        }
    }
    request
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

pub(super) async fn check_gateway_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), GatewayError> {
    use subtle::ConstantTimeEq;

    let Some(expected) = &state.config.gateway_api_key else {
        return Ok(());
    };
    let Some(actual) = bearer_token(headers) else {
        return Err(GatewayError::Unauthorized);
    };
    if bool::from(actual.as_bytes().ct_eq(expected.as_bytes())) {
        return Ok(());
    }
    if !state.config.accept_codex_oauth || !state.config.bind.ip().is_loopback() {
        return Err(GatewayError::Unauthorized);
    }
    let (authorization, _) = state
        .official_auth()
        .await
        .map_err(|_| GatewayError::Unauthorized)?;
    let oauth_token = authorization
        .to_str()
        .ok()
        .and_then(|authorization| authorization.strip_prefix("Bearer "))
        .ok_or(GatewayError::Unauthorized)?;
    if bool::from(actual.as_bytes().ct_eq(oauth_token.as_bytes())) {
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
