//! Layer 0: pure kernel and domain rules.
//!
//! Nothing here depends on configuration files, local clients, network
//! transports, or management use cases.

pub mod access;
pub mod error;
pub(crate) mod images;
pub mod protocol;
pub mod provider;
pub(crate) mod usage;
