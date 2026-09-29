//! Locating the Codex CLI executable, installing it on demand.

use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::Context;
use codex_mixin::platform::{
    codex_cli_candidates, codex_cli_installer, launcher_command, path_executable_candidates,
};

pub(super) fn ensure_codex_cli_for_install() -> anyhow::Result<PathBuf> {
    match resolve_codex_cli() {
        Ok(codex_cli) => Ok(codex_cli),
        Err(missing_error) => match install_official_codex_cli() {
            Ok(codex_cli) => {
                println!("codex cli install: installed {}", codex_cli.display());
                Ok(codex_cli)
            }
            Err(install_error) => {
                anyhow::bail!("{missing_error}; automatic install also failed: {install_error:#}")
            }
        },
    }
}

fn install_official_codex_cli() -> anyhow::Result<PathBuf> {
    let mut installer = codex_cli_installer()?;
    println!(
        "codex cli install: not found; running the official installer from {} in non-interactive mode",
        installer.script_url
    );
    let status = installer
        .command
        .status()
        .context("failed to run the official Codex CLI installer")?;
    anyhow::ensure!(
        status.success(),
        "official Codex CLI installer exited with {status}"
    );
    match installer.installed_path {
        Some(path) => {
            anyhow::ensure!(
                path.is_file(),
                "official Codex CLI installer completed without creating {}",
                path.display()
            );
            Ok(path)
        }
        None => resolve_default_codex_cli().context(
            "official Codex CLI installer completed without creating a discoverable codex executable",
        ),
    }
}

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
        "Codex CLI was not found; set CODEX_CLI_PATH or install Codex before installing Codex Mixin"
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
