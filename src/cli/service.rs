use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use codex_mixin::config::{GatewayConfig, load_stored_config, save_stored_config};
use codex_mixin::server::{AppState, serve_on_listener};
use codex_mixin::web_search::WebSearchCapabilities;

use super::codex::{
    refresh_managed_codex_catalog_with_capabilities, refresh_managed_official_codex_catalog,
    resolve_codex_config_path, sync_managed_codex_gateway_base_url,
};
use super::runtime::*;

pub(super) const CODEX_CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
pub(super) const OFFICIAL_CODEX_CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
pub(super) const GATEWAY_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

pub(super) fn init_tracing(log_file: Option<&Path>) -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::Level::INFO.into())
        .from_env_lossy();
    if let Some(log_file) = log_file {
        rotate_gateway_log_if_needed(log_file, GATEWAY_LOG_MAX_BYTES)?;
        if let Some(parent) = log_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)?;
        #[cfg(unix)]
        fs::set_permissions(log_file, fs::Permissions::from_mode(0o600))?;
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_env_filter(filter)
            .with_writer(Mutex::new(file))
            .try_init()
            .map_err(|error| anyhow::anyhow!("failed to install tracing subscriber: {error}"))?;
    } else {
        tracing_subscriber::fmt()
            .with_writer(io::stderr)
            .with_env_filter(filter)
            .try_init()
            .map_err(|error| anyhow::anyhow!("failed to install tracing subscriber: {error}"))?;
    }
    Ok(())
}

pub(super) fn rotate_gateway_log_if_needed(path: &Path, max_bytes: u64) -> anyhow::Result<()> {
    if !path.exists() || fs::metadata(path)?.len() < max_bytes {
        return Ok(());
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let mut backup_name = path.as_os_str().to_os_string();
    backup_name.push(".1");
    let backup = PathBuf::from(backup_name);
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    fs::rename(path, backup)?;
    Ok(())
}

pub(super) fn persist_gateway_bind(bind: SocketAddr) -> anyhow::Result<bool> {
    let Some(mut stored) = load_stored_config()? else {
        return Ok(false);
    };
    let bind = bind.to_string();
    if stored.gateway_bind.as_deref() == Some(&bind) {
        return Ok(false);
    }
    stored.gateway_bind = Some(bind);
    save_stored_config(&stored)?;
    Ok(true)
}

pub(super) async fn bind_gateway_listener(
    bind: SocketAddr,
    automatic_bind: bool,
) -> anyhow::Result<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind(bind).await {
        Ok(listener) => Ok(listener),
        Err(err)
            if automatic_bind
                && bind.ip().is_loopback()
                && err.kind() == io::ErrorKind::AddrInUse =>
        {
            Ok(tokio::net::TcpListener::bind(SocketAddr::new(bind.ip(), 0)).await?)
        }
        Err(err) => Err(err.into()),
    }
}

pub(super) async fn start(
    bind: Option<SocketAddr>,
    daemon: bool,
    log_file: Option<PathBuf>,
) -> anyhow::Result<()> {
    let mut config = GatewayConfig::from_env()?;
    let automatic_bind = bind.is_none()
        && std::env::var("CODEX_GATEWAY_BIND")
            .ok()
            .is_none_or(|value| value.is_empty());
    if let Some(bind) = bind {
        config.bind = bind;
    }
    if daemon {
        return start_daemon(bind, log_file);
    }
    if let Some(runtime) = load_runtime_metadata()? {
        if pid_is_running(runtime.pid)? {
            anyhow::bail!(
                "gateway already running: pid {}, bind {}",
                runtime.pid,
                runtime.bind
            );
        }
        tracing::warn!(pid = runtime.pid, "removing stale gateway runtime metadata");
        delete_runtime_metadata()?;
    }
    let listener = bind_gateway_listener(config.bind, automatic_bind).await?;
    let actual_bind = listener.local_addr()?;
    config.bind = actual_bind;
    if automatic_bind {
        persist_gateway_bind(actual_bind)?;
    }
    let config_path = resolve_codex_config_path(None)?;
    sync_managed_codex_gateway_base_url(&config_path, actual_bind)?;
    let supported_models = WebSearchCapabilities::from_default_path(&config)?.supported_model_ids();
    match refresh_managed_codex_catalog_with_capabilities(&config_path, Some(&supported_models)) {
        Ok(true) => tracing::info!("Codex model catalog refreshed"),
        Ok(false) => {}
        Err(err) => tracing::warn!(error = %err, "failed to refresh Codex model catalog"),
    }
    let official_catalog_state = AppState::new(config.clone())?;
    match refresh_managed_official_codex_catalog(
        &config_path,
        &official_catalog_state,
        Some(&supported_models),
    )
    .await
    {
        Ok(true) => tracing::info!("official Codex model catalog refreshed"),
        Ok(false) => {}
        Err(err) => tracing::warn!(error = %err, "failed to refresh official Codex model catalog"),
    }
    let refresh_config = config.clone();
    let capabilities_config_path = config_path.clone();
    let refresh_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(CODEX_CATALOG_REFRESH_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            let refresh_result = WebSearchCapabilities::from_default_path(&refresh_config)
                .map(|capabilities| capabilities.supported_model_ids())
                .and_then(|supported_models| {
                    refresh_managed_codex_catalog_with_capabilities(
                        &capabilities_config_path,
                        Some(&supported_models),
                    )
                });
            match refresh_result {
                Ok(true) => tracing::info!("Codex model catalog refreshed"),
                Ok(false) => {}
                Err(err) => tracing::warn!(error = %err, "failed to refresh Codex model catalog"),
            }
        }
    });
    let official_refresh_config = config.clone();
    let official_refresh_config_path = config_path.clone();
    let official_refresh_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(OFFICIAL_CODEX_CATALOG_REFRESH_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            let supported_models =
                match WebSearchCapabilities::from_default_path(&official_refresh_config) {
                    Ok(capabilities) => Some(capabilities.supported_model_ids()),
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to load web search capabilities");
                        None
                    }
                };
            match refresh_managed_official_codex_catalog(
                &official_refresh_config_path,
                &official_catalog_state,
                supported_models.as_ref(),
            )
            .await
            {
                Ok(true) => tracing::info!("official Codex model catalog refreshed"),
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "failed to refresh official Codex model catalog")
                }
            }
        }
    });
    let pid = std::process::id();
    save_runtime_metadata(&RuntimeMetadata {
        pid,
        bind: actual_bind,
        started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
    })?;
    let _runtime_guard = RuntimeMetadataGuard { pid };
    let result = serve_on_listener(config, listener).await;
    refresh_task.abort();
    official_refresh_task.abort();
    match &result {
        Ok(()) => tracing::info!(pid, "gateway stopped"),
        Err(error) => tracing::error!(pid, error = %error, "gateway stopped with error"),
    }
    result
}

pub(super) fn start_daemon(
    mut bind: Option<SocketAddr>,
    log_file: Option<PathBuf>,
) -> anyhow::Result<()> {
    let daemon = load_daemon_metadata()?;
    let runtime = load_runtime_metadata()?;
    let daemon_running = daemon
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
    let runtime_running = runtime
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
    if daemon_running
        && runtime_running
        && daemon.as_ref().map(|metadata| metadata.pid)
            != runtime.as_ref().map(|metadata| metadata.pid)
    {
        anyhow::bail!(
            "conflicting live gateway metadata: daemon pid {}, runtime pid {}",
            daemon.as_ref().expect("live daemon metadata").pid,
            runtime.as_ref().expect("live runtime metadata").pid
        );
    }
    if runtime_running {
        let runtime = runtime.as_ref().expect("live runtime metadata");
        if let Some(existing_bind) =
            replacement_bind_for_outdated_runtime(runtime, env!("CARGO_PKG_VERSION"))
        {
            println!(
                "replacing gateway version {} with {} on {}",
                runtime.version.as_deref().unwrap_or("unknown"),
                env!("CARGO_PKG_VERSION"),
                existing_bind
            );
            stop(false)?;
            if bind.is_none() {
                bind = Some(existing_bind);
            }
        } else if daemon_running {
            anyhow::bail!(
                "gateway daemon already running: pid {}, bind {}",
                runtime.pid,
                runtime.bind
            );
        } else {
            anyhow::bail!(
                "gateway already running: pid {}, bind {}",
                runtime.pid,
                runtime.bind
            );
        }
    } else if daemon_running {
        let daemon = daemon.as_ref().expect("live daemon metadata");
        println!(
            "replacing gateway with missing runtime metadata on {}",
            daemon.bind
        );
        let existing_bind = daemon.bind;
        stop(false)?;
        if bind.is_none() {
            bind = Some(existing_bind);
        }
    }
    delete_daemon_metadata()?;
    delete_runtime_metadata()?;
    let log_file = log_file.unwrap_or_else(default_log_file_path);
    if let Some(parent) = log_file.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)?;
    let mut command = ProcessCommand::new(std::env::current_exe()?);
    command.arg("start").arg("--log-file").arg(&log_file);
    if let Some(bind) = bind {
        command.arg("--bind").arg(bind.to_string());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn()?;
    let pid = child.id();
    let mut actual_bind = None;
    for _ in 0..50 {
        if !pid_is_running(pid)? {
            anyhow::bail!(
                "gateway daemon exited immediately; inspect log: {}",
                log_file.display()
            );
        }
        if let Some(runtime) = load_runtime_metadata()?
            && runtime.pid == pid
        {
            actual_bind = Some(runtime.bind);
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let bind = actual_bind.ok_or_else(|| {
        anyhow::anyhow!(
            "gateway daemon did not publish its endpoint within 5s; inspect log: {}",
            log_file.display()
        )
    })?;
    save_daemon_metadata(&DaemonMetadata {
        pid,
        bind,
        log_file: log_file.clone(),
        started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    })?;
    println!("gateway daemon started");
    println!("pid: {pid}");
    println!("bind: {bind}");
    println!("log: {}", log_file.display());
    Ok(())
}

pub(super) fn stop(force: bool) -> anyhow::Result<()> {
    let daemon = load_daemon_metadata()?;
    let runtime = load_runtime_metadata()?;
    let daemon_running = daemon
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
    let runtime_running = runtime
        .as_ref()
        .map(|metadata| pid_is_running(metadata.pid))
        .transpose()?
        .unwrap_or(false);
    if daemon_running
        && runtime_running
        && daemon.as_ref().map(|metadata| metadata.pid)
            != runtime.as_ref().map(|metadata| metadata.pid)
    {
        anyhow::bail!(
            "conflicting live gateway metadata: daemon pid {}, runtime pid {}",
            daemon.as_ref().expect("live daemon metadata").pid,
            runtime.as_ref().expect("live runtime metadata").pid
        );
    }
    let (pid, process_kind) = if daemon_running {
        (daemon.as_ref().expect("live daemon metadata").pid, "daemon")
    } else if runtime_running {
        (
            runtime.as_ref().expect("live runtime metadata").pid,
            "foreground",
        )
    } else {
        let stale_pid = daemon
            .as_ref()
            .map(|metadata| metadata.pid)
            .or_else(|| runtime.as_ref().map(|metadata| metadata.pid));
        delete_daemon_metadata()?;
        delete_runtime_metadata()?;
        if let Some(pid) = stale_pid {
            println!("removed stale gateway metadata for pid {pid}");
        } else {
            println!("gateway is not recorded");
        }
        return Ok(());
    };
    send_signal(pid, "TERM")?;
    for _ in 0..50 {
        if !pid_is_running(pid)? {
            delete_daemon_metadata()?;
            delete_runtime_metadata()?;
            println!("gateway {process_kind} stopped: pid {pid}");
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    if force {
        send_signal(pid, "KILL")?;
        delete_daemon_metadata()?;
        delete_runtime_metadata()?;
        println!("gateway {process_kind} killed: pid {pid}");
        return Ok(());
    }
    anyhow::bail!(
        "gateway {process_kind} did not stop within 5s: pid {pid}. Use --force to send SIGKILL"
    )
}

pub(super) async fn restart(
    bind: Option<SocketAddr>,
    log_file: Option<PathBuf>,
) -> anyhow::Result<()> {
    stop(false)?;
    start(bind, true, log_file).await
}

pub(super) fn logs(lines: usize, follow: bool) -> anyhow::Result<()> {
    let log_file = load_daemon_metadata()?
        .map(|metadata| metadata.log_file)
        .unwrap_or_else(default_log_file_path);
    if !log_file.exists() {
        anyhow::bail!("log file does not exist: {}", log_file.display());
    }
    if follow {
        let status = ProcessCommand::new("tail")
            .arg("-n")
            .arg(lines.to_string())
            .arg("-f")
            .arg(&log_file)
            .status()?;
        if !status.success() {
            anyhow::bail!("tail exited with status {status}");
        }
        return Ok(());
    }
    let content = fs::read_to_string(&log_file)?;
    let lines = content.lines().rev().take(lines).collect::<Vec<_>>();
    for line in lines.into_iter().rev() {
        println!("{line}");
    }
    Ok(())
}
