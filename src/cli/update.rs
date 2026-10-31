use anyhow::Context;
use codex_mixin::application::update;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

const CLI_VERSION_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) use codex_mixin::platform::update::release_target as cli_release_target;

pub(super) async fn run(check: bool, json: bool) -> anyhow::Result<()> {
    cli_release_target()?;
    let latest = update::latest_version().await?;
    if check {
        if json {
            println!(
                "{}",
                serde_json::json!({"current_version":env!("CARGO_PKG_VERSION"),"latest_version":latest})
            );
        } else {
            println!("Latest release: {latest}");
        }
        return Ok(());
    }
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
