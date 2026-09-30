//! Opt-in ECH transport for official GPT hosts, with explicit direct fallback.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use reqwest::{Client, Url};
use rustls::client::{EchConfig, EchMode, EchStatus};
use rustls::pki_types::{EchConfigListBytes, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;

mod dns;

pub(crate) const PLUS_DOH: &str = "https://edge.1molchuan.top/plus";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(crate) struct EchConnection {
    pub(crate) client: Client,
    pub(crate) tls: Arc<ClientConfig>,
    pub(crate) addresses: Vec<SocketAddr>,
    expires: Instant,
}

pub(crate) struct OfficialEch {
    // At most two hosts. Serialize refreshes to avoid duplicate DNS work.
    cache: Mutex<HashMap<String, EchConnection>>,
    disabled: AtomicBool,
    config_path: std::path::PathBuf,
}

impl Default for OfficialEch {
    fn default() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            disabled: AtomicBool::new(false),
            config_path: crate::config::stored_config_path(),
        }
    }
}

impl OfficialEch {
    pub(crate) fn disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }

    pub(crate) async fn disable(&self, error: anyhow::Error) -> anyhow::Result<()> {
        if self.disabled.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let reason = format!("{error:#}");
        let config_path = self.config_path.clone();
        tracing::warn!(
            reason,
            "official ECH disabled; reverting to direct connections"
        );
        tokio::task::spawn_blocking(move || {
            crate::config::mutate_stored_config_at_path(&config_path, |stored| {
                stored.official_ech_proxy = false;
                stored.official_ech_fallback_reason = Some(reason);
                Ok(())
            })
        })
        .await
        .context("persist ECH fallback task")?
        .context("ECH disabled in memory, but saving fallback status failed")
    }

    #[cfg(test)]
    pub(crate) fn for_config(path: std::path::PathBuf) -> Self {
        Self {
            config_path: path,
            ..Self::default()
        }
    }
}

pub(crate) fn official_host(url: &Url) -> anyhow::Result<&str> {
    ensure!(
        matches!(url.scheme(), "https" | "wss"),
        "ECH requires HTTPS or WSS"
    );
    ensure!(
        url.port_or_known_default() == Some(443),
        "ECH requires port 443"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "ECH URL must not contain credentials"
    );
    let host = url.host_str().context("ECH URL has no host")?;
    ensure!(
        matches!(host, "chatgpt.com" | "api.openai.com"),
        "ECH is limited to chatgpt.com and api.openai.com"
    );
    Ok(host)
}

pub(crate) fn is_official_url(url: &Url) -> bool {
    matches!(url.host_str(), Some("chatgpt.com" | "api.openai.com"))
}

/// Discovery/probing runs outside the gateway transport. Other hosts keep the
/// supplied client, including its timeout and proxy settings.
pub(crate) async fn provider_http_client(
    default: &Client,
    url: &Url,
    timeout: Duration,
) -> anyhow::Result<Client> {
    if !is_official_url(url) {
        return Ok(default.clone());
    }
    let stored = tokio::task::spawn_blocking(crate::config::load_stored_config).await??;
    let Some(stored) = stored else {
        return Ok(default.clone());
    };
    if stored.official_ech_proxy {
        let transport = OfficialEch::default();
        let attempt =
            tokio::time::timeout(Duration::from_secs(20), transport.connection(url, timeout))
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result);
        match attempt {
            Ok(connection) => return Ok(connection.client),
            Err(error) => transport.disable(error).await?,
        }
    } else if stored.official_ech_fallback_reason.is_none() {
        return Ok(default.clone());
    }
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()?)
}

impl OfficialEch {
    pub(crate) async fn connection(
        &self,
        url: &Url,
        timeout: Duration,
    ) -> anyhow::Result<EchConnection> {
        let host = official_host(url)?;
        let mut cache = self.cache.lock().await;
        if let Some(connection) = cache
            .get(host)
            .filter(|entry| entry.expires > Instant::now())
        {
            return Ok(connection.clone());
        }
        // Refresh expired keys; the caller persists and reports a failed refresh.
        cache.remove(host);
        let resolution = dns::resolve(host)
            .await
            .with_context(|| format!("resolve ECH for {host}"))?;
        let tls = tls_config(&resolution.ech, resolution.alpn)?;
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, &resolution.addresses)
            .use_preconfigured_tls(tls.clone())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .pool_max_idle_per_host(8)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .context("build official ECH HTTP client")?;
        let connection = EchConnection {
            client,
            tls: Arc::new(tls),
            addresses: resolution.addresses,
            expires: Instant::now() + Duration::from_secs(u64::from(resolution.ttl.min(3600))),
        };
        // Prove ECH acceptance before any credential or model body is sent.
        connect_tls(host, &connection, false).await?;
        cache.insert(host.to_owned(), connection.clone());
        Ok(connection)
    }
}

fn tls_config(ech: &[u8], alpn: Vec<Vec<u8>>) -> anyhow::Result<ClientConfig> {
    let config = EchConfig::new(
        EchConfigListBytes::from(ech),
        rustls::crypto::aws_lc_rs::hpke::ALL_SUPPORTED_SUITES,
    )
    .context("DoH returned no compatible ECH configuration")?;
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let mut tls = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_ech(EchMode::Enable(config))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    tls.alpn_protocols = alpn;
    // Disable resumption so each new connection proves ECH acceptance.
    tls.resumption = rustls::client::Resumption::disabled();
    Ok(tls)
}

pub(crate) async fn connect_tls(
    host: &str,
    connection: &EchConnection,
    websocket: bool,
) -> anyhow::Result<(tokio_rustls::client::TlsStream<TcpStream>, SocketAddr)> {
    let mut tls = (*connection.tls).clone();
    if websocket {
        ensure!(
            tls.alpn_protocols
                .iter()
                .any(|protocol| protocol == b"http/1.1"),
            "DoH HTTPS record does not support HTTP/1.1 WebSocket"
        );
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    }
    let connector = TlsConnector::from(Arc::new(tls));
    let name = ServerName::try_from(host.to_owned())?;
    let mut last_error = None;
    for address in &connection.addresses {
        let attempt = async {
            let socket = TcpStream::connect(address)
                .await
                .context("connect to DoH address")?;
            let stream = connector
                .connect(name.clone(), socket)
                .await
                .context("ECH TLS handshake rejected or failed")?;
            ensure!(
                stream.get_ref().1.ech_status() == EchStatus::Accepted,
                "server did not accept ECH"
            );
            Ok::<_, anyhow::Error>((stream, *address))
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(error)) => last_error = Some(error),
            Err(error) => last_error = Some(error.into()),
        }
    }
    Err(last_error.context("DoH returned no connection addresses")?)
        .context("all ECH connection addresses failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_ech_to_official_tls_hosts() {
        for url in [
            "https://chatgpt.com/backend-api/codex/responses",
            "wss://api.openai.com/v1/live",
        ] {
            assert!(official_host(&Url::parse(url).unwrap()).is_ok());
        }
        for url in [
            "http://chatgpt.com",
            "https://chatgpt.com:8443",
            "https://chatgpt.com.evil.test",
            "https://127.0.0.1",
            "https://user@chatgpt.com",
        ] {
            assert!(official_host(&Url::parse(url).unwrap()).is_err(), "{url}");
        }
    }

    #[test]
    fn refuses_missing_or_invalid_ech() {
        assert!(tls_config(&[], vec![]).is_err());
        assert!(tls_config(&[0, 1, 2], vec![]).is_err());
    }

    #[tokio::test]
    #[ignore = "requires live /plus DNS and public TLS endpoints; sends no credentials"]
    async fn live_ech_comparison() {
        for host in ["chatgpt.com", "api.openai.com"] {
            let url = Url::parse(&format!("https://{host}/")).unwrap();
            let transport = OfficialEch::default();
            let connection = tokio::time::timeout(
                Duration::from_secs(20),
                transport.connection(&url, Duration::from_secs(20)),
            )
            .await
            .unwrap()
            .unwrap();
            let started = Instant::now();
            let (_, peer) = tokio::time::timeout(
                Duration::from_secs(20),
                connect_tls(host, &connection, false),
            )
            .await
            .unwrap()
            .unwrap();
            println!(
                "{host} plus_doh_with_ech peer={peer} accepted=true handshake_ms={}",
                started.elapsed().as_millis()
            );

            let plain = ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })
            .with_no_client_auth();
            let connector = TlsConnector::from(Arc::new(plain));
            let system_address = tokio::net::lookup_host((host, 443))
                .await
                .unwrap()
                .next()
                .unwrap();
            for (label, address) in [
                ("system_dns_without_ech", system_address),
                ("plus_doh_without_ech", peer),
            ] {
                let started = Instant::now();
                let attempt = async {
                    let socket = TcpStream::connect(address).await?;
                    connector
                        .connect(ServerName::try_from(host.to_owned())?, socket)
                        .await?;
                    Ok::<_, anyhow::Error>(())
                };
                let result = tokio::time::timeout(Duration::from_secs(8), attempt).await;
                println!(
                    "{host} {label} peer={address} handshake_ms={} result={result:?}",
                    started.elapsed().as_millis()
                );
            }
        }
    }
}
