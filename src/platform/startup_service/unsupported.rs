use anyhow::bail;

use super::{StartupServiceSpec, StartupServiceStatus};

pub(super) const SUPPORTED: bool = false;

pub(super) fn status(_spec: &StartupServiceSpec<'_>) -> anyhow::Result<StartupServiceStatus> {
    Ok(StartupServiceStatus::default())
}

pub(super) fn install(_spec: &StartupServiceSpec<'_>) -> anyhow::Result<()> {
    unsupported()
}

pub(super) fn start() -> anyhow::Result<()> {
    unsupported()
}

pub(super) fn stop() -> anyhow::Result<()> {
    Ok(())
}

pub(super) fn remove() -> anyhow::Result<()> {
    Ok(())
}

fn unsupported() -> anyhow::Result<()> {
    bail!("gateway startup at login is unsupported on this operating system")
}
