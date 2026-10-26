use super::*;

/// Internal marker set by [`mark_loopback_peer`] for connections whose TCP
/// peer is loopback. Inbound copies are always stripped, so clients cannot
/// forge it; requests served without connection info never carry it.
pub(super) use crate::gateway_access::LOOPBACK_PEER_HEADER;
use crate::gateway_access::bearer_token;

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
    if state.gateway.accepts_codex_oauth(headers).await {
        Ok(())
    } else {
        Err(GatewayError::Unauthorized)
    }
}

pub(super) use crate::gateway::stable_oneapi_routing;
