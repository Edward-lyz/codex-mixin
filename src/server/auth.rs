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
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.as_str()),
        Some(Value::String(_)) => None,
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
        return Ok(Some(UpstreamRouting {
            session_id: thread_id.to_owned(),
            hash_key: Uuid::new_v5(&Uuid::NAMESPACE_URL, cache_namespace.as_bytes()).to_string(),
        }));
    }

    let session_id = prompt_cache_key.or(x_session_id).or(session_id);
    Ok(session_id.map(|session_id| UpstreamRouting {
        session_id: session_id.to_owned(),
        hash_key: Uuid::new_v5(&Uuid::NAMESPACE_URL, session_id.as_bytes()).to_string(),
    }))
}
