#![forbid(unsafe_code)]

pub mod anthropic;
pub mod application;
pub mod benchmark;
pub mod catalog;
pub mod clients;
pub mod config;
pub mod error;
pub mod fusion;
mod gateway;
pub mod gateway_access;
mod images;
pub mod platform {
    use std::path::PathBuf;

    pub fn home_dir() -> PathBuf {
        // Windows tools (Flutter UI, install/uninstall scripts) key off
        // USERPROFILE, so prefer it there to keep config, runtime metadata and
        // logs in a single directory. Elsewhere HOME stays authoritative.
        let (primary, secondary) = if cfg!(windows) {
            ("USERPROFILE", "HOME")
        } else {
            ("HOME", "USERPROFILE")
        };
        std::env::var_os(primary)
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var_os(secondary).filter(|value| !value.is_empty()))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Like [`home_dir`] but fails fast when neither the primary nor the
    /// secondary home variable is set, instead of falling back to the current
    /// directory. Use this where a missing home directory must abort the
    /// operation, matching the pre-Windows fail-fast behaviour of the CLI and
    /// report-hook paths.
    pub fn home_dir_required() -> anyhow::Result<PathBuf> {
        let (primary, secondary) = if cfg!(windows) {
            ("USERPROFILE", "HOME")
        } else {
            ("HOME", "USERPROFILE")
        };
        std::env::var_os(primary)
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var_os(secondary).filter(|value| !value.is_empty()))
            .map(PathBuf::from)
            .ok_or_else(|| {
                anyhow::anyhow!("home directory is not set (neither {primary} nor {secondary})")
            })
    }

    /// Configure a command so it never flashes a console window on Windows
    /// (CREATE_NO_WINDOW). No-op elsewhere. Use for every short-lived helper
    /// process the gateway spawns in the background (tasklist, taskkill, reg,
    /// codex app-server, DUCX, rg, git); otherwise a console-subsystem child of
    /// a windowless gateway pops a visible console window.
    pub fn hide_console(command: &mut std::process::Command) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        #[cfg(not(windows))]
        let _ = command;
    }
}
pub mod protocol;
pub mod provider;
pub mod server;
mod upstream;
pub mod web_search;

pub const CODEX_MIXIN_PROVIDER: &str = "codex-mixin";
