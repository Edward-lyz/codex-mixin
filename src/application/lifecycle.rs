use std::net::SocketAddr;

use crate::config::GatewayConfig;

/// Reload committed runtime configuration after startup integration work and
/// retain the listener address selected by the operating system.
pub fn reload_for_listener(actual_bind: SocketAddr) -> anyhow::Result<GatewayConfig> {
    let mut config = GatewayConfig::from_stored_config()?;
    config.bind = actual_bind;
    Ok(config)
}

/// What `service ensure` must do to leave exactly one current gateway running.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayLifecycleAction {
    KeepReady,
    /// Stop every gateway instance, refresh the service definition when
    /// stale, and start through the service manager.
    RestartManaged,
    RestartDaemon,
    StartManaged,
    StartDaemon,
}

/// Observed gateway and startup-service state used by the lifecycle policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayObservation<'a> {
    pub gateway_healthy: bool,
    pub gateway_version: Option<&'a str>,
    pub daemon_running: bool,
    pub service_installed: bool,
    pub service_current: bool,
    pub service_loaded: bool,
    pub current_version: &'a str,
}

/// Select the lifecycle operation for `service ensure`.
///
/// A gateway from another binary version is always replaced. When the user
/// enabled startup at login, a gateway still running as a plain daemon or
/// from a stale service definition is migrated under the service manager,
/// and a loaded but unhealthy managed job is restarted.
pub fn gateway_lifecycle_action(state: GatewayObservation<'_>) -> GatewayLifecycleAction {
    let current_binary = state.gateway_version == Some(state.current_version);
    if state.service_installed {
        if state.gateway_healthy && current_binary && !state.daemon_running && state.service_current
        {
            return GatewayLifecycleAction::KeepReady;
        }
        if state.gateway_healthy || state.service_loaded {
            return GatewayLifecycleAction::RestartManaged;
        }
        return GatewayLifecycleAction::StartManaged;
    }
    match (state.gateway_healthy, current_binary) {
        (true, true) => GatewayLifecycleAction::KeepReady,
        (true, false) => GatewayLifecycleAction::RestartDaemon,
        // A job the service manager still runs after its definition was
        // removed must be stopped before a daemon takes the port.
        (false, _) if state.service_loaded => GatewayLifecycleAction::RestartDaemon,
        (false, _) => GatewayLifecycleAction::StartDaemon,
    }
}
