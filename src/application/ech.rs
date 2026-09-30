//! Configure and test the official GPT ECH connection without credentials.
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use serde::Serialize;

use crate::config::{load_stored_config, mutate_stored_config};
use crate::ech::{OfficialEch, PLUS_DOH};
use crate::upstream::transport_log::{
    OfficialTransport, TransportTrace, attach_trace_for, take_trace,
};

#[derive(Serialize)]
pub struct EchStatus {
    pub enabled: bool,
    pub fallback_reason: Option<String>,
    pub doh_url: &'static str,
    pub hosts: [&'static str; 2],
}

pub fn status() -> anyhow::Result<EchStatus> {
    let stored = load_stored_config()?.unwrap_or_default();
    Ok(EchStatus {
        enabled: stored.official_ech_proxy,
        fallback_reason: stored.official_ech_fallback_reason,
        doh_url: PLUS_DOH,
        hosts: ["chatgpt.com", "api.openai.com"],
    })
}

pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    mutate_stored_config(|stored| {
        ensure!(
            !stored.providers.is_empty(),
            "provider configuration is missing"
        );
        stored.official_ech_proxy = enabled;
        stored.official_ech_fallback_reason = None;
        Ok(())
    })
}

pub fn save_probe_result(probes: &[EchProbe]) -> anyhow::Result<bool> {
    ensure!(
        probes.len() == 2 && probes[0].host == "chatgpt.com" && probes[1].host == "api.openai.com",
        "incomplete official ECH probe"
    );
    let enabled = probes.iter().all(EchProbe::ready);
    let reason = (!enabled).then(|| {
        probes
            .iter()
            .filter(|probe| !probe.ready())
            .map(|probe| {
                let reason = probe.error.clone().unwrap_or_else(|| {
                    format!("official endpoint returned HTTP {:?}", probe.http_status)
                });
                format!("{}: {reason}", probe.host)
            })
            .collect::<Vec<_>>()
            .join("; ")
    });
    mutate_stored_config(|stored| {
        ensure!(
            !stored.providers.is_empty(),
            "provider configuration is missing"
        );
        stored.official_ech_proxy = enabled;
        stored.official_ech_fallback_reason = reason;
        Ok(())
    })?;
    Ok(enabled)
}

#[derive(Serialize)]
pub struct EchProbe {
    pub host: &'static str,
    pub ech_accepted: bool,
    pub resolved_addresses: Vec<std::net::SocketAddr>,
    pub connected_address: Option<std::net::SocketAddr>,
    pub http_status: Option<u16>,
    pub elapsed_ms: u128,
    pub error: Option<String>,
    // A TLS handshake cannot prove the relay's geography or final egress IP.
    pub relay_exit_verified: bool,
}

impl EchProbe {
    pub fn ready(&self) -> bool {
        self.ech_accepted && matches!(self.http_status, Some(200..=299 | 401))
    }
}

async fn probe_host(host: &'static str) -> EchProbe {
    let started = Instant::now();
    let mut addresses = Vec::new();
    let attempt = async {
        let endpoint = if host == "chatgpt.com" {
            format!(
                "https://{host}/backend-api/codex/models?client_version={}",
                env!("CARGO_PKG_VERSION")
            )
        } else {
            format!("https://{host}/v1/models")
        };
        let url = reqwest::Url::parse(&endpoint)?;
        let connection = OfficialEch::default()
            .connection(&url, Duration::from_secs(20))
            .await?;
        addresses.clone_from(&connection.addresses);
        let builder = attach_trace_for(
            connection.client.get(url),
            TransportTrace::diagnostic(OfficialTransport::Ech(connection.transport_id)),
        )?;
        let (client, request) = builder.build_split();
        let (request, trace) = take_trace(request?)?;
        let response = trace
            .context("ECH diagnostic request trace is missing")?
            .execute(&client, request)
            .await
            .context("unauthenticated official models check over ECH")?;
        let peer = response
            .remote_addr()
            .context("ECH HTTP response has no peer address")?;
        Ok::<_, anyhow::Error>((peer, response.status().as_u16()))
    };
    let result = tokio::time::timeout(Duration::from_secs(20), attempt)
        .await
        .context("ECH connectivity test timed out")
        .and_then(|result| result);
    EchProbe {
        host,
        ech_accepted: result.is_ok(),
        resolved_addresses: addresses,
        connected_address: result.as_ref().ok().map(|(peer, _)| *peer),
        http_status: result.as_ref().ok().map(|(_, status)| *status),
        elapsed_ms: started.elapsed().as_millis(),
        error: result.err().map(|error| format!("{error:#}")),
        relay_exit_verified: false,
    }
}

pub async fn probe() -> Vec<EchProbe> {
    let (chatgpt, api) = tokio::join!(probe_host("chatgpt.com"), probe_host("api.openai.com"));
    vec![chatgpt, api]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_both_ech_and_official_endpoint_reachability() {
        let mut probe = EchProbe {
            host: "api.openai.com",
            ech_accepted: true,
            resolved_addresses: Vec::new(),
            connected_address: None,
            http_status: Some(401),
            elapsed_ms: 0,
            error: None,
            relay_exit_verified: false,
        };
        assert!(probe.ready());
        for status in [403, 421, 429, 500] {
            probe.http_status = Some(status);
            assert!(!probe.ready());
        }
        probe.http_status = Some(401);
        probe.ech_accepted = false;
        assert!(!probe.ready());
    }
}
