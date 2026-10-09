#![forbid(unsafe_code)]

pub mod anthropic;
pub mod application;
pub mod benchmark;
pub mod catalog;
pub mod clients;
pub mod config;
mod ducx_auth_carrier;
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
