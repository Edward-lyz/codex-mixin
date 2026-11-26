//! Layer 3: offline management use cases.
//!
//! Depends on `core`, `infra`, and `runtime`; orchestrates provider lifecycle,
//! client onboarding, diagnostics, reporting, and updates without any UI.

pub mod agent_versions;
pub mod agents;
pub mod client;
pub mod diagnostic;
pub mod dsh;
pub mod ech;
pub mod error;
pub mod fusion;
pub mod lifecycle;
pub mod pi_plugins;
pub mod provider;
pub mod reporting;
pub mod routing;
pub mod update;
