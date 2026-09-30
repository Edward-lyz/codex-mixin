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

#[cfg(test)]
mod tests {
    use super::{GatewayLifecycleAction as Action, GatewayObservation, gateway_lifecycle_action};

    fn running_daemon() -> GatewayObservation<'static> {
        GatewayObservation {
            gateway_healthy: true,
            gateway_version: Some("1.0.0"),
            daemon_running: true,
            service_installed: false,
            service_current: false,
            service_loaded: false,
            current_version: "1.0.0",
        }
    }

    fn running_managed() -> GatewayObservation<'static> {
        GatewayObservation {
            daemon_running: false,
            service_installed: true,
            service_current: true,
            service_loaded: true,
            ..running_daemon()
        }
    }

    #[test]
    fn keeps_a_current_gateway_in_either_mode() {
        assert_eq!(
            gateway_lifecycle_action(running_daemon()),
            Action::KeepReady
        );
        assert_eq!(
            gateway_lifecycle_action(running_managed()),
            Action::KeepReady
        );
    }

    #[test]
    fn replaces_a_gateway_from_another_version() {
        let old_daemon = GatewayObservation {
            gateway_version: Some("0.9.0"),
            ..running_daemon()
        };
        assert_eq!(gateway_lifecycle_action(old_daemon), Action::RestartDaemon);
        let unknown_managed = GatewayObservation {
            gateway_version: None,
            ..running_managed()
        };
        assert_eq!(
            gateway_lifecycle_action(unknown_managed),
            Action::RestartManaged
        );
    }

    #[test]
    fn migrates_a_daemon_or_stale_definition_when_autostart_is_enabled() {
        let daemon_with_autostart = GatewayObservation {
            daemon_running: true,
            service_loaded: false,
            ..running_managed()
        };
        assert_eq!(
            gateway_lifecycle_action(daemon_with_autostart),
            Action::RestartManaged
        );
        let stale_definition = GatewayObservation {
            service_current: false,
            ..running_managed()
        };
        assert_eq!(
            gateway_lifecycle_action(stale_definition),
            Action::RestartManaged
        );
    }

    #[test]
    fn restarts_a_loaded_but_unhealthy_managed_job() {
        let unhealthy = GatewayObservation {
            gateway_healthy: false,
            ..running_managed()
        };
        assert_eq!(gateway_lifecycle_action(unhealthy), Action::RestartManaged);
    }

    #[test]
    fn starts_through_the_mode_matching_the_autostart_choice() {
        let stopped_managed = GatewayObservation {
            gateway_healthy: false,
            service_loaded: false,
            service_current: false,
            ..running_managed()
        };
        assert_eq!(
            gateway_lifecycle_action(stopped_managed),
            Action::StartManaged
        );
        let stopped_daemon = GatewayObservation {
            gateway_healthy: false,
            daemon_running: false,
            ..running_daemon()
        };
        assert_eq!(
            gateway_lifecycle_action(stopped_daemon),
            Action::StartDaemon
        );
        let orphaned_job = GatewayObservation {
            service_loaded: true,
            ..stopped_daemon
        };
        assert_eq!(
            gateway_lifecycle_action(orphaned_job),
            Action::RestartDaemon
        );
    }
}
