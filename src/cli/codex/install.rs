use std::fs;
use std::path::PathBuf;

use clap::Args;
use toml_edit::DocumentMut;

use codex_mixin::config::GatewayConfig;
use codex_mixin::server::AppState;

use super::catalog::*;
use super::managed_config::*;
use crate::cli::metadata::load_model_metadata_resolver;
use crate::cli::official_models::{
    filter_official_catalog, load_official_account_catalog, official_template_cache_path,
};
use crate::cli::runtime::effective_gateway_bind;
use codex_mixin::application::provider::normalize_base_url;
use codex_mixin::catalog::apply_auto_review_override;
use codex_mixin::provider::auxiliary_auto_review_slug;

#[derive(Debug, Args)]
pub(in crate::cli) struct InstallCodexOptions {
    #[arg(long = "model")]
    pub(in crate::cli) requested_model: Option<String>,
    #[arg(long)]
    pub(in crate::cli) set_default: bool,
    #[arg(
        long,
        required_unless_present = "custom_only",
        conflicts_with = "custom_only"
    )]
    pub(in crate::cli) codex_oauth_proxy: bool,
    #[arg(
        long,
        required_unless_present = "codex_oauth_proxy",
        conflicts_with = "codex_oauth_proxy"
    )]
    pub(in crate::cli) custom_only: bool,
    #[arg(long = "config")]
    pub(in crate::cli) config_path: Option<PathBuf>,
    #[arg(long = "catalog")]
    pub(in crate::cli) catalog_path: Option<PathBuf>,
    #[arg(long)]
    pub(in crate::cli) base_url: Option<String>,
    #[arg(long, default_value = "live")]
    pub(in crate::cli) web_search: String,
    #[arg(long)]
    pub(in crate::cli) env_key: Option<String>,
    #[arg(long)]
    pub(in crate::cli) no_env_key: bool,
}

pub(in crate::cli) async fn install_codex(options: InstallCodexOptions) -> anyhow::Result<()> {
    let client = codex_mixin::gateway_access::GatewayClient::Codex;
    codex_mixin::application::client::install_with_client_key_async(
        client,
        install_codex_inner(options),
    )
    .await
}

async fn install_codex_inner(options: InstallCodexOptions) -> anyhow::Result<()> {
    let clears_switch_state = options.config_path.is_none();
    let InstallCodexOptions {
        requested_model,
        set_default,
        codex_oauth_proxy,
        custom_only: _,
        config_path,
        catalog_path,
        base_url,
        web_search,
        env_key,
        no_env_key,
    } = options;
    if env_key.is_some() || no_env_key {
        println!(
            "codex auth note: --env-key/--no-env-key are ignored; both install modes use Codex auth"
        );
    }
    let paths = resolve_codex_install_paths(config_path, catalog_path)?;
    println!(
        "codex install started: mode={}, config={}, catalog={}",
        if codex_oauth_proxy {
            "codex_oauth_proxy"
        } else {
            "custom_only"
        },
        paths.config.display(),
        paths.catalog.display()
    );
    super::super::progress_step("Checking local config and gateway state");
    let client_key = codex_mixin::config::ensure_gateway_client_key(
        codex_mixin::gateway_access::GatewayClient::Codex,
    )?;
    let gateway_config = GatewayConfig::from_stored_config()?;
    let state = AppState::new(gateway_config.clone())?;
    super::super::progress_step("Loading Codex config template");
    let account_catalog = if codex_oauth_proxy {
        tokio::task::spawn_blocking(load_official_account_catalog).await??
    } else {
        None
    };
    let mut template = load_codex_install_template_online(
        codex_oauth_proxy,
        &official_template_cache_path(),
        codex_mixin::catalog::official::OFFICIAL_CATALOG_URL,
        || Ok(None),
        account_catalog.as_ref(),
    )
    .await?;
    if let Some(template) = &mut template {
        filter_official_catalog(template, gateway_config.official_selected_models.as_deref())?;
    }
    super::super::progress_step("Fetching available models");
    let models = state.fetch_models().await?;
    if models.is_empty() {
        anyhow::bail!("upstream /v1/models returned no models");
    }
    super::super::progress_step("Loading model metadata");
    let metadata = load_model_metadata_resolver().await?;
    let mut catalog = generated_codex_catalog(
        &models,
        gateway_config.default_context_window,
        template.as_ref(),
        &metadata,
        codex_oauth_proxy,
    );
    apply_auto_review_override(
        &mut catalog,
        auxiliary_auto_review_slug(&gateway_config.providers).as_deref(),
    )?;
    let serialized_catalog = serde_json::to_vec_pretty(&catalog)?;
    let gateway_bind = effective_gateway_bind(&gateway_config)?;
    let gateway_base_url =
        normalize_base_url(base_url.unwrap_or_else(|| format!("http://{gateway_bind}/v1")))?;
    let provider_id = codex_mixin::clients::codex::provider_id(codex_oauth_proxy);
    let _config_lock = ManagedConfigLock::acquire(&paths.config)?;
    let raw_config = read_managed_config_for_install(&paths.config)?;
    let mut doc = if raw_config.trim().is_empty() {
        DocumentMut::new()
    } else {
        raw_config.parse::<DocumentMut>()?
    };
    let should_set_default = set_default || requested_model.is_some();
    let selected_model = if codex_oauth_proxy {
        if should_set_default {
            Some(select_codex_oauth_proxy_model(
                requested_model,
                &models,
                template.as_ref(),
                &doc,
            )?)
        } else {
            None
        }
    } else if should_set_default {
        Some(select_codex_model(requested_model, &models, &doc)?)
    } else {
        None
    };
    codex_mixin::clients::codex::upsert(
        &mut doc,
        selected_model.as_deref(),
        &paths.catalog,
        &gateway_base_url,
        &web_search,
        Some(&client_key),
        codex_oauth_proxy,
    )?;
    let serialized_config = codex_mixin::clients::codex::serialize(&doc);
    serialized_config.parse::<DocumentMut>()?;
    super::super::progress_step("Writing Codex config and model catalog");
    super::restore::install(
        &paths,
        &raw_config,
        &serialized_catalog,
        serialized_config.as_bytes(),
        codex_oauth_proxy,
        clears_switch_state,
        || {
            fs::read_to_string(&paths.config)?.parse::<DocumentMut>()?;
            let written_catalog = serde_json::from_slice(&fs::read(&paths.catalog)?)?;
            codex_mixin::catalog::official::validate_catalog(&written_catalog)
        },
    )?;
    println!("codex config updated: {}", paths.config.display());
    println!(
        "codex config backup: {}",
        managed_backup_path(&paths.config).display()
    );
    println!("model catalog written: {}", paths.catalog.display());
    println!("models installed: {}", models.len());
    println!("metadata entries loaded: {}", metadata.len());
    println!("web search capabilities: cached/provider metadata applied");
    println!("web search discovery: continues in the gateway background");
    println!("provider: {provider_id}");
    if codex_oauth_proxy {
        println!("codex oauth proxy: enabled");
        println!("codex auth: official account preserved");
    } else {
        println!(
            "custom-only model picker: enabled with a local Bedrock-shaped login placeholder (no AWS connection)"
        );
        println!("official Codex plugins, cloud tasks, and account features: unavailable");
    }
    if let Some(selected_model) = selected_model {
        println!("default model: {selected_model}");
    } else {
        println!("default model: unchanged");
    }
    println!("base_url: {gateway_base_url}");
    println!("reload required: restart Codex app; for Codex CLI, start a new session");
    Ok(())
}

pub(in crate::cli) fn uninstall_codex(
    config_path: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let clears_switch_state = config_path.is_none();
    reset_codex(config_path, catalog_path, clears_switch_state)?;
    println!("reload required: restart Codex app; for Codex CLI, start a new session");
    Ok(())
}

pub(in crate::cli) fn uninstall_codex_preserving_restore_mode(
    config_path: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    reset_codex(config_path, catalog_path, false)
}

fn reset_codex(
    config_path: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
    clear_switch: bool,
) -> anyhow::Result<()> {
    let config_path = resolve_codex_config_path(config_path)?;
    let _config_lock = ManagedConfigLock::acquire(&config_path)?;
    let (warnings, history) = super::restore::uninstall(&config_path, catalog_path, clear_switch)?;
    println!(
        "Codex config reset to the official OpenAI provider: {}",
        config_path.display()
    );
    if !config_path.with_file_name("auth.json").exists() {
        println!("login required: sign in to Codex with your OpenAI account");
    }
    println!(
        "history normalized to openai: {} session files ({} lines), {} database rows",
        history.jsonl_files_changed, history.jsonl_lines_changed, history.sqlite_rows_changed
    );
    if let Some(backup_root) = &history.backup_root {
        println!("history backup: {}", backup_root.display());
    }
    for warning in warnings.iter().chain(&history.failures) {
        eprintln!("warning: {warning}");
    }
    if !history.failures.is_empty() {
        eprintln!(
            "warning: {} history items were not normalized; disconnect again to retry",
            history.failures.len()
        );
    }
    Ok(())
}
