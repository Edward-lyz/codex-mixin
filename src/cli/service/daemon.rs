use std::fs;
use std::fs::OpenOptions;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use codex_mixin::config::GatewayConfig;

use super::super::runtime::{
    DaemonMetadata, RuntimeMetadata, config_fingerprint, default_log_file_path,
    delete_daemon_metadata, delete_runtime_metadata, load_daemon_metadata, load_runtime_metadata,
    pid_is_running, replacement_bind_for_outdated_runtime, save_daemon_metadata, send_signal,
};
use super::start;

const STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);
const GRACEFUL_STOP_ATTEMPTS: usize = 50;
const FORCED_STOP_ATTEMPTS: usize = 10;

pub(crate) fn start_daemon(
    mut bind: Option<SocketAddr>,
    log_file: Option<PathBuf>,
    config: &GatewayConfig,
    quiet: bool,
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
    let initial_config_fingerprint = config_fingerprint()?;
    let requested_log_file = log_file.clone().unwrap_or_else(default_log_file_path);
    if runtime_running {
        let runtime = runtime.as_ref().expect("live runtime metadata");
        if let Some(existing_bind) =
            replacement_bind_for_outdated_runtime(runtime, env!("CARGO_PKG_VERSION"))
        {
            if !quiet {
                println!(
                    "replacing gateway version {} with {} on {}",
                    runtime.version.as_deref().unwrap_or("unknown"),
                    env!("CARGO_PKG_VERSION"),
                    existing_bind
                );
            }
            stop_with_output(false, quiet)?;
            if bind.is_none() {
                bind = Some(existing_bind);
            }
        } else if let Some(daemon) = &daemon {
            let requested_bind = bind.unwrap_or(config.bind);
            if running_daemon_needs_replacement(
                runtime,
                daemon,
                requested_bind,
                &requested_log_file,
                initial_config_fingerprint,
            ) {
                if !quiet {
                    println!(
                        "restarting gateway to apply current configuration on {}",
                        runtime.bind
                    );
                }
                if bind.is_none() {
                    bind = Some(requested_bind);
                }
                stop_with_output(false, quiet)?;
            } else {
                if !quiet {
                    println!(
                        "gateway daemon already running: pid {}, bind {}",
                        runtime.pid, runtime.bind
                    );
                }
                return Ok(());
            }
        } else {
            let requested_bind = bind.unwrap_or(config.bind);
            let needs_replacement =
                replacement_bind_for_outdated_runtime(runtime, env!("CARGO_PKG_VERSION")).is_some()
                    || requested_bind != runtime.bind
                    || log_file.is_some()
                    || runtime.config_fingerprint != initial_config_fingerprint;
            if needs_replacement {
                if !quiet {
                    println!(
                        "restarting gateway to apply current configuration on {}",
                        runtime.bind
                    );
                }
                if bind.is_none() {
                    bind = Some(requested_bind);
                }
                stop_with_output(false, quiet)?;
            } else {
                if !quiet {
                    println!(
                        "gateway already running: pid {}, bind {}",
                        runtime.pid, runtime.bind
                    );
                }
                return Ok(());
            }
        }
    } else if daemon_running {
        let daemon = daemon.as_ref().expect("live daemon metadata");
        if !quiet {
            println!(
                "replacing gateway with missing runtime metadata on {}",
                daemon.bind
            );
        }
        let existing_bind = daemon.bind;
        stop_with_output(false, quiet)?;
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
    let config_fingerprint = config_fingerprint()?;
    save_daemon_metadata(&DaemonMetadata {
        pid,
        bind,
        log_file: log_file.clone(),
        started_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        config_fingerprint,
    })?;
    if !quiet {
        println!("gateway daemon started in background; terminal can be closed safely");
        println!("pid: {pid}");
        println!("bind: {bind}");
        println!("log: {}", log_file.display());
    }
    Ok(())
}

pub(crate) fn running_daemon_needs_replacement(
    runtime: &RuntimeMetadata,
    daemon: &DaemonMetadata,
    requested_bind: SocketAddr,
    requested_log_file: &Path,
    config_fingerprint: Option<u64>,
) -> bool {
    replacement_bind_for_outdated_runtime(runtime, env!("CARGO_PKG_VERSION")).is_some()
        || requested_bind != runtime.bind
        || requested_log_file != daemon.log_file
        || daemon.config_fingerprint != config_fingerprint
}

pub(crate) fn stop(force: bool) -> anyhow::Result<()> {
    stop_with_output(force, false)
}

pub(super) fn stop_with_output(force: bool, quiet: bool) -> anyhow::Result<()> {
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
        if !quiet {
            if let Some(pid) = stale_pid {
                println!("removed stale gateway metadata for pid {pid}");
            } else {
                println!("gateway is not recorded");
            }
        }
        return Ok(());
    };
    let graceful_attempts = if force { 0 } else { GRACEFUL_STOP_ATTEMPTS };
    let killed = terminate_process(pid, process_kind, quiet, graceful_attempts)?;
    delete_daemon_metadata()?;
    delete_runtime_metadata()?;
    if !quiet {
        let outcome = if killed { "killed" } else { "stopped" };
        println!("gateway {process_kind} {outcome}: pid {pid}");
    }
    Ok(())
}

fn terminate_process(
    pid: u32,
    process_kind: &str,
    quiet: bool,
    graceful_attempts: usize,
) -> anyhow::Result<bool> {
    send_signal(pid, "TERM")?;
    if wait_for_process_exit(pid, graceful_attempts)? {
        return Ok(false);
    }
    if !quiet {
        let reason = if graceful_attempts == 0 {
            "forced stop requested"
        } else {
            "did not stop within 5s"
        };
        println!("gateway {process_kind} {reason}; sending SIGKILL: pid {pid}");
    }
    send_signal(pid, "KILL")?;
    anyhow::ensure!(
        wait_for_process_exit(pid, FORCED_STOP_ATTEMPTS)?,
        "gateway {process_kind} did not exit within 1s after SIGKILL: pid {pid}"
    );
    Ok(true)
}

fn wait_for_process_exit(pid: u32, attempts: usize) -> anyhow::Result<bool> {
    for _ in 0..attempts {
        if !pid_is_running(pid)? {
            return Ok(true);
        }
        thread::sleep(STOP_POLL_INTERVAL);
    }
    Ok(!pid_is_running(pid)?)
}

pub(crate) async fn restart(
    bind: Option<SocketAddr>,
    log_file: Option<PathBuf>,
    quiet: bool,
) -> anyhow::Result<()> {
    stop_with_output(false, quiet)?;
    if quiet {
        let mut config = GatewayConfig::from_stored_config()?;
        if let Some(bind) = bind {
            config.bind = bind;
        }
        return start_daemon(bind, log_file, &config, true);
    }
    start(bind, true, log_file).await
}

pub(crate) fn logs(lines: usize, follow: bool) -> anyhow::Result<()> {
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

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};

    use super::*;

    #[cfg(unix)]
    #[test]
    fn termination_timeout_forces_a_kill() {
        let mut child = ProcessCommand::new("sh")
            .args([
                "-c",
                "trap '' TERM; printf 'ready\\n'; while :; do sleep 1; done",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready, "ready\n");
        let pid = child.id();
        let reaper = thread::spawn(move || child.wait().unwrap());

        assert!(terminate_process(pid, "test", true, 1).unwrap());
        assert!(!reaper.join().unwrap().success());
    }
}
