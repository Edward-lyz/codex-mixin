use codex_mixin::application::update;

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
    update::install_version(&latest, &std::env::current_exe()?).await?;
    println!("Updated codex-mixin to {latest}; restarting gateway...");
    super::service::restart(None, None, false).await
}
