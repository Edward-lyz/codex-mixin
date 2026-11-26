//! CLI subcommands grouped by the workflow each one serves.
//!
//! Shell infrastructure (`args`, `interface`, `metadata`, `runtime`) stays in
//! the parent module. Everything that implements a `codex-mixin` command lives
//! here, so command module names never collide with library module names.

pub(crate) mod benchmark_proxy;
pub(crate) mod connect;
pub(crate) mod context;
pub(crate) mod doctor;
pub(crate) mod maintenance;
pub(crate) mod plugin;
pub(crate) mod providers;
pub(crate) mod report_hook;
pub(crate) mod service;
pub(crate) mod settings;
pub(crate) mod status;
pub(crate) mod update;
