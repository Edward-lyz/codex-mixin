//! Locating the Codex CLI executable, installing it on demand.

use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

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
    #[cfg(windows)]
    anyhow::bail!(
        "automatic Codex CLI installation is not available on Windows; install Codex and set CODEX_CLI_PATH if needed"
    );

    #[cfg(not(windows))]
    {
        use std::process::Command as ProcessCommand;

        use anyhow::Context;

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
        PathBuf::from("C:/Program Files/OpenAI/Codex/codex.exe"),
    ] {
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let local_bin_codex = PathBuf::from(home).join(".local/bin").join("codex");
        if local_bin_codex.is_file() {
            return Ok(local_bin_codex);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidates = if cfg!(windows) {
                ["codex.exe", "codex.cmd", "codex.bat", "codex.ps1"]
                    .into_iter()
                    .map(|name| directory.join(name))
                    .collect::<Vec<_>>()
            } else {
                vec![directory.join("codex")]
            };
            for candidate in candidates {
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    anyhow::bail!(
        "Codex CLI was not found; set CODEX_CLI_PATH or install Codex before installing Codex Mixin"
    )
}

pub(in crate::cli) fn codex_command(path: &Path) -> ProcessCommand {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    let mut command = match extension.as_deref() {
        Some("cmd" | "bat") => {
            // `.cmd`/`.bat` must run through cmd.exe. Pass the script path as a
            // normal argument and let Rust apply its Windows quoting: manually
            // wrapping it in quotes and adding `/s` makes cmd.exe strip the
            // outer quotes wrong and fail with "not recognized" (empty stdout).
            let mut command = ProcessCommand::new("cmd.exe");
            command.args(["/d", "/c"]).arg(path);
            command
        }
        Some("ps1") => {
            let mut command = ProcessCommand::new("powershell.exe");
            command
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(path);
            command
        }
        _ => ProcessCommand::new(path),
    };
    // Never let a background codex child (app-server/catalog/validate) pop a
    // console window when the gateway itself is windowless.
    codex_mixin::platform::hide_console(&mut command);
    command
}
