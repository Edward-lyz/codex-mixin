use std::path::Path;

use anyhow::bail;

pub(super) fn is_installed() -> bool {
    false
}

pub(super) fn is_enabled() -> bool {
    false
}

pub(super) fn is_running() -> anyhow::Result<bool> {
    Ok(false)
}

pub(super) fn needs_update(_executable: &Path, _log_file: &Path) -> anyhow::Result<bool> {
    Ok(true)
}

pub(super) fn install(_executable: &Path, _log_file: &Path) -> anyhow::Result<()> {
    unsupported()
}

pub(super) fn start() -> anyhow::Result<()> {
    unsupported()
}

pub(super) fn stop() -> anyhow::Result<()> {
    unsupported()
}

pub(super) fn remove() -> anyhow::Result<()> {
    unsupported()
}

pub(super) fn set_enabled(_enabled: bool) -> anyhow::Result<()> {
    unsupported()
}

fn unsupported() -> anyhow::Result<()> {
    bail!("managed gateway startup is unsupported on this operating system")
}
