//! Application use cases shared by the CLI, the macOS app, and integration
//! tests. Each use case takes concrete dependencies and returns a domain
//! result; progress reporting and user-facing text stay in the caller.

pub mod client;
pub mod diagnostic;
pub mod error;
pub mod lifecycle;
pub mod provider;
