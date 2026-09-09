use std::net::SocketAddr;

use crate::config::GatewayConfig;

/// Reload committed runtime configuration after startup integration work and
/// retain the listener address selected by the operating system.
pub fn reload_for_listener(actual_bind: SocketAddr) -> anyhow::Result<GatewayConfig> {
    let mut config = GatewayConfig::from_stored_config()?;
    config.bind = actual_bind;
    Ok(config)
}
