#![forbid(unsafe_code)]

pub mod anthropic;
pub mod catalog;
pub mod config;
pub mod convert;
pub mod error;
pub mod history;
mod image_generation;
pub mod model_metadata;
pub mod openai_chat;
pub mod openai_events;
pub mod server;
pub mod sse;

pub const CODEX_MIXIN_PROVIDER: &str = "codex-mixin";
