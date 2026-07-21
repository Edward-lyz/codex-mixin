use super::types::{CAPABILITY_TTL, ModelWebSearchCapability};
use super::*;

pub(super) fn capability_is_fresh(capability: &ModelWebSearchCapability, now: u64) -> bool {
    capability.error.is_none()
        && now.saturating_sub(capability.probed_at) < CAPABILITY_TTL.as_secs()
}

pub(super) fn default_capability_path() -> PathBuf {
    stored_config_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("web-search-capabilities.json")
}

pub(super) fn unix_seconds() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
