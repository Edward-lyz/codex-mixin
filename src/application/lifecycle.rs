use std::net::SocketAddr;

use crate::config::GatewayConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayLifecycleAction {
    KeepReady,
    Restart,
    StartManaged,
    StartDaemon,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayLifecycleState<'a> {
    pub gateway_running: bool,
    pub gateway_version: Option<&'a str>,
    pub daemon_running: bool,
    pub startup_service_installed: bool,
    pub startup_service_running: bool,
    pub startup_service_needs_update: bool,
    pub current_version: &'a str,
}

/// Select the lifecycle operation from the observed gateway and startup state.
/// A running old binary always restarts; managed startup also migrates a daemon
/// or stale service definition before declaring the service current.
pub fn gateway_lifecycle_action(state: GatewayLifecycleState<'_>) -> GatewayLifecycleAction {
    let unhealthy_managed_service =
        state.startup_service_installed && state.startup_service_running && !state.gateway_running;
    if unhealthy_managed_service {
        return GatewayLifecycleAction::Restart;
    }
    if state.gateway_running {
        let outdated_binary = state.gateway_version != Some(state.current_version);
        let needs_managed_migration = state.startup_service_installed
            && (state.daemon_running || state.startup_service_needs_update);
        if outdated_binary || needs_managed_migration {
            return GatewayLifecycleAction::Restart;
        }
        return GatewayLifecycleAction::KeepReady;
    }
    if state.startup_service_installed {
        GatewayLifecycleAction::StartManaged
    } else {
        GatewayLifecycleAction::StartDaemon
    }
}

/// Reload committed runtime configuration after startup integration work and
/// retain the listener address selected by the operating system.
pub fn reload_for_listener(actual_bind: SocketAddr) -> anyhow::Result<GatewayConfig> {
    let mut config = GatewayConfig::from_stored_config()?;
    config.bind = actual_bind;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::{
        GatewayLifecycleAction as Action, GatewayLifecycleState as State, gateway_lifecycle_action,
    };

    fn state() -> State<'static> {
        State {
            gateway_running: true,
            gateway_version: Some("1.0.0"),
            daemon_running: false,
            startup_service_installed: false,
            startup_service_running: false,
            startup_service_needs_update: false,
            current_version: "1.0.0",
        }
    }

    #[test]
    fn keeps_current_gateway() {
        assert_eq!(gateway_lifecycle_action(state()), Action::KeepReady);
    }

    #[test]
    fn restarts_gateway_when_version_changes() {
        assert_eq!(
            gateway_lifecycle_action(State {
                gateway_version: Some("0.9.0"),
                ..state()
            }),
            Action::Restart
        );
    }

    #[test]
    fn migrates_daemon_to_managed_startup() {
        assert_eq!(
            gateway_lifecycle_action(State {
                daemon_running: true,
                startup_service_installed: true,
                startup_service_running: true,
                ..state()
            }),
            Action::Restart
        );
    }

    #[test]
    fn restarts_a_loaded_unhealthy_managed_service() {
        assert_eq!(
            gateway_lifecycle_action(State {
                gateway_running: false,
                startup_service_installed: true,
                startup_service_running: true,
                ..state()
            }),
            Action::Restart
        );
    }

    #[test]
    fn starts_through_installed_service_when_stopped() {
        assert_eq!(
            gateway_lifecycle_action(State {
                gateway_running: false,
                startup_service_installed: true,
                ..state()
            }),
            Action::StartManaged
        );
    }
}
