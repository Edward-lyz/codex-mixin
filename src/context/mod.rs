pub mod checkpoint;
pub mod engine;
pub mod guidance;
mod store;
pub mod tools;
pub mod types;

use std::path::PathBuf;

use anyhow::{Context, Result};

pub use engine::{ContextEngine, ToolExecution};
pub(crate) use store::TurnSample;
pub use types::{
    CacheStats, ContextStats, ContextStatus, SessionStats, SessionSummary, StatsTotals, Tier,
    TierCounts, TurnView,
};

pub const DATABASE_ENV: &str = "CODEX_MIXIN_CONTEXT_DB";

/// Resolves the context database path next to the gateway configuration.
pub fn database_path() -> Result<PathBuf> {
    database_path_with(|name| std::env::var(name).ok())
}

/// Resolves the database path through a caller-provided environment lookup so
/// the server reuses its own lookup and tests can redirect the database
/// without touching the process environment.
pub fn database_path_with(env_lookup: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    if let Some(path) = env_lookup(DATABASE_ENV).filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let config_path = crate::config::stored_config_path();
    let parent = config_path
        .parent()
        .context("gateway configuration path has no parent directory")?;
    Ok(parent.join("context.db"))
}
