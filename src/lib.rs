#![forbid(unsafe_code)]

pub mod anthropic;
pub mod application;
pub mod benchmark;
pub mod catalog;
pub mod clients;
pub mod config;
pub mod context;
pub mod ducx_auth_carrier;
mod ech;
pub mod error;
pub mod fusion;
mod gateway;
pub mod gateway_access;
mod images;
pub mod platform;
pub mod protocol;
pub mod provider;
pub mod server;
mod upstream;
pub mod web_search;

pub const CODEX_MIXIN_PROVIDER: &str = "codex-mixin";

/// Short session tag used by the gateway usage ledger.
///
/// The DUCX report hook runs in a separate process and has to look up turns
/// recorded by the gateway, so both sides must derive the same tag from the
/// client session id.
pub fn usage_session_tag(session_id: &str) -> String {
    gateway::turn_usage::session_tag(session_id)
}
