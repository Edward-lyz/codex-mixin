#![forbid(unsafe_code)]

pub mod application;
pub mod benchmark;
pub mod catalog;
pub mod clients;
pub mod config;
pub mod context;
mod ech;
pub mod error;
pub mod fusion;
mod gateway;
pub mod gateway_access;
mod images;
mod pi_plugins;
pub use codex_mixin_platform as platform;
pub mod protocol;
pub use protocol::anthropic;
pub mod provider;
pub use provider::baidu::auth_carrier as ducx_auth_carrier;
pub mod server;
mod upstream;
mod usage;
pub mod web_search;

pub const CODEX_MIXIN_PROVIDER: &str = "codex-mixin";
