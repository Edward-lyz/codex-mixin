//! Baidu OneAPI vendor specialization.
//!
//! Adapter behavior, the available-models catalog shape, and the managed DUCX
//! authentication bridge all live here. Code outside this module interacts
//! with Baidu OneAPI only through the generic `ProviderAdapter` /
//! `ProviderRuntime` interface.

pub(crate) mod adapter;
pub mod auth_carrier;
pub(crate) mod discovery;
pub(crate) mod quota;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::header::HeaderMap;

use super::{ProviderRuntime, auth};

/// Mint native authentication headers for the managed auth bridge.
///
/// The DUCX headers are minted on demand, so no carrier process stays alive
/// after the call.
pub(crate) async fn native_auth_headers(provider: &ProviderRuntime) -> anyhow::Result<HeaderMap> {
    let executable = provider
        .auth_bridge_executable()
        .map(PathBuf::from)
        .or_else(auth::ducx::default_ducx_executable)
        .context("managed auth bridge is enabled but no executable is configured")?;
    if !provider.uses_managed_auth_bridge() {
        bail!("provider has no managed native auth bridge configured")
    }
    auth::ducx::DucxRuntime::shared(executable)?
        .native_headers(Duration::from_secs(30))
        .await
}
