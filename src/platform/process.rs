use std::process::{Command, Stdio};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn prepare_background_command(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

pub fn prepare_background_tokio_command(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    #[cfg(not(windows))]
    let _ = command;
}

pub fn prepare_daemon_command(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

pub fn pid_is_running(pid: u32, expected_image_name: &str) -> std::io::Result<bool> {
    #[cfg(windows)]
    return windows_pid_is_running(pid, Some(expected_image_name));
    #[cfg(not(windows))]
    {
        let _ = expected_image_name;
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
    }
}

pub fn send_process_signal(pid: u32, signal: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        anyhow::ensure!(
            matches!(signal, "KILL" | "TERM"),
            "unsupported Windows process signal: {signal}"
        );
        let mut stopped = terminate_windows_tree(pid, signal == "KILL")?;
        if !stopped {
            stopped = terminate_windows_tree(pid, true)?;
        }
        if !stopped && windows_pid_is_running(pid, None)? {
            anyhow::bail!("failed to stop process {pid} with taskkill");
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let status = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(pid.to_string())
            .status()?;
        anyhow::ensure!(status.success(), "failed to send SIG{signal} to pid {pid}");
        Ok(())
    }
}

pub fn force_kill_process_tree(pid: u32) -> std::io::Result<bool> {
    #[cfg(windows)]
    return terminate_windows_tree(pid, true);
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
    }
}

pub async fn force_kill_process_tree_async(pid: u32) -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        let mut command = tokio::process::Command::new("taskkill");
        command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        prepare_background_tokio_command(&mut command);
        command.status().await.map(|status| status.success())
    }
    #[cfg(not(windows))]
    force_kill_process_tree(pid)
}

#[cfg(windows)]
fn terminate_windows_tree(pid: u32, force: bool) -> std::io::Result<bool> {
    let mut command = Command::new("taskkill");
    command
        .args(["/PID", &pid.to_string(), "/T"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if force {
        command.arg("/F");
    }
    prepare_background_command(&mut command);
    command.status().map(|status| status.success())
}

#[cfg(windows)]
fn windows_pid_is_running(pid: u32, expected_image_name: Option<&str>) -> std::io::Result<bool> {
    let mut command = Command::new("tasklist");
    command.args(["/FI", &format!("PID eq {pid}")]);
    if let Some(image_name) = expected_image_name {
        command.args(["/FI", &format!("IMAGENAME eq {image_name}")]);
    }
    command
        .args(["/FO", "CSV", "/NH"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    prepare_background_command(&mut command);
    let output = command.output()?;
    Ok(String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        line.split("\",\"")
            .nth(1)
            .and_then(|value| value.trim_matches('"').parse::<u32>().ok())
            == Some(pid)
    }))
}

/// Start the child in its own process group so the whole tree can be killed
/// with [`kill_process_tree`]. Windows tracks the tree through taskkill.
pub fn isolate_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = command;
}

/// Tokio counterpart of [`isolate_process_group`].
pub fn isolate_tokio_process_group(command: &mut tokio::process::Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    let _ = command;
}

/// Forcefully kill a child and every descendant it spawned. The child must
/// have been started with [`isolate_process_group`]. Returns whether the
/// tree kill was delivered; callers still reap the direct child.
pub fn kill_process_tree(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let pid = i32::try_from(pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        rustix::process::kill_process_group(pid, rustix::process::Signal::KILL)
            .map_err(std::io::Error::from)
    }
    #[cfg(windows)]
    {
        if terminate_windows_tree(pid, true)? {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "taskkill could not stop process tree {pid}"
            )))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

/// Async counterpart of [`kill_process_tree`].
pub async fn kill_process_tree_async(pid: u32) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if force_kill_process_tree_async(pid).await? {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "taskkill could not stop process tree {pid}"
            )))
        }
    }
    #[cfg(not(windows))]
    kill_process_tree(pid)
}

/// Image name of the running executable, used to guard PID reuse.
pub fn current_executable_image_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| super::files::executable_file_name("codex-mixin"))
}

/// Shutdown requests the process honours: Ctrl-C everywhere, plus SIGTERM
/// from service managers on Unix. Install before serving so a registration
/// failure surfaces immediately.
pub struct ShutdownSignal {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}

impl ShutdownSignal {
    pub fn install() -> std::io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            terminate: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?,
        })
    }

    /// Resolve when a shutdown is requested.
    pub async fn recv(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = self.terminate.recv() => {}
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Kill a child started with [`isolate_tokio_process_group`] together with
/// every descendant, then reap it. Unix signals the whole process group even
/// after the leader exited, because descendants can outlive it. Windows only
/// walks the tree while the child still runs, so a reused PID is never hit.
pub async fn terminate_isolated_tokio_child(pid: Option<u32>, child: &mut tokio::process::Child) {
    if let Some(pid) = pid {
        #[cfg(unix)]
        let _ = kill_process_tree(pid);
        #[cfg(not(unix))]
        if child.try_wait().ok().flatten().is_none() {
            let _ = kill_process_tree_async(pid).await;
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// Blocking counterpart of [`terminate_isolated_tokio_child`] for a child
/// started with [`isolate_process_group`]. Falls back to killing the direct
/// child when the tree kill cannot be delivered; does not reap.
pub fn terminate_isolated_child(child: &mut std::process::Child) -> std::io::Result<()> {
    match kill_process_tree(child.id()) {
        Ok(()) => Ok(()),
        Err(_) if child.try_wait()?.is_some() => Ok(()),
        Err(error) => child.kill().map_err(|_| error),
    }
}
