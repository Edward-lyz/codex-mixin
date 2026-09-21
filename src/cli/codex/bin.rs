//! Locating the Codex CLI executable, installing it on demand.

use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

#[cfg(not(windows))]
const CODEX_CLI_INSTALL_SCRIPT_URL: &str = "https://chatgpt.com/codex/install.sh";
#[cfg(windows)]
const CODEX_CLI_INSTALL_SCRIPT_URL: &str = "https://chatgpt.com/codex/install.ps1";

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
    {
        use anyhow::Context;

        println!(
            "codex cli install: running the official Windows installer in non-interactive mode"
        );
        let status = official_windows_installer_command()
            .status()
            .context("failed to run the official Codex CLI Windows installer")?;
        anyhow::ensure!(
            status.success(),
            "official Codex CLI Windows installer exited with {status}"
        );
        resolve_default_codex_cli().context(
            "official Codex CLI Windows installer completed without creating a discoverable codex.exe",
        )
    }

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

#[cfg(windows)]
fn official_windows_installer_command() -> ProcessCommand {
    let mut command = ProcessCommand::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ])
        .arg(format!(
            "Invoke-RestMethod '{CODEX_CLI_INSTALL_SCRIPT_URL}' | Invoke-Expression"
        ))
        .env("CODEX_NON_INTERACTIVE", "true")
        .env("CODEX_INSTALLER_USE_RELEASES_OPENAI_COM", "true");
    codex_mixin::platform::prepare_background_command(&mut command);
    command
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
    default_codex_cli_candidates()
        .into_iter()
        .chain(path_codex_cli_candidates())
        .find(|path| path.is_file())
}

fn default_codex_cli_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(target_os = "macos")]
    candidates.extend([
        PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
        PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
    ]);
    #[cfg(windows)]
    candidates.extend(windows_codex_cli_candidates());
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        candidates.push(
            PathBuf::from(home)
                .join(".local/bin")
                .join(if cfg!(windows) { "codex.exe" } else { "codex" }),
        );
    }
    candidates
}

#[cfg(windows)]
fn windows_codex_cli_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(install_dir) = std::env::var_os("CODEX_INSTALL_DIR") {
        candidates.push(PathBuf::from(install_dir).join("codex.exe"));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local_app_data).join("Programs/OpenAI/Codex/bin/codex.exe"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let current = PathBuf::from(home).join(".codex/packages/standalone/current");
        candidates.push(current.join("bin/codex.exe"));
        candidates.push(current.join("codex.exe"));
    }
    if let Some(app_data) = std::env::var_os("APPDATA") {
        candidates.push(PathBuf::from(app_data).join("npm/codex.cmd"));
    }
    candidates.push(PathBuf::from("C:/Program Files/OpenAI/Codex/codex.exe"));
    candidates
}

fn path_codex_cli_candidates() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .flat_map(|directory| {
            if cfg!(windows) {
                ["codex.exe", "codex.cmd", "codex.bat", "codex.ps1"]
                    .into_iter()
                    .map(|name| directory.join(name))
                    .collect::<Vec<_>>()
            } else {
                vec![directory.join("codex")]
            }
        })
        .collect()
}

pub(in crate::cli) fn codex_command(path: &Path) -> ProcessCommand {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
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
    codex_mixin::platform::prepare_background_command(&mut command);
    command
}
