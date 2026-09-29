use std::path::Path;

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

pub fn startup_service_is_installed() -> bool {
    implementation::is_installed()
}

pub fn startup_service_is_enabled() -> bool {
    implementation::is_enabled()
}

pub fn startup_service_is_running() -> anyhow::Result<bool> {
    implementation::is_running()
}

pub fn startup_service_needs_update(executable: &Path, log_file: &Path) -> anyhow::Result<bool> {
    implementation::needs_update(executable, log_file)
}

pub fn install_startup_service(executable: &Path, log_file: &Path) -> anyhow::Result<()> {
    implementation::install(executable, log_file)
}

pub fn start_startup_service() -> anyhow::Result<()> {
    implementation::start()
}

pub fn stop_startup_service() -> anyhow::Result<()> {
    implementation::stop()
}

pub fn remove_startup_service() -> anyhow::Result<()> {
    implementation::remove()
}

pub fn set_startup_service_enabled(enabled: bool) -> anyhow::Result<()> {
    implementation::set_enabled(enabled)
}
