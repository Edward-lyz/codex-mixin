use codex_mixin::application::provider::{after_provider_commit, after_provider_commit_async};
use codex_mixin::config::load_stored_config;

/// Apply a saved batch as one CLI operation, so shells do not need to know
/// which services and client catalogs must change together.
pub(in crate::cli) async fn run() -> anyhow::Result<()> {
    let stored = load_stored_config()?;
    if stored
        .as_ref()
        .is_none_or(|config| config.providers.is_empty())
    {
        crate::cli::progress_step("Stopping gateway without provider configuration");
        // The provider change is already saved; a Codex stop guard failure must
        // report after_commit so shells do not treat the save as rolled back.
        after_provider_commit_async("gateway stop", crate::cli::service::stop_managed(false))
            .await?;
        return Ok(());
    }
    crate::cli::progress_step("Synchronizing reporting hooks");
    after_provider_commit(
        "reporting hook synchronization",
        crate::cli::report_hook::sync_installation,
    )?;
    crate::cli::progress_step("Applying gateway configuration");
    after_provider_commit_async("gateway restart", crate::cli::service::restart_managed()).await?;
    crate::cli::progress_step("Refreshing connected client catalogs");
    after_provider_commit_async(
        "Codex catalog synchronization",
        crate::cli::clients::codex::refresh_default_managed_codex_catalog(),
    )
    .await?;
    after_provider_commit(
        "client model synchronization",
        crate::cli::sync_installed_client_models,
    )?;
    println!("saved configuration applied");
    Ok(())
}
