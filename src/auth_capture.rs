//! Shared one-shot header capture for Baidu auth-carrier CLIs.
//!
//! Both DUCC and DUCX are treated as header generators: run a short carrier
//! process through a loopback HTTP proxy, capture the native authentication
//! headers it emits, and stop before the warmup request reaches OneAPI.

use std::sync::Arc;

use anyhow::{Context, ensure};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, oneshot};

pub(crate) const NATIVE_HEADER: &str = "comate_custom_header";
const MAX_PROXY_HEAD: usize = 64 * 1024;

/// Headers worth forwarding from the carrier request onto Mixin's own request.
/// Everything else (transport, host, content-length) is rebuilt by Mixin.
fn is_capturable_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == NATIVE_HEADER
        || lower == "authorization"
        || lower == "x-api-key"
        || lower.starts_with("x-baidu")
        || lower.starts_with("comate")
}

pub(crate) struct CaptureProxy {
    pub(crate) addr: std::net::SocketAddr,
    pub(crate) captured: oneshot::Receiver<HeaderMap>,
}

impl CaptureProxy {
    pub(crate) async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind auth header capture proxy")?;
        let addr = listener
            .local_addr()
            .context("read capture proxy address")?;
        let (sender, receiver) = oneshot::channel();
        let sender = Arc::new(Mutex::new(Some(sender)));
        tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    break;
                };
                let sender = Arc::clone(&sender);
                tokio::spawn(async move {
                    if let Err(error) = proxy_connection(client, sender).await {
                        tracing::debug!(error = %format!("{error:#}"), "auth header proxy connection ended");
                    }
                });
            }
        });
        Ok(Self {
            addr,
            captured: receiver,
        })
    }
}

async fn proxy_connection(
    mut client: TcpStream,
    sender: Arc<Mutex<Option<oneshot::Sender<HeaderMap>>>>,
) -> anyhow::Result<()> {
    let (head, leftover) = read_head(&mut client).await?;
    let head_text = String::from_utf8_lossy(&head);
    let mut lines = head_text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();

    if method.eq_ignore_ascii_case("CONNECT") {
        return tunnel_connect(client, &target).await;
    }

    let (host, port, path) = split_absolute_target(&target)?;
    let mut header_map = HeaderMap::new();
    for line in lines.clone() {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && is_capturable_header(name.trim())
            && let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.trim().as_bytes()),
                HeaderValue::from_str(value.trim()),
            )
        {
            header_map.insert(name, value);
        }
    }
    if header_map.contains_key(NATIVE_HEADER) {
        let mut slot = sender.lock().await;
        if let Some(sender) = slot.take() {
            let _ = sender.send(header_map);
        }
        // The carrier CLI is only used as a header generator. Stop the warmup
        // turn before it can reach the real OneAPI and consume quota.
        return Ok(());
    }
    if sender.lock().await.is_none() {
        // Already captured by an earlier request; do not forward retries.
        return Ok(());
    }

    // Transparent forward: replay non-auth handshake traffic so the carrier can
    // finish model discovery and source-auth steps before the inference request.
    let mut origin = TcpStream::connect((host.as_str(), port))
        .await
        .with_context(|| format!("connect capture proxy origin {host}:{port}"))?;
    let mut rebuilt = format!("{method} {path} HTTP/1.1\r\n");
    for line in lines {
        if line.is_empty() {
            break;
        }
        let keep = line
            .split_once(':')
            .map(|(name, _)| {
                let lower = name.trim().to_ascii_lowercase();
                lower != "proxy-connection"
            })
            .unwrap_or(true);
        if keep {
            rebuilt.push_str(line);
            rebuilt.push_str("\r\n");
        }
    }
    rebuilt.push_str("\r\n");
    origin.write_all(rebuilt.as_bytes()).await?;
    if !leftover.is_empty() {
        origin.write_all(&leftover).await?;
    }
    origin.flush().await?;
    tokio::io::copy_bidirectional(&mut client, &mut origin)
        .await
        .context("capture proxy relay")?;
    Ok(())
}

async fn tunnel_connect(mut client: TcpStream, target: &str) -> anyhow::Result<()> {
    let (host, port) = target
        .rsplit_once(':')
        .context("CONNECT target must be host:port")?;
    let port: u16 = port.parse().context("CONNECT port")?;
    let mut origin = TcpStream::connect((host, port))
        .await
        .with_context(|| format!("connect CONNECT origin {host}:{port}"))?;
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    client.flush().await?;
    tokio::io::copy_bidirectional(&mut client, &mut origin)
        .await
        .context("capture proxy CONNECT tunnel")?;
    Ok(())
}

async fn read_head(stream: &mut TcpStream) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(index) = find_head_end(&buffer) {
            let leftover = buffer.split_off(index + 4);
            return Ok((buffer, leftover));
        }
        ensure!(
            buffer.len() <= MAX_PROXY_HEAD,
            "auth header proxy request head too large"
        );
        let read = stream.read(&mut chunk).await?;
        ensure!(
            read > 0,
            "auth header proxy connection closed before request head"
        );
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    memchr::memmem::find(buffer, b"\r\n\r\n")
}

fn split_absolute_target(target: &str) -> anyhow::Result<(String, u16, String)> {
    let without_scheme = target
        .strip_prefix("http://")
        .context("auth header proxy only forwards absolute http targets")?;
    let (authority, path) = match without_scheme.find('/') {
        Some(index) => (&without_scheme[..index], &without_scheme[index..]),
        None => (without_scheme, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host.to_owned(), port.parse().context("origin port")?),
        None => (authority.to_owned(), 80u16),
    };
    Ok((host, port, path.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_absolute_http_targets() {
        let (host, port, path) =
            split_absolute_target("http://oneapi.ai-chat.host:8602/v1/responses").unwrap();
        assert_eq!(host, "oneapi.ai-chat.host");
        assert_eq!(port, 8602);
        assert_eq!(path, "/v1/responses");
    }

    #[test]
    fn defaults_origin_port_to_80() {
        let (host, port, path) = split_absolute_target("http://ducc-auth.baidu-int.com/x").unwrap();
        assert_eq!(host, "ducc-auth.baidu-int.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/x");
    }

    #[test]
    fn only_login_derived_headers_are_capturable() {
        assert!(is_capturable_header("comate_custom_header"));
        assert!(is_capturable_header("Authorization"));
        assert!(is_capturable_header("x-api-key"));
        assert!(!is_capturable_header("content-length"));
        assert!(!is_capturable_header("host"));
    }

    #[tokio::test]
    async fn proxy_forwards_non_native_traffic() {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = origin.accept().await {
                let mut buffer = [0u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
            }
        });

        let proxy = CaptureProxy::start().await.unwrap();
        let request = format!(
            "POST http://{origin_addr}/v1/responses HTTP/1.1\r\nHost: {origin_addr}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let mut client = TcpStream::connect(proxy.addr).await.unwrap();
        client.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("ok"));
    }

    #[tokio::test]
    async fn proxy_captures_native_header_without_forwarding() {
        let proxy = CaptureProxy::start().await.unwrap();
        let request = "POST http://oneapi.invalid/v1/responses HTTP/1.1\r\nHost: oneapi.invalid\r\ncomate_custom_header: native-value\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let mut client = TcpStream::connect(proxy.addr).await.unwrap();
        client.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.is_empty());
        let captured = proxy.captured.await.unwrap();
        assert_eq!(captured.get(NATIVE_HEADER).unwrap(), "native-value");
    }
}
