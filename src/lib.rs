#![forbid(unsafe_code)]

//! Four layers arranged as a strictly lower-triangular design matrix
//! (MIT Axiomatic Design): `core` <- `infra` <- `runtime` <- `app`.
//!
//! Every leaf module is also re-exported at the crate root under its historic
//! name, so existing `crate::*` and `codex_mixin::*` paths keep resolving.

pub mod app;
pub mod core;
pub mod infra;
pub mod runtime;

pub use codex_mixin_platform as platform;

// Layer 3: offline management use cases.
pub use app as application;

// Layer 0: pure kernel and domain rules.
pub use core::access as gateway_access;
pub use core::error;
pub use core::protocol;
pub use core::protocol::anthropic;
pub use core::provider;
pub use core::provider::baidu::auth_carrier as ducx_auth_carrier;

// Layer 1: local storage and external infrastructure.
pub use infra::{clients, config, web_search};

// Layer 2: online gateway data plane.
pub use runtime::{benchmark, catalog, context, fusion, server};

// Historic crate-internal modules; their external visibility stays unchanged.
pub(crate) use core::{images, usage};
pub(crate) use infra::{ech, pi_plugins};
pub(crate) use runtime::{gateway, upstream};

pub const CODEX_MIXIN_PROVIDER: &str = "codex-mixin";
