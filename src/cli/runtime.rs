use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use codex_mixin::config::stored_config_path;

use super::atomic_file::write_atomic_if_changed;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct DaemonMetadata {
    pub(super) pid: u32,
    pub(super) bind: SocketAddr,
    pub(super) log_file: PathBuf,
    pub(super) started_at: u64,
    #[serde(default)]
    pub(super) config_fingerprint: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct RuntimeMetadata {
    pub(super) pid: u32,
    pub(super) bind: SocketAddr,
    pub(super) started_at: u64,
    #[serde(default)]
    pub(super) version: Option<String>,
    #[serde(default)]
    pub(super) config_fingerprint: Option<u64>,
}

pub(super) struct RuntimeMetadataGuard {
    pub(super) pid: u32,
}

impl Drop for RuntimeMetadataGuard {
    fn drop(&mut self) {
        if load_runtime_metadata()
            .ok()
            .flatten()
            .is_some_and(|metadata| metadata.pid == self.pid)
        {
            let _ = delete_runtime_metadata();
        }
    }
}

pub(super) fn state_dir() -> PathBuf {
    stored_config_path()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(super) fn daemon_metadata_path() -> PathBuf {
    std::env::var("CODEX_GATEWAY_DAEMON_FILE")
        .ok()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir().join("daemon.json"))
}

pub(super) fn runtime_metadata_path() -> PathBuf {
    std::env::var("CODEX_GATEWAY_RUNTIME_FILE")
        .ok()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir().join("runtime.json"))
}

pub(super) fn default_log_file_path() -> PathBuf {
    std::env::var("CODEX_GATEWAY_LOG_FILE")
        .ok()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir().join("gateway.log"))
}

pub(super) fn default_report_hook_log_path() -> PathBuf {
    std::env::var("CODEX_REPORT_HOOK_LOG_FILE")
        .ok()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state_dir().join("report-hook.log"))
}

pub(super) fn load_daemon_metadata() -> anyhow::Result<Option<DaemonMetadata>> {
    let path = daemon_metadata_path();
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

pub(super) fn load_runtime_metadata() -> anyhow::Result<Option<RuntimeMetadata>> {
    let path = runtime_metadata_path();
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

/// The bind client installs should target: a live gateway's actual bind wins
/// over the configured one, so clients keep pointing at the running instance.
pub(super) fn effective_gateway_bind(
    config: &codex_mixin::config::GatewayConfig,
) -> anyhow::Result<SocketAddr> {
    Ok(match load_runtime_metadata()? {
        Some(runtime) if pid_is_running(runtime.pid)? => runtime.bind,
        _ => config.bind,
    })
}

pub(super) fn replacement_bind_for_outdated_runtime(
    runtime: &RuntimeMetadata,
    current_version: &str,
) -> Option<SocketAddr> {
    (runtime.version.as_deref() != Some(current_version)).then_some(runtime.bind)
}

pub(super) fn config_fingerprint() -> anyhow::Result<Option<u64>> {
    let path = stored_config_path();
    if !path.exists() {
        return Ok(None);
    }
    let modified = fs::metadata(&path)?.modified()?;
    let nanos = modified.duration_since(UNIX_EPOCH)?.as_nanos();
    let nanos = u64::try_from(nanos)
        .map_err(|_| anyhow::anyhow!("config modification time is outside the supported range"))?;
    Ok(Some(nanos))
}

pub(super) fn save_daemon_metadata(metadata: &DaemonMetadata) -> anyhow::Result<()> {
    let path = daemon_metadata_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(metadata)?)?;
    Ok(())
}

pub(super) fn save_runtime_metadata(metadata: &RuntimeMetadata) -> anyhow::Result<()> {
    let path = runtime_metadata_path();
    write_atomic_if_changed(&path, &serde_json::to_vec_pretty(metadata)?)?;
    Ok(())
}

pub(super) fn delete_daemon_metadata() -> anyhow::Result<()> {
    let path = daemon_metadata_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub(super) fn delete_runtime_metadata() -> anyhow::Result<()> {
    let path = runtime_metadata_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub(super) fn pid_is_running(pid: u32) -> anyhow::Result<bool> {
    let status = ProcessCommand::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(status.success())
}

pub(super) fn send_signal(pid: u32, signal: &str) -> anyhow::Result<()> {
    let status = ProcessCommand::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()?;
    if !status.success() {
        anyhow::bail!("failed to send SIG{signal} to pid {pid}");
    }
    Ok(())
}
