use anyhow::Context;
use tokio::net::TcpStream;
use tokio_tungstenite_proxy::tungstenite::client::IntoClientRequest;
use tokio_tungstenite_proxy::tungstenite::http::Uri;
use tokio_tungstenite_proxy::tungstenite::proxy::ProxyConfig;
use tokio_tungstenite_proxy::{MaybeTlsStream, WebSocketStream, client_async_tls_with_config};

#[derive(Clone, Debug, Default)]
pub(super) struct ProxyEnv {
    http_proxy: Option<String>,
    https_proxy: Option<String>,
    all_proxy: Option<String>,
    no_proxy: Option<String>,
}

impl ProxyEnv {
    pub(super) fn from_lookup(env_lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            http_proxy: first_env(env_lookup, &["HTTP_PROXY", "http_proxy"]),
            https_proxy: first_env(env_lookup, &["HTTPS_PROXY", "https_proxy"]),
            all_proxy: first_env(env_lookup, &["ALL_PROXY", "all_proxy"]),
            no_proxy: first_env(env_lookup, &["NO_PROXY", "no_proxy"]),
        }
    }
}

fn first_env(env_lookup: &dyn Fn(&str) -> Option<String>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| env_lookup(name))
        .filter(|value| !value.trim().is_empty())
}

pub(super) async fn connect_upstream_websocket<R>(
    request: R,
    proxy_env: &ProxyEnv,
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
    let (stream, _) = client_async_tls_with_config(request, socket, None, None)
        .await
        .context("websocket handshake failed")?;
    Ok(stream)
}

fn resolve_proxy(uri: &Uri, proxy_env: &ProxyEnv) -> anyhow::Result<Option<ProxyConfig>> {
    let Some(host) = uri.host() else {
        anyhow::bail!("websocket request has no host");
    };
    let port = uri.port_u16().unwrap_or_else(|| match uri.scheme_str() {
        Some("wss") => 443,
        _ => 80,
    });
    if bypasses_proxy(host, port, proxy_env) {
        return Ok(None);
    }
    let proxy = if uri.scheme_str() == Some("wss") {
        proxy_env
            .https_proxy
            .as_deref()
            .or(proxy_env.http_proxy.as_deref())
    } else {
        proxy_env.http_proxy.as_deref()
    }
    .or(proxy_env.all_proxy.as_deref());
    let Some(value) = proxy else {
        return Ok(None);
    };
    Ok(Some(ProxyConfig::parse(value).with_context(|| {
        format!("invalid websocket proxy URL: {value:?}")
    })?))
}

fn bypasses_proxy(host: &str, port: u16, proxy_env: &ProxyEnv) -> bool {
    let Some(no_proxy) = proxy_env.no_proxy.as_deref() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if no_proxy.trim() == "*" {
        return true;
    }
    no_proxy.split(',').any(|entry| {
        let entry = entry.trim();
        if entry.is_empty() {
            return false;
        }
        let (entry_host, entry_port) = match entry.split_once(':') {
            Some((host, port)) if !entry.starts_with('[') => (host, port.parse::<u16>().ok()),
            _ => (entry, None),
        };
        if entry_port.is_some_and(|entry_port| entry_port != port) {
            return false;
        }
        let entry_host = entry_host.trim_start_matches('[').trim_end_matches(']');
        if entry_host == host {
            return true;
        }
        if let Some(suffix) = entry_host.strip_prefix('.') {
            host == suffix || host.ends_with(&format!(".{suffix}"))
        } else {
            host.ends_with(&format!(".{entry_host}"))
        }
    })
}
