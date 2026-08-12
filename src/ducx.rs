//! Managed DUCX authentication capture.
//!
//! DUCX is Baidu's Codex fork. Its default model proxy mints a per-session
//! `comate_custom_header` (and bearer token) from the login state and sends the
//! OneAPI request in plaintext HTTP. We cannot redirect that request through a
//! configured `base_url` (the model proxy overrides provider config), but the
//! Go HTTP client honours `HTTP_PROXY`. So we run a one-shot local forward proxy,
//! let DUCX perform its real auth handshake, and sniff the native headers off the
//! first proxied request that carries `comate_custom_header`. Mixin then injects
//! those headers into its own upstream request. Nothing is fabricated: the header
//! is the one DUCX itself generated.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::{Mutex, oneshot};

const NATIVE_HEADER: &str = "comate_custom_header";
/// Auth handshake hosts DUCX must reach directly. Only the OneAPI inference host
/// is routed through our capture proxy; proxying the source-auth handshake makes
/// DUCX's `generate source auth` call fail, so these bypass the proxy.
const DIRECT_HOSTS: &str = "baidu-int.com,bcebos.com,baidu.com,openai.com,chatgpt.com";
/// DUCX platform identity required for the comate source-auth handshake.
const DUCX_PLATFORM: &str = "AIIDE-terminal";
/// Captured headers are reused until this TTL elapses to avoid spawning DUCX per
/// request. The minted token is short lived upstream, so keep the window small.
const HEADER_TTL: Duration = Duration::from_secs(600);
const MAX_PROXY_HEAD: usize = 64 * 1024;

/// Headers worth forwarding from the DUCX request onto Mixin's own request.
/// Everything else (transport, host, content-length) is rebuilt by Mixin.
fn is_capturable_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == NATIVE_HEADER
        || lower == "authorization"
        || lower == "x-api-key"
        || lower.starts_with("x-baidu")
        || lower.starts_with("comate")
}

struct CapturedHeaders {
    headers: HeaderMap,
    at: Instant,
}

pub(crate) struct DucxRuntime {
    executable: PathBuf,
    home: PathBuf,
    cached: Mutex<Option<CapturedHeaders>>,
}

impl DucxRuntime {
    pub(crate) async fn spawn(executable: PathBuf) -> anyhow::Result<Self> {
        ensure!(
            executable.is_file(),
            "DUCX executable does not exist: {}",
            executable.display()
        );
        let home = managed_home(&executable)?;
        Ok(Self {
            executable,
            home,
            cached: Mutex::new(None),
        })
    }

    /// Return the DUCX-native authentication headers, minting them via a one-shot
    /// forward-proxy capture when the cache is empty or stale.
    pub(crate) async fn native_headers(&self, timeout: Duration) -> anyhow::Result<HeaderMap> {
        let mut cached = self.cached.lock().await;
        if let Some(entry) = cached.as_ref()
            && entry.at.elapsed() < HEADER_TTL
        {
            return Ok(entry.headers.clone());
        }
        let headers = self.mint_headers(timeout).await?;
        *cached = Some(CapturedHeaders {
            headers: headers.clone(),
            at: Instant::now(),
        });
        Ok(headers)
    }

    async fn mint_headers(&self, timeout: Duration) -> anyhow::Result<HeaderMap> {
        let proxy = CaptureProxy::start().await?;
        let proxy_url = format!("http://{}", proxy.addr);
        // Login state and config live under HOME/.baidu-cx (CODEX_HOME).
        let codex_home = self.home.join(".baidu-cx");
        let mut child = Command::new(&self.executable)
            .args([
                "exec",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
                "codex-mixin auth warmup, reply ok",
            ])
            .env("HOME", &self.home)
            .env("CODEX_HOME", &codex_home)
            .env("HTTP_PROXY", &proxy_url)
            .env("http_proxy", &proxy_url)
            .env("NO_PROXY", DIRECT_HOSTS)
            .env("no_proxy", DIRECT_HOSTS)
            .env("BAIDU_CX_PLATFORM", DUCX_PLATFORM)
            .env("DISABLE_DUCX_CLI_UPDATE", "1")
            .env("DISABLE_BAIDU_CLAUDE_UPDATE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("start managed DUCX {}", self.executable.display()))?;
        let captured = tokio::time::timeout(timeout, proxy.captured)
            .await
            .context("DUCX did not emit an authenticated request in time")?
            .context("DUCX capture proxy closed before capturing native headers")?;
        let _ = child.start_kill();
        Ok(captured)
    }
}

/// One-shot transparent forward proxy. It streams every proxied connection to the
/// real origin and, on the first request that carries `comate_custom_header`,
/// reports the captured headers over the oneshot channel. It never blocks DUCX's
/// own traffic, so the auth handshake completes normally.
struct CaptureProxy {
    addr: std::net::SocketAddr,
    captured: oneshot::Receiver<HeaderMap>,
}

impl CaptureProxy {
    async fn start() -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind DUCX capture proxy")?;
        let addr = listener.local_addr().context("read DUCX proxy address")?;
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
                        tracing::debug!(error = %format!("{error:#}"), "DUCX proxy connection ended");
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
    // Capture native headers off the first request that carries the DUCX header.
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
    if header_map.contains_key(NATIVE_HEADER)
        && let Some(sender) = sender.lock().await.take()
    {
        let _ = sender.send(header_map);
    }

    // Transparent forward: replay the request in origin-form, then splice both ways.
    let mut origin = TcpStream::connect((host.as_str(), port))
        .await
        .with_context(|| format!("connect DUCX proxy origin {host}:{port}"))?;
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
        .context("DUCX proxy relay")?;
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
        .context("DUCX CONNECT tunnel")?;
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
            "DUCX proxy request head too large"
        );
        let read = stream.read(&mut chunk).await?;
        ensure!(read > 0, "DUCX proxy connection closed before request head");
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    memchr::memmem::find(buffer, b"\r\n\r\n")
}

fn split_absolute_target(target: &str) -> anyhow::Result<(String, u16, String)> {
    let without_scheme = target
        .strip_prefix("http://")
        .context("DUCX proxy only forwards absolute http targets")?;
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

fn managed_home(executable: &Path) -> anyhow::Result<PathBuf> {
    let bin = executable
        .parent()
        .context("DUCX executable has no bin directory")?;
    let install = bin
        .parent()
        .context("DUCX executable has no install directory")?;
    let root = install
        .parent()
        .context("DUCX executable has no .baidu-cx directory")?;
    ensure!(
        install.file_name().and_then(|value| value.to_str()) == Some("baidu-cx")
            && root.file_name().and_then(|value| value.to_str()) == Some(".baidu-cx"),
        "DUCX executable must use the managed HOME/.baidu-cx/baidu-cx/bin layout"
    );
    Ok(root
        .parent()
        .context("DUCX executable has no managed HOME")?
        .to_owned())
}

/// Default managed DUCX executable location under the Mixin-managed home.
pub(crate) fn default_ducx_executable() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let candidates: HashMap<&str, PathBuf> = [
        (
            "ducx",
            home.join(".codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/ducx"),
        ),
        (
            "codex",
            home.join(".codex-mixin/ducx/home/.baidu-cx/baidu-cx/bin/codex"),
        ),
    ]
    .into_iter()
    .collect();
    candidates.into_values().find(|path| path.is_file())
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
    async fn proxy_captures_native_header_and_forwards_traffic() {
        // Origin echoes a fixed body so we can assert the proxy relayed the request.
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
            "POST http://{origin_addr}/v1/responses HTTP/1.1\r\nHost: {origin_addr}\r\ncomate_custom_header: native-value\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let mut client = TcpStream::connect(proxy.addr).await.unwrap();
        client.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("ok"));

        let captured = proxy.captured.await.unwrap();
        assert_eq!(captured.get(NATIVE_HEADER).unwrap(), "native-value");
    }
}
