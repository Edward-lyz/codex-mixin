use super::*;

/// Internal marker set by [`mark_loopback_peer`] for connections whose TCP
/// peer is loopback. Inbound copies are always stripped, so clients cannot
/// forge it; requests served without connection info never carry it.
pub(super) use crate::gateway_access::LOOPBACK_PEER_HEADER;
use crate::gateway_access::{PUBLIC_PEER_HEADER, bearer_token, is_trusted_peer};

pub(super) async fn mark_loopback_peer(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|axum::extract::ConnectInfo(peer)| peer.ip());
    let headers = request.headers_mut();
    headers.remove(LOOPBACK_PEER_HEADER);
    headers.remove(PUBLIC_PEER_HEADER);
    if peer.is_some_and(|ip| ip.to_canonical().is_loopback()) {
        headers.insert(
            LOOPBACK_PEER_HEADER,
            axum::http::HeaderValue::from_static("1"),
        );
    }
    if peer.is_some_and(|ip| !is_trusted_peer(ip)) {
        headers.insert(
            PUBLIC_PEER_HEADER,
            axum::http::HeaderValue::from_static("1"),
        );
    }
    next.run(request).await
}

pub(super) async fn check_gateway_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), GatewayError> {
    use subtle::ConstantTimeEq;

    // Public peers always need a key; client-auth builds also check local ones.
    let public_peer = headers.contains_key(PUBLIC_PEER_HEADER);
    if !cfg!(feature = "client-auth") && !public_peer {
        return Ok(());
    }
    let config = &state.config;
    if config.gateway_api_key.is_none() && config.gateway_api_keys.is_empty() {
        return if public_peer {
            Err(GatewayError::Unauthorized)
        } else {
            Ok(())
        };
    }
    let Some(actual) = bearer_token(headers) else {
        return Err(GatewayError::Unauthorized);
    };
    let matches_key = |expected: &str| bool::from(actual.as_bytes().ct_eq(expected.as_bytes()));
    let key_matches = config.gateway_api_key.as_deref().is_some_and(matches_key)
        || config
            .gateway_api_keys
            .iter()
            .any(|entry| matches_key(&entry.key));
    if key_matches
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
    if state.gateway.accepts_codex_oauth(headers).await {
        Ok(())
    } else {
        Err(GatewayError::Unauthorized)
    }
}

pub(super) use crate::gateway::stable_oneapi_routing;
