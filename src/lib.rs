#![forbid(unsafe_code)]

pub mod anthropic;
pub mod benchmark;
pub mod catalog;
pub mod config;
pub mod error;
pub mod fusion;
mod gateway;
pub mod gateway_access;
pub mod history;
mod images;
pub mod protocol;
pub mod provider;

pub mod server;
pub mod web_search;

pub const CODEX_MIXIN_PROVIDER: &str = "codex-mixin";
