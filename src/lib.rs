#![forbid(unsafe_code)]

pub mod anthropic;
pub mod benchmark;
pub mod catalog;
pub(crate) mod compaction;
pub mod config;
pub mod convert;
pub mod error;
pub mod fusion;
mod gateway;
pub mod gateway_access;
pub mod history;
mod image_generation;
pub mod model_reasoning;
pub mod openai_chat;
pub mod openai_events;
pub mod provider;

pub(crate) mod request_body;
pub mod server;
pub mod sse;
pub mod web_search;

pub const CODEX_MIXIN_PROVIDER: &str = "codex-mixin";
