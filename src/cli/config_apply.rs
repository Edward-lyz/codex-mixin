use codex_mixin::application::provider::{after_provider_commit, after_provider_commit_async};
use codex_mixin::config::load_stored_config;

/// Apply a saved batch as one CLI operation, so shells do not need to know
/// which services and client catalogs must change together.
pub(super) async fn run() -> anyhow::Result<()> {
    let stored = load_stored_config()?;
    if stored
        .as_ref()
        .is_none_or(|config| config.providers.is_empty())
    {
        super::progress_step("Stopping gateway without provider configuration");
        super::service::stop_managed().await?;
        return Ok(());
    }
    super::progress_step("Synchronizing reporting hooks");
    after_provider_commit(
        "reporting hook synchronization",
        super::report_hook::sync_installation,
    )?;
    super::progress_step("Applying gateway configuration");
    after_provider_commit_async("gateway restart", super::service::restart_managed()).await?;
    super::progress_step("Refreshing connected client catalogs");
    after_provider_commit_async(
        "Codex catalog synchronization",
        super::codex::refresh_default_managed_codex_catalog(),
    )
    .await?;
    after_provider_commit(
        "client model synchronization",
        super::sync_installed_client_models,
    )?;
    println!("saved configuration applied");
    Ok(())
}
