use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, bail};

use super::definition::{
    SYSTEMD_UNIT_NAME, render_systemd_unit, systemd_active_state, systemd_enabled_state,
};
use super::{StartupServiceSpec, StartupServiceStatus};

pub(super) const SUPPORTED: bool = true;

fn unit_path() -> anyhow::Result<PathBuf> {
    let config_dir = match std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => crate::platform::home_dir_required()?.join(".config"),
    };
    Ok(config_dir.join("systemd/user").join(SYSTEMD_UNIT_NAME))
}

fn systemctl(args: &[&str]) -> anyhow::Result<Output> {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .env("SYSTEMD_COLORS", "0")
        .output()
        .context("run systemctl --user")
}

fn checked(args: &[&str]) -> anyhow::Result<()> {
    let output = systemctl(args)?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "systemctl --user {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn state(query: &str, classify: fn(&str) -> Option<bool>) -> anyhow::Result<bool> {
    let output = systemctl(&[query, SYSTEMD_UNIT_NAME])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    classify(&stdout).with_context(|| {
        format!(
            "systemctl --user {query} returned an unknown state: {} {}",
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
    })
}

pub(super) fn status(spec: &StartupServiceSpec<'_>) -> anyhow::Result<StartupServiceStatus> {
    let path = unit_path()?;
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StartupServiceStatus::default());
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let enabled = state("is-enabled", systemd_enabled_state)?;
    let loaded = state("is-active", systemd_active_state)?;
    // A unit the user disabled with systemctl means autostart is off.
    Ok(StartupServiceStatus {
        installed: enabled,
        current: enabled && content == render_systemd_unit(spec)?,
        loaded,
    })
}

pub(super) fn install(spec: &StartupServiceSpec<'_>) -> anyhow::Result<()> {
    let path = unit_path()?;
    let parent = path.parent().context("systemd unit path has no parent")?;
    std::fs::create_dir_all(parent).context("create systemd user unit directory")?;
    let temporary = path.with_extension("service.tmp");
    std::fs::write(&temporary, render_systemd_unit(spec)?).context("write gateway systemd unit")?;
    std::fs::rename(&temporary, &path).context("replace gateway systemd unit")?;
    checked(&["daemon-reload"])?;
    // Register for login without starting; starting is a separate intent.
    checked(&["enable", SYSTEMD_UNIT_NAME])
}

pub(super) fn start() -> anyhow::Result<()> {
    checked(&["start", SYSTEMD_UNIT_NAME])
}

pub(super) fn stop() -> anyhow::Result<()> {
    let path = unit_path()?;
    if !path.try_exists().context("inspect systemd user unit")? {
        return Ok(());
    }
    checked(&["stop", SYSTEMD_UNIT_NAME])
}

pub(super) fn remove() -> anyhow::Result<()> {
    let path = unit_path()?;
    if !path.try_exists().context("inspect systemd user unit")? {
        return Ok(());
    }
    checked(&["disable", "--now", SYSTEMD_UNIT_NAME])?;
    std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    checked(&["daemon-reload"])
}
