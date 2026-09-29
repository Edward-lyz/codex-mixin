use anyhow::Context;
use std::path::{Path, PathBuf};

pub fn install_cli_executable(source: &Path, target: &Path) -> anyhow::Result<bool> {
    if source == target
        || (target.exists() && std::fs::canonicalize(source)? == std::fs::canonicalize(target)?)
    {
        return Ok(false);
    }
    let parent = target
        .parent()
        .context("CLI installation target has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .context("CLI installation target has no valid file name")?;
    let temporary = target.with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
    std::fs::copy(source, &temporary)?;
    std::fs::set_permissions(&temporary, std::fs::metadata(source)?.permissions())?;
    if let Err(error) = std::fs::rename(&temporary, target) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(true)
}

pub fn install_cli_command() -> anyhow::Result<Option<PathBuf>> {
    let home = super::home_dir_required()?;
    let bin = if cfg!(windows) {
        home.join(".codex-mixin/bin")
    } else {
        home.join(".local/bin")
    };
    let target = bin.join(if cfg!(windows) {
        "codex-mixin.exe"
    } else {
        "codex-mixin"
    });
    let source = std::env::current_exe()?;
    install_cli_executable(&source, &target).map(|installed| installed.then_some(target))
}

/// Resolve the durable executable used by installed client integrations.
/// A bundled macOS install survives replacement of a temporary CLI download.
pub fn installed_cli_executable() -> anyhow::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let bundled = PathBuf::from("/Applications/Codex Mixin.app/Contents/Resources/codex-mixin");
        if bundled.is_file() {
            return Ok(bundled);
        }
    }
    std::env::current_exe().context("resolve codex-mixin executable")
}

pub fn shell_quote(value: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
