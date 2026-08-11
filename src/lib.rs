#![forbid(unsafe_code)]

pub mod anthropic;
pub mod benchmark;
pub mod catalog;
pub mod config;
pub mod convert;
pub(crate) mod ducc;
pub mod error;
pub mod fusion;
pub mod fusion_tools;
mod gateway;
pub mod history;
mod image_generation;
pub mod model_metadata;
pub mod model_reasoning;
pub mod openai_chat;
pub mod openai_events;
pub mod provider;
pub mod provider_capabilities;
pub(crate) mod request_body;
pub mod server;
pub mod sse;
pub mod upstream;
pub mod web_search;

pub const CODEX_MIXIN_PROVIDER: &str = "codex-mixin";
