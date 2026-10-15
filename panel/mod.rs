//! Web control panel shell. It serves embedded static assets and a small
//! JSON API that runs the real CLI (`--no-tui`) as a child process.

mod actions;
mod api;
mod cli;
#[cfg(test)]
mod tests;

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

/// Default panel listener: all interfaces so remote machines can manage
/// a headless host. Every API call still needs the per-start token.
const DEFAULT_PANEL_BIND: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 18_790);
/// Only used to learn the primary LAN address; no packet is sent.
const ROUTE_PROBE_TARGET: (Ipv4Addr, u16) = (Ipv4Addr::new(192, 0, 2, 1), 9);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartPage {
    Dashboard,
    Setup,
}

impl StartPage {
    fn fragment(self) -> &'static str {
        match self {
            Self::Dashboard => "home",
            Self::Setup => "setup",
        }
    }
}

pub(crate) async fn run(
    start_page: StartPage,
    bind: Option<SocketAddr>,
    installed_cli_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let listener = bind_listener(bind).await?;
    let local = listener.local_addr().context("read panel listen address")?;
    let token = new_token();
    let state = Arc::new(api::PanelState {
        token: token.clone(),
        start_page: start_page.fragment(),
        installed_cli_path: installed_cli_path.map(|path| path.display().to_string()),
        action_lock: tokio::sync::Mutex::new(()),
    });
    let mut shutdown = codex_mixin::platform::ShutdownSignal::install()
        .context("install panel shutdown signal")?;
    eprintln!("Codex Mixin control panel listening on {local}");
    for host in access_hosts(local) {
        eprintln!(
            "  open http://{host}/#{}&token={token}",
            start_page.fragment()
        );
    }
    if local.ip().is_unspecified() {
        eprintln!("The panel accepts connections from other machines; keep the token private.");
    }
    eprintln!("Press Ctrl-C to stop.");
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async move { shutdown.recv().await })
        .await
        .context("serve control panel")
}

async fn bind_listener(bind: Option<SocketAddr>) -> anyhow::Result<tokio::net::TcpListener> {
    let address = bind.unwrap_or(DEFAULT_PANEL_BIND);
    match tokio::net::TcpListener::bind(address).await {
        Ok(listener) => Ok(listener),
        // Only the implicit default may move; an explicit --bind fails fast.
        Err(error) if bind.is_none() && error.kind() == io::ErrorKind::AddrInUse => {
            tokio::net::TcpListener::bind(SocketAddr::new(address.ip(), 0))
                .await
                .with_context(|| format!("bind control panel on {}", address.ip()))
        }
        Err(error) => Err(error).with_context(|| format!("bind control panel on {address}")),
    }
}

/// 256 random bits from two v4 UUIDs (OS randomness via `getrandom`).
fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Addresses a user can type into a browser for this listener.
fn access_hosts(local: SocketAddr) -> Vec<SocketAddr> {
    if !local.ip().is_unspecified() {
        return vec![local];
    }
    let mut hosts = vec![SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        local.port(),
    )];
    // Best effort: hosts without a default route only get the loopback URL.
    if let Some(lan) = primary_lan_ip() {
        hosts.push(SocketAddr::new(lan, local.port()));
    }
    hosts
}

fn primary_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(ROUTE_PROBE_TARGET).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}
