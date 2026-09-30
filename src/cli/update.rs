use anyhow::Context;
use codex_mixin::application::update;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

const CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
pub(super) use codex_mixin::application::update::release_version_from_redirect;
pub(super) use codex_mixin::platform::update::release_target as cli_release_target;
#[cfg(test)]
pub(super) use codex_mixin::platform::update::replace_executable;

pub(super) async fn run() -> anyhow::Result<()> {
    cli_release_target()?;
    let latest = update::latest_version().await?;
    if latest == env!("CARGO_PKG_VERSION") {
        println!("codex-mixin {latest} is already up to date.");
        return Ok(());
    }
    super::progress_step(&format!("Downloading codex-mixin {latest}"));
    let executable = std::env::current_exe()?;
    update::install_version(&latest, &executable).await?;
    println!("Updated codex-mixin to {latest}; restarting gateway...");
    restart_updated_cli(&executable, &latest).await
}

async fn restart_updated_cli(executable: &Path, version: &str) -> anyhow::Result<()> {
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("--version")
        .kill_on_drop(true)
        .stdin(Stdio::null());
    codex_mixin::platform::prepare_background_tokio_command(&mut command);
    let output = tokio::time::timeout(CLI_VERSION_TIMEOUT, command.output())
        .await
        .context("updated CLI version check timed out")?
        .context("run updated CLI version check")?;
    anyhow::ensure!(
        output.status.success(),
        "updated CLI version check failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let reported = String::from_utf8(output.stdout).context("decode updated CLI version")?;
    anyhow::ensure!(
        reported.trim() == format!("codex-mixin {version}"),
        "updated CLI reports {}; expected codex-mixin {version}",
        reported.trim()
    );
    // The old process still embeds its old version after replacing its file.
    // Let the new CLI select the service manager and verify its own readiness.
    let mut command = tokio::process::Command::new(executable);
    command.args(["--no-tui", "service", "restart", "--managed"]);
    codex_mixin::platform::prepare_background_tokio_command(&mut command);
    let status = command
        .status()
        .await
        .context("restart gateway with updated CLI")?;
    anyhow::ensure!(
        status.success(),
        "CLI was updated, but gateway restart failed with {status}"
    );
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn updated_cli_restarts_managed() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("codex-mixin");
        let invoked = directory.path().join("invoked");
        std::fs::write(&executable, format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'codex-mixin 9.0.0'; exit 0; fi\nprintf '%s\\n' \"$@\" > {}\n",
            codex_mixin::platform::shell_quote(&invoked.to_string_lossy())
        )).unwrap();
        codex_mixin::platform::make_executable(&executable).unwrap();
        restart_updated_cli(&executable, "9.0.0").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&invoked).unwrap(),
            "--no-tui\nservice\nrestart\n--managed\n"
        );
        std::fs::remove_file(&invoked).unwrap();
        assert!(restart_updated_cli(&executable, "9.0.1").await.is_err());
        assert!(!invoked.exists());
        std::fs::write(&executable, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'codex-mixin 9.0.0'; exit 0; fi\nexit 7\n").unwrap();
        assert!(
            restart_updated_cli(&executable, "9.0.0")
                .await
                .unwrap_err()
                .to_string()
                .contains("restart failed")
        );
    }
}
