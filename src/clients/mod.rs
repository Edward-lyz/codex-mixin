//! Coding-client configuration adapters.
//!
//! Adapters accept explicit paths and gateway data. CLI input/output and
//! application workflow ordering stay outside this module.

pub mod claude;
pub mod claude_desktop;
pub mod codex;
pub mod dsh;
pub mod frameworks;
pub mod opencode;
pub mod pi;

/// Connection state of one agent integration, shared by every adapter.
#[derive(Clone, Debug, serde::Serialize)]
pub struct IntegrationStatus {
    pub state: &'static str,
    pub mode: Option<&'static str>,
    pub detail: &'static str,
    pub error: Option<String>,
}
