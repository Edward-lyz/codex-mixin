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
        return Ok(());
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
        return command.status().await.map(|status| status.success());
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
