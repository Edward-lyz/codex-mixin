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

pub(super) use crate::gateway::stable_oneapi_routing;
