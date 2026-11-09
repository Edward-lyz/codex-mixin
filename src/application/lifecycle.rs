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
    /// The recorded gateway process is alive but did not answer `/healthz` in
    /// time. A busy turn can stall the listener without the process exiting;
    /// replacing it would drop the request that caused the stall.
    pub process_alive: bool,
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
/// and a loaded but unhealthy managed job is restarted. A current process
/// that is still alive is left alone: a slow health check is not an exit.
pub fn gateway_lifecycle_action(state: GatewayObservation<'_>) -> GatewayLifecycleAction {
    let current_binary = state.gateway_version == Some(state.current_version);
    if state.service_installed {
        if current_binary && !state.daemon_running && state.service_current && state.process_alive {
            return GatewayLifecycleAction::KeepReady;
        }
        if state.gateway_healthy || state.service_loaded {
            return GatewayLifecycleAction::RestartManaged;
        }
        return GatewayLifecycleAction::StartManaged;
    }
    if current_binary && state.process_alive {
        return GatewayLifecycleAction::KeepReady;
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

#[cfg(test)]
mod tests {
    use super::{GatewayLifecycleAction, GatewayObservation, gateway_lifecycle_action};

    fn observe<'a>(healthy: bool, alive: bool, version: Option<&'a str>) -> GatewayObservation<'a> {
        GatewayObservation {
            gateway_healthy: healthy,
            process_alive: alive,
            gateway_version: version,
            daemon_running: false,
            service_installed: true,
            service_current: true,
            service_loaded: true,
            current_version: "1.0.0",
        }
    }

    #[test]
    fn a_live_current_gateway_survives_a_missed_health_check() {
        let action = gateway_lifecycle_action(observe(false, true, Some("1.0.0")));
        assert_eq!(action, GatewayLifecycleAction::KeepReady);
    }

    #[test]
    fn a_dead_loaded_service_is_still_restarted() {
        let action = gateway_lifecycle_action(observe(false, false, Some("1.0.0")));
        assert_eq!(action, GatewayLifecycleAction::RestartManaged);
    }

    #[test]
    fn a_live_process_from_another_binary_is_replaced() {
        let action = gateway_lifecycle_action(observe(false, true, Some("0.9.0")));
        assert_eq!(action, GatewayLifecycleAction::RestartManaged);
    }

    #[test]
    fn a_live_current_daemon_survives_a_missed_health_check() {
        let mut state = observe(false, true, Some("1.0.0"));
        state.service_installed = false;
        state.service_loaded = false;
        state.daemon_running = true;
        let action = gateway_lifecycle_action(state);
        assert_eq!(action, GatewayLifecycleAction::KeepReady);
    }
}
