//! Optional Codex CLI discovery for explicit diagnostic commands.

use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use codex_mixin::platform::{codex_cli_candidates, launcher_command, path_executable_candidates};

pub(in crate::cli) fn resolve_codex_cli() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_CLI_PATH").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
        anyhow::bail!(
            "CODEX_CLI_PATH does not point to a file: {}",
            path.display()
        );
    }
    if let Some(path) = resolve_default_codex_cli() {
        return Ok(path);
    }
    anyhow::bail!(
        "Codex CLI was not found for this optional diagnostic; set CODEX_CLI_PATH to run it"
    )
}

fn resolve_default_codex_cli() -> Option<PathBuf> {
    codex_cli_candidates()
        .into_iter()
        .chain(path_codex_cli_candidates())
        .find(|path| path.is_file())
}

fn path_codex_cli_candidates() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .flat_map(|directory| path_executable_candidates(&directory, "codex"))
        .collect()
}

/// Command that runs the Codex CLI launcher without opening a console window.
pub(in crate::cli) fn codex_command(path: &Path) -> ProcessCommand {
    launcher_command(path)
}
