//! Locating the Codex CLI executable, installing it on demand.

use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use anyhow::Context;

const CODEX_CLI_INSTALL_SCRIPT_URL: &str = "https://chatgpt.com/codex/install.sh";

pub(super) fn ensure_codex_cli_for_install() -> anyhow::Result<PathBuf> {
    match resolve_codex_cli() {
        Ok(codex_cli) => Ok(codex_cli),
        Err(missing_error) => {
            println!(
                "codex cli install: not found; installing from {CODEX_CLI_INSTALL_SCRIPT_URL}"
            );
            match install_official_codex_cli() {
                Ok(codex_cli) => {
                    println!("codex cli install: installed {}", codex_cli.display());
                    Ok(codex_cli)
                }
                Err(install_error) => anyhow::bail!(
                    "{missing_error}; automatic install also failed: {install_error:#}"
                ),
            }
        }
    }
}

fn install_official_codex_cli() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is required to install Codex CLI")?;
    let bin_dir = PathBuf::from(&home).join(".local/bin");
    let installed_path = bin_dir.join("codex");
    println!("codex cli install: running the official installer in non-interactive mode");
    let status = ProcessCommand::new("sh")
        .arg("-c")
        .arg(format!("curl -fsSL {CODEX_CLI_INSTALL_SCRIPT_URL} | sh"))
        .env("CODEX_NON_INTERACTIVE", "true")
        .env("CODEX_INSTALLER_USE_RELEASES_OPENAI_COM", "true")
        .env("CODEX_INSTALL_DIR", bin_dir.as_os_str())
        .status()
        .context("failed to run the official Codex CLI installer")?;
    anyhow::ensure!(
        status.success(),
        "official Codex CLI installer exited with {status}"
    );
    anyhow::ensure!(
        installed_path.is_file(),
        "official Codex CLI installer completed without creating {}",
        installed_path.display()
    );
    Ok(installed_path)
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
    for path in [
        PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
        PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
    ] {
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let local_bin_codex = PathBuf::from(home).join(".local/bin").join("codex");
        if local_bin_codex.is_file() {
            return Ok(local_bin_codex);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join("codex");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    anyhow::bail!(
        "Codex CLI was not found; set CODEX_CLI_PATH or install Codex before installing Codex Mixin"
    )
}
