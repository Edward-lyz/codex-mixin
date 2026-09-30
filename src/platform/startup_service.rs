//! Per-user OS startup service for the gateway.
//!
//! A service definition exists exactly when the user enabled gateway startup
//! at login: launchd LaunchAgent on macOS, an enabled systemd user unit on
//! Linux, and a current-user logon task on Windows. Starting or stopping the
//! gateway never adds or removes the definition, so the user's autostart
//! choice survives restarts, upgrades, and definition migrations.
//!
//! All functions block on OS tools; async callers must use `spawn_blocking`.

// Each adapter uses its own subset; every renderer is tested on every host.
#[allow(dead_code)]
mod definition;

pub use definition::StartupServiceSpec;

#[cfg(target_os = "macos")]
#[path = "startup_service/macos.rs"]
mod implementation;
#[cfg(target_os = "linux")]
#[path = "startup_service/linux.rs"]
mod implementation;
#[cfg(windows)]
#[path = "startup_service/windows.rs"]
mod implementation;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
#[path = "startup_service/unsupported.rs"]
mod implementation;

/// Observed state of the gateway startup service.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StartupServiceStatus {
    /// A definition is installed, i.e. gateway autostart is enabled.
    pub installed: bool,
    /// The installed definition matches what this executable would install.
    pub current: bool,
    /// The service manager currently owns a loaded or running gateway job.
    pub loaded: bool,
}

/// Whether this platform's service manager supervises the gateway process
/// and can start it on demand.
pub fn startup_service_supported() -> bool {
    implementation::SUPPORTED
}

pub fn startup_service_status(
    spec: &StartupServiceSpec<'_>,
) -> anyhow::Result<StartupServiceStatus> {
    implementation::status(spec)
}

/// Write or replace the definition and register it for startup at login.
/// Does not start the gateway.
pub fn install_startup_service(spec: &StartupServiceSpec<'_>) -> anyhow::Result<()> {
    implementation::install(spec)
}

/// Start the gateway through the installed service definition.
pub fn start_startup_service() -> anyhow::Result<()> {
    implementation::start()
}

/// Stop a gateway owned by the service manager, keeping the definition.
pub fn stop_startup_service() -> anyhow::Result<()> {
    implementation::stop()
}

/// Stop the managed job and delete the definition.
pub fn remove_startup_service() -> anyhow::Result<()> {
    implementation::remove()
}
