//! Provider credential handling.
//!
//! Everything that mints, captures, or signs upstream credentials lives
//! here: AWS SigV4 request signing, the Baidu DUCX native-auth runtime with
//! its capture proxy, agy OAuth token management, and environment-sourced custom headers.

pub(crate) mod agy;
pub(crate) mod aws_sigv4;
pub(crate) mod capture;
pub(crate) mod ducx;
pub(crate) mod external;
