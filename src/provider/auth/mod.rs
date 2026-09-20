//! Provider credential handling.
//!
//! Everything that mints, captures, or signs upstream credentials lives
//! here: AWS SigV4 request signing, the Baidu DUCX native-auth runtime with
//! its capture proxy, and environment-sourced custom headers.

pub(crate) mod aws_sigv4;
#[cfg(any(unix, windows))]
pub(crate) mod capture;
#[cfg(any(unix, windows))]
pub(crate) mod ducx;
pub(crate) mod external;
