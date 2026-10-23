//! Hermetic launcher for the real `codex-mixin` binary.
//!
//! Every integration test that runs the binary must build its `Command`
//! here. The CLI manages OS-global state (startup service, daemon, client
//! configs); an unisolated child would address the developer's real gateway.
//! Callers may override any variable afterwards, but never the service id.

#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_codex-mixin");
const DEAD_PROXY: &str = "http://127.0.0.1:9";

/// Point `command` entirely at `root`: home, Codex home, gateway config,
/// runtime/daemon files, XDG/AppData dirs, a unique startup-service identity
/// and a dead outbound proxy (loopback stays reachable for fixtures).
pub fn isolate(command: &mut Command, root: &Path) {
    codex_mixin::platform::set_home_env(command, &root.join("home"));
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    let service_id = format!("test-{}-{:x}", std::process::id(), hasher.finish());
    command
        .env("CODEX_HOME", root.join("codex"))
        // DSH_HOME overrides HOME for the DSH client, so a test run inside DSH
        // would otherwise rewrite the developer's real DSH profile.
        .env("DSH_HOME", root.join("home").join(".dsh"))
        .env("CLAUDE_CONFIG_DIR", root.join("home").join(".claude"))
        .env("CODEX_GATEWAY_CONFIG", root.join("config.json"))
        .env("CODEX_GATEWAY_RUNTIME_FILE", root.join("runtime.json"))
        .env("CODEX_GATEWAY_DAEMON_FILE", root.join("daemon.json"))
        .env("CODEX_GATEWAY_SERVICE_ID", service_id)
        .env("XDG_CONFIG_HOME", root.join("xdg-config"))
        .env("XDG_DATA_HOME", root.join("xdg-data"))
        .env("XDG_STATE_HOME", root.join("xdg-state"))
        .env("XDG_CACHE_HOME", root.join("xdg-cache"))
        .env("APPDATA", root.join("appdata"))
        .env("LOCALAPPDATA", root.join("localappdata"));
    for proxy in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
        command.env(proxy, DEAD_PROXY);
    }
    for no_proxy in ["NO_PROXY", "no_proxy"] {
        command.env(no_proxy, "127.0.0.1,localhost");
    }
}

/// Isolated blocking command for the binary.
pub fn cli(root: &Path) -> Command {
    let mut command = Command::new(BINARY);
    isolate(&mut command, root);
    command
}

/// Isolated tokio command for the binary.
pub fn tokio_cli(root: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(BINARY);
    isolate(command.as_std_mut(), root);
    command
}
