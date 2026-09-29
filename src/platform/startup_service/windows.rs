use std::path::Path;
use std::process::Command;

use anyhow::{Context, bail};

const TASK_NAME: &str = "Codex Mixin Gateway";

fn task_command() -> anyhow::Result<std::process::Output> {
    Command::new("schtasks.exe")
        .args(["/Query", "/TN", TASK_NAME, "/FO", "LIST"])
        .output()
        .context("query gateway scheduled task")
}

fn task_run(operation: &str) -> anyhow::Result<std::process::Output> {
    Command::new("schtasks.exe")
        .args([operation, "/TN", TASK_NAME])
        .output()
        .context("control gateway scheduled task")
}

pub(super) fn is_installed() -> bool {
    task_command().is_ok_and(|output| output.status.success())
}

pub(super) fn is_enabled() -> bool {
    Command::new("schtasks.exe")
        .args(["/Query", "/TN", TASK_NAME, "/XML"])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .to_lowercase()
                    .contains("<enabled>true</enabled>")
        })
}

pub(super) fn is_running() -> anyhow::Result<bool> {
    let output = task_command()?;
    if !output.status.success() {
        return Ok(false);
    }
    let details = String::from_utf8_lossy(&output.stdout).to_lowercase();
    Ok(details
        .lines()
        .any(|line| line.contains("status:") && line.contains("running")))
}

pub(super) fn needs_update(executable: &Path, log_file: &Path) -> anyhow::Result<bool> {
    let output = task_command()?;
    if !output.status.success() {
        return Ok(true);
    }
    let details = String::from_utf8_lossy(&output.stdout).to_lowercase();
    let expected = format!(
        "{} start --log-file {}",
        executable.display(),
        log_file.display()
    );
    Ok(!details.contains(&expected.to_lowercase()))
}

pub(super) fn install(executable: &Path, log_file: &Path) -> anyhow::Result<()> {
    let action = format!(
        "\"{}\" start --log-file \"{}\"",
        executable.display(),
        log_file.display()
    );
    let output = Command::new("schtasks.exe")
        .args([
            "/Create", "/SC", "ONLOGON", "/TN", TASK_NAME, "/TR", &action, "/F",
        ])
        .output()
        .context("install gateway scheduled task")?;
    ensure_success(output, "schtasks /Create")
}

pub(super) fn start() -> anyhow::Result<()> {
    let output = task_run("Run")?;
    ensure_success(output, "schtasks /Run")
}

pub(super) fn stop() -> anyhow::Result<()> {
    let output = task_run("End")?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    if message.contains("not running") || message.contains("cannot find") {
        return Ok(());
    }
    bail!("schtasks /End failed: {}", message.trim())
}

pub(super) fn remove() -> anyhow::Result<()> {
    stop()?;
    let output = Command::new("schtasks.exe")
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .output()
        .context("remove gateway scheduled task")?;
    if output.status.success() || String::from_utf8_lossy(&output.stderr).contains("cannot find") {
        return Ok(());
    }
    ensure_success(output, "schtasks /Delete")
}

pub(super) fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let operation = if enabled { "/Enable" } else { "/Disable" };
    let output = Command::new("schtasks.exe")
        .args(["/Change", "/TN", TASK_NAME, operation])
        .output()
        .context("change gateway scheduled task startup")?;
    ensure_success(output, "schtasks /Change")
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
