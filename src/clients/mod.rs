//! Coding-client configuration adapters.
//!
//! Adapters accept explicit paths and gateway data. CLI input/output and
//! application workflow ordering stay outside this module.

pub mod claude;
pub mod codex;
pub mod dsh;
pub mod files;
pub mod opencode;
pub mod pi;
