//! Listener allocation shared by the gateway and control panel.

use std::io;
use std::net::SocketAddr;

use anyhow::Context;
use tokio::net::TcpListener;

const FIRST_PORT: u16 = 8000;
const LAST_PORT: u16 = 9000;

pub(super) async fn bind(address: SocketAddr, automatic: bool) -> anyhow::Result<TcpListener> {
    if !automatic {
        return TcpListener::bind(address)
            .await
            .with_context(|| format!("bind listener on {address}"));
    }
    // Keep a saved in-range endpoint stable so managed clients need no rewrite.
    if (FIRST_PORT..=LAST_PORT).contains(&address.port()) {
        match TcpListener::bind(address).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {}
            Err(error) => return Err(error).context("bind saved listener address"),
        }
    }
    let port_count = LAST_PORT - FIRST_PORT + 1;
    let start = (uuid::Uuid::new_v4().as_u128() % u128::from(port_count)) as u16;
    bind_port_range(address, FIRST_PORT, port_count, start).await
}

async fn bind_port_range(
    mut address: SocketAddr,
    first: u16,
    count: u16,
    start: u16,
) -> anyhow::Result<TcpListener> {
    for offset in 0..count {
        address.set_port(first + (start + offset) % count);
        match TcpListener::bind(address).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {}
            Err(error) => {
                return Err(error).with_context(|| format!("bind listener on {address}"));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        format!(
            "no available listener port on {} in {first}..={}",
            address.ip(),
            first + count - 1
        ),
    )
    .into())
}
