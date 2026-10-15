use super::*;

/// Internal marker set by [`mark_loopback_peer`] for connections whose TCP
/// peer is loopback. Inbound copies are always stripped, so clients cannot
/// forge it; requests served without connection info never carry it.
pub(super) const LOOPBACK_PEER_HEADER: &str = "x-codex-mixin-internal-loopback-peer";

pub(super) async fn mark_loopback_peer(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let loopback = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .is_some_and(|axum::extract::ConnectInfo(peer)| peer.ip().to_canonical().is_loopback());
    let headers = request.headers_mut();
    headers.remove(LOOPBACK_PEER_HEADER);
    if loopback {
        headers.insert(
            LOOPBACK_PEER_HEADER,
            axum::http::HeaderValue::from_static("1"),
        );
    }
    next.run(request).await
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()))
}

pub(super) async fn check_gateway_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), GatewayError> {
    use subtle::ConstantTimeEq;

    if !cfg!(feature = "client-auth") {
        return Ok(());
    }
    let Some(expected) = &state.config.gateway_api_key else {
        return Ok(());
    };
    let Some(actual) = bearer_token(headers) else {
        return Err(GatewayError::Unauthorized);
    };
    if bool::from(actual.as_bytes().ct_eq(expected.as_bytes()))
        || matches!(
            state.config.gateway_client_keys.authenticate(headers),
            Some(
                crate::gateway_access::GatewayClient::Claude
                    | crate::gateway_access::GatewayClient::ClaudeDesktop
                    | crate::gateway_access::GatewayClient::Dsh
                    | crate::gateway_access::GatewayClient::OpenCode
                    | crate::gateway_access::GatewayClient::Pi
                    | crate::gateway_access::GatewayClient::Framework(_)
            )
        )
    {
        return Ok(());
    }
    // Codex OAuth passthrough is only for clients on this host, even when
    // the gateway listens on all interfaces.
    if !state.config.accept_codex_oauth || !headers.contains_key(LOOPBACK_PEER_HEADER) {
        return Err(GatewayError::Unauthorized);
    }
    let (authorization, _) = state
        .upstream
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

pub(crate) fn require_ducx_client(
    state: &AppState,
    provider: &ProviderRuntime,
    headers: &HeaderMap,
) -> Result<(), GatewayError> {
    if !cfg!(feature = "client-auth") || !provider.uses_ducx_loopback() {
        return Ok(());
    }
    state
        .config
        .gateway_client_keys
        .authenticate(headers)
        .map(|_| ())
        .ok_or(GatewayError::Unauthorized)
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
