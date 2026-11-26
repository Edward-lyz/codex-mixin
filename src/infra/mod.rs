//! Layer 1: local storage and external infrastructure.
//!
//! Depends only on `core`; owns configuration files, client-side configuration
//! rendering, ECH transport, Pi plugin plumbing, and web-search probing.

pub mod clients;
pub mod config;
pub(crate) mod ech;
pub(crate) mod pi_plugins;
pub mod web_search;
