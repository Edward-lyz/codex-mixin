//! Layer 2: online gateway data plane.
//!
//! Depends only on `core` and `infra`; owns request routing, upstream
//! transport, catalog aggregation, context compression, fusion, benchmarking,
//! and the HTTP/WS server.

pub mod benchmark;
pub mod catalog;
pub mod context;
pub mod fusion;
pub(crate) mod gateway;
pub mod server;
pub(crate) mod upstream;
