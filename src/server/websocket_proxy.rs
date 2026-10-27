use crate::platform::proxy::ProxySelection;
use anyhow::Context;
use tokio::net::TcpStream;
use tokio_tungstenite_proxy::tungstenite::client::IntoClientRequest;
use tokio_tungstenite_proxy::tungstenite::http::Uri;
use tokio_tungstenite_proxy::tungstenite::proxy::ProxyConfig;
use tokio_tungstenite_proxy::{MaybeTlsStream, WebSocketStream, client_async_tls_with_config};

pub(super) fn log_official_websocket(
    stream: &WebSocketStream<MaybeTlsStream<TcpStream>>,
    host: &str,
) -> anyhow::Result<()> {
    let (socket, ech_accepted) = match stream.get_ref() {
        MaybeTlsStream::Plain(socket) => (socket, false),
        MaybeTlsStream::Rustls(tls) => (
            tls.get_ref().0,
            tls.get_ref().1.ech_status() == rustls::client::EchStatus::Accepted,
        ),
        _ => anyhow::bail!("unsupported official WebSocket TLS backend"),
    };
    let peer = socket.peer_addr().context("read official WebSocket peer")?;
    let local = socket
        .local_addr()
        .context("read official WebSocket local address")?;
    tracing::info!(event = "official_websocket_connected", host,
        peer_ip = %peer.ip(), peer_port = peer.port(), local_ip = %local.ip(), local_port = local.port(),
        ech_accepted, transport = if ech_accepted { "ech" } else { "non_ech" },
        phase = "websocket_upgrade", "official WebSocket connected");
    Ok(())
}

pub(super) async fn connect_official_websocket<R>(
    request: R,
    upstream: &crate::upstream::UpstreamAccess,
    proxy_env: &ProxySelection,
) -> anyhow::Result<WebSocketStream<MaybeTlsStream<TcpStream>>>
where
    R: IntoClientRequest + Unpin,
{
    let request = request.into_client_request()?;
    let url = reqwest::Url::parse(&request.uri().to_string())?;
    let stream = if let Some(tls) = upstream.official_tls(&url).await? {
        tokio_tungstenite_proxy::client_async(request, MaybeTlsStream::Rustls(tls))
            .await?
            .0
    } else if upstream.official_is_direct() {
        connect_upstream_websocket(request, &ProxySelection::default()).await?
    } else {
        connect_upstream_websocket(request, proxy_env).await?
    };
    log_official_websocket(
        &stream,
        url.host_str().context("official WebSocket has no host")?,
    )?;
    Ok(stream)
}

pub(super) async fn connect_upstream_websocket<R>(
    request: R,
    proxy_env: &ProxySelection,
) -> anyhow::Result<WebSocketStream<MaybeTlsStream<TcpStream>>>
where
    R: IntoClientRequest + Unpin,
{
    let request = request
        .into_client_request()
        .context("failed to build websocket request")?;
    let proxy = resolve_proxy(request.uri(), proxy_env)
        .context("failed to resolve websocket proxy configuration")?;
    let host = request
        .uri()
        .host()
        .context("websocket request has no host")?
        .to_owned();
    let port = request
        .uri()
        .port_u16()
        .or_else(|| match request.uri().scheme_str() {
            Some("wss") => Some(443),
            Some("ws") => Some(80),
            _ => None,
        })
        .context("websocket request has an unsupported scheme")?;
    let socket = match proxy {
        Some(proxy) => {
            let socket = TcpStream::connect(proxy.authority())
                .await
                .context("failed to connect to websocket proxy")?;
            tokio_tungstenite_proxy::proxy::connect_via_proxy(socket, &proxy, &host, port)
                .await
                .context("failed to establish websocket proxy tunnel")?
        }
        None => TcpStream::connect(format!("{host}:{port}"))
            .await
            .context("failed to connect to websocket upstream")?,
    };
    // Keep the existing ring transport explicit: ECH adds a second provider.
    let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    })
    .with_no_client_auth();
    let connector = tokio_tungstenite_proxy::Connector::Rustls(std::sync::Arc::new(tls));
    let (stream, _) = client_async_tls_with_config(request, socket, None, Some(connector))
        .await
        .context("websocket handshake failed")?;
    Ok(stream)
}

fn resolve_proxy(uri: &Uri, selection: &ProxySelection) -> anyhow::Result<Option<ProxyConfig>> {
    let Some(host) = uri.host() else {
        anyhow::bail!("websocket request has no host");
    };
    let port = uri.port_u16().unwrap_or_else(|| match uri.scheme_str() {
        Some("wss") => 443,
        _ => 80,
    });
    let Some(value) = selection.select(uri.scheme_str().unwrap_or("ws"), host, port) else {
        return Ok(None);
    };
    Ok(Some(ProxyConfig::parse(&value).with_context(|| {
        format!("invalid websocket proxy URL: {value:?}")
    })?))
}
