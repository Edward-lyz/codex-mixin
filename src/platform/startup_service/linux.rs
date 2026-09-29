use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

const SERVICE_NAME: &str = "codex-mixin.service";

fn unit_path() -> anyhow::Result<PathBuf> {
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(crate::platform::home_dir_required()?.join(".config"));
    Ok(config_dir.join("systemd/user").join(SERVICE_NAME))
}

fn systemctl(args: &[&str]) -> anyhow::Result<std::process::Output> {
    Command::new("systemctl")
        .args(["--user"])
        .args(args)
        .output()
        .context("run systemctl --user")
}

pub(super) fn is_installed() -> bool {
    unit_path().is_ok_and(|path| path.is_file())
}

pub(super) fn is_enabled() -> bool {
    systemctl(&["is-enabled", SERVICE_NAME]).is_ok_and(|output| output.status.success())
}

pub(super) fn is_running() -> anyhow::Result<bool> {
    let output = systemctl(&["is-active", SERVICE_NAME])?;
    Ok(output.status.success())
}

pub(super) fn needs_update(executable: &Path, log_file: &Path) -> anyhow::Result<bool> {
    let path = unit_path()?;
    if !path.is_file() {
        return Ok(true);
    }
    let content = std::fs::read_to_string(path).context("read gateway systemd unit")?;
    let expected_exec = format!(
        "ExecStart={} start --log-file {}",
        systemd_quote(&executable.display().to_string()),
        systemd_quote(&log_file.display().to_string())
    );
    Ok(!content.lines().any(|line| line == expected_exec)
        || !content.contains("Restart=on-failure")
        || !content.contains("WantedBy=default.target"))
}

pub(super) fn install(executable: &Path, log_file: &Path) -> anyhow::Result<()> {
    let path = unit_path()?;
    let parent = path.parent().context("systemd unit path has no parent")?;
    std::fs::create_dir_all(parent).context("create systemd user unit directory")?;
    let content = format!(
        "[Unit]\nDescription=Codex Mixin Gateway\nAfter=network-online.target\n\n[Service]\nType=simple\nExecStart={} start --log-file {}\nRestart=on-failure\nRestartSec=10\nWorkingDirectory={}\nStandardOutput=append:{}\nStandardError=append:{}\n\n[Install]\nWantedBy=default.target\n",
        systemd_quote(&executable.display().to_string()),
        systemd_quote(&log_file.display().to_string()),
        systemd_quote(&crate::platform::home_dir_required()?.display().to_string()),
        systemd_quote(&log_file.display().to_string()),
        systemd_quote(&log_file.display().to_string())
    );
    std::fs::write(path, content).context("write gateway systemd unit")?;
    let output = systemctl(&["daemon-reload"])?;
    ensure_success(output, "systemctl daemon-reload")
}

pub(super) fn start() -> anyhow::Result<()> {
    let output = systemctl(&["start", SERVICE_NAME])?;
    ensure_success(output, "systemctl start")
}

pub(super) fn stop() -> anyhow::Result<()> {
    let output = systemctl(&["stop", SERVICE_NAME])?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    if message.contains("not loaded") || message.contains("not found") {
        return Ok(());
    }
    bail!("systemctl stop failed: {}", message.trim())
}

pub(super) fn remove() -> anyhow::Result<()> {
    if is_installed() {
        set_enabled(false)?;
    }
    stop()?;
    let path = unit_path()?;
    if path.exists() {
        std::fs::remove_file(path).context("remove gateway systemd unit")?;
        let output = systemctl(&["daemon-reload"])?;
        ensure_success(output, "systemctl daemon-reload")?;
    }
    Ok(())
}

pub(super) fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let operation = if enabled { "enable" } else { "disable" };
    let output = systemctl(&[operation, SERVICE_NAME])?;
    ensure_success(output, &format!("systemctl {operation}"))
}

fn ensure_success(output: std::process::Output, operation: &str) -> anyhow::Result<()> {
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{operation} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn systemd_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::systemd_quote;

    #[test]
    fn quotes_systemd_argument_values() {
        assert_eq!(systemd_quote("/a path/a\\b\"c"), "\"/a path/a\\\\b\\\"c\"");
    }
}
