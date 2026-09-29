//! Where the official Codex CLI lives on each OS, how its official installer
//! runs, and how script-based launchers are executed.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Official installer for the Codex CLI on this OS.
pub struct CodexCliInstaller {
    pub script_url: &'static str,
    pub command: Command,
    /// Where the installer places the CLI when that location is fixed.
    pub installed_path: Option<PathBuf>,
}

/// Well-known Codex CLI locations, most preferred first. `PATH` entries are
/// searched separately through [`path_executable_candidates`].
pub fn codex_cli_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(target_os = "macos")]
    for bundle in ["/Applications/ChatGPT.app", "/Applications/Codex.app"] {
        candidates.extend(app_bundle_codex_cli_candidates(Path::new(bundle)));
    }
    #[cfg(windows)]
    candidates.extend(windows_codex_cli_candidates());
    if let Ok(home) = super::home_dir_required() {
        candidates.push(
            home.join(".local/bin")
                .join(super::executable_file_name("codex")),
        );
    }
    candidates
}

/// Launcher file names a `PATH` directory may contain for a command.
pub fn path_executable_candidates(directory: &Path, stem: &str) -> Vec<PathBuf> {
    if cfg!(windows) {
        ["exe", "cmd", "bat", "ps1"]
            .into_iter()
            .map(|extension| directory.join(format!("{stem}.{extension}")))
            .collect()
    } else {
        vec![directory.join(stem)]
    }
}

/// Codex CLI paths inside a desktop app bundle, current layout first.
///
/// The ChatGPT app moved the bundled CLI from `Contents/Resources/codex` to
/// `Contents/Resources/codex-cli/bin/codex` in 2026-09. Probing only the old
/// path makes the gateway fall back to `models_cache.json`, which pins a stale
/// client version and hides newer official models.
#[cfg(any(target_os = "macos", test))]
fn app_bundle_codex_cli_candidates(bundle: &Path) -> [PathBuf; 2] {
    let resources = bundle.join("Contents/Resources");
    [
        resources.join("codex-cli").join("bin").join("codex"),
        resources.join("codex"),
    ]
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
    if let Ok(home) = super::home_dir_required() {
        let current = home.join(".codex/packages/standalone/current");
        candidates.push(current.join("bin/codex.exe"));
        candidates.push(current.join("codex.exe"));
    }
    if let Some(app_data) = std::env::var_os("APPDATA") {
        candidates.push(PathBuf::from(app_data).join("npm/codex.cmd"));
    }
    candidates.push(PathBuf::from("C:/Program Files/OpenAI/Codex/codex.exe"));
    candidates
}

/// Build the non-interactive official installer invocation.
pub fn codex_cli_installer() -> anyhow::Result<CodexCliInstaller> {
    #[cfg(windows)]
    {
        const URL: &str = "https://chatgpt.com/codex/install.ps1";
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
            ])
            .arg(format!("Invoke-RestMethod '{URL}' | Invoke-Expression"))
            .env("CODEX_NON_INTERACTIVE", "true")
            .env("CODEX_INSTALLER_USE_RELEASES_OPENAI_COM", "true");
        super::prepare_background_command(&mut command);
        Ok(CodexCliInstaller {
            script_url: URL,
            command,
            installed_path: None,
        })
    }
    #[cfg(not(windows))]
    {
        use anyhow::Context;

        const URL: &str = "https://chatgpt.com/codex/install.sh";
        let bin_dir = super::home_dir_required()
            .context("a home directory is required to install Codex CLI")?
            .join(".local/bin");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("curl -fsSL {URL} | sh"))
            .env("CODEX_NON_INTERACTIVE", "true")
            .env("CODEX_INSTALLER_USE_RELEASES_OPENAI_COM", "true")
            .env("CODEX_INSTALL_DIR", bin_dir.as_os_str());
        Ok(CodexCliInstaller {
            script_url: URL,
            installed_path: Some(bin_dir.join("codex")),
            command,
        })
    }
}

/// Command that runs a launcher, dispatching script launchers through their
/// interpreter. The child never opens a console window.
pub fn launcher_command(path: &Path) -> Command {
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
            let mut command = Command::new("cmd.exe");
            command.args(["/d", "/c"]).arg(path);
            command
        }
        Some("ps1") => {
            let mut command = Command::new("powershell.exe");
            command
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(path);
            command
        }
        _ => Command::new(path),
    };
    super::prepare_background_command(&mut command);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_bundle_candidates_find_the_codex_cli_layout() {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("ChatGPT.app");
        let cli = bundle.join("Contents/Resources/codex-cli/bin/codex");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, b"").unwrap();
        let resolved = app_bundle_codex_cli_candidates(&bundle)
            .into_iter()
            .find(|path| path.is_file());
        assert_eq!(resolved, Some(cli));
    }

    #[test]
    fn app_bundle_candidates_keep_the_legacy_layout() {
        let directory = tempfile::tempdir().unwrap();
        let bundle = directory.path().join("Codex.app");
        let cli = bundle.join("Contents/Resources/codex");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, b"").unwrap();
        let resolved = app_bundle_codex_cli_candidates(&bundle)
            .into_iter()
            .find(|path| path.is_file());
        assert_eq!(resolved, Some(cli));
    }
}
