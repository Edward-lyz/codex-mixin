use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_yaml::{Mapping, Value};

use codex_mixin::config::GatewayConfig;
use codex_mixin::gateway_access::GatewayClient;

use super::official_models::{selected_catalog_models, selected_official_models};
use super::runtime::effective_gateway_bind;

pub(in crate::cli) const DSH_PROVIDER_ID: &str = "codex-mixin";

pub(in crate::cli) fn default_dsh_home() -> PathBuf {
    std::env::var_os("DSH_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| codex_mixin::platform::home_dir().join(".dsh"))
}

fn resolve_dsh_home(dsh_home: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    std::path::absolute(dsh_home.unwrap_or_else(default_dsh_home)).map_err(Into::into)
}

pub(in crate::cli) fn dsh_is_installed() -> anyhow::Result<bool> {
    codex_mixin::clients::dsh::is_managed(&resolve_dsh_home(None)?)
}

pub(in crate::cli) fn install_dsh(dsh_home: Option<PathBuf>) -> anyhow::Result<()> {
    let client = codex_mixin::gateway_access::GatewayClient::Dsh;
    codex_mixin::application::client::install_with_client_key(client, || {
        let gateway_config = GatewayConfig::from_stored_config()?;
        let official_models = selected_official_models(&gateway_config)?;
        let bind = effective_gateway_bind(&gateway_config)?;
        install_dsh_with_models(dsh_home, &gateway_config, &official_models, bind, true).map(|_| ())
    })
}

fn install_dsh_with_models(
    dsh_home: Option<PathBuf>,
    gateway_config: &GatewayConfig,
    official_models: &[codex_mixin::provider::ProviderModel],
    bind: std::net::SocketAddr,
    announce: bool,
) -> anyhow::Result<bool> {
    let dsh_home = resolve_dsh_home(dsh_home)?;
    let models = collect_dsh_models(gateway_config, official_models);
    anyhow::ensure!(
        !models.is_empty(),
        "no enabled upstream models are available; refresh or select models before installing to DSH"
    );
    let credential_value = gateway_config.require_client_key(GatewayClient::Dsh)?;
    let settings_path = dsh_home.join("settings.yaml");
    let changed = codex_mixin::clients::dsh::install(&dsh_home, bind, models, &credential_value)?;

    if announce {
        println!("DSH settings updated: {}", settings_path.display());
        println!("DSH provider: {DSH_PROVIDER_ID}");
        println!("DSH base URL: http://{bind}/v1");
        println!("reload required: restart DSH or start a new DSH session");
    }
    Ok(changed)
}

pub(in crate::cli) fn uninstall_dsh(dsh_home: Option<PathBuf>) -> anyhow::Result<()> {
    let dsh_home = resolve_dsh_home(dsh_home)?;
    let settings_path = dsh_home.join("settings.yaml");
    codex_mixin::clients::dsh::uninstall(&dsh_home)?;

    println!("DSH settings restored: {}", settings_path.display());
    println!("DSH provider removed: {DSH_PROVIDER_ID}");
    println!("reload required: restart DSH or start a new DSH session");
    Ok(())
}

fn collect_dsh_models(
    config: &GatewayConfig,
    official_models: &[codex_mixin::provider::ProviderModel],
) -> Vec<Value> {
    let catalog = selected_catalog_models(config, official_models);
    let mut seen: HashSet<String> = catalog.iter().map(|(id, _, _)| id.clone()).collect();
    let mut models = Vec::new();
    for (id, name, model) in catalog {
        let mut entry = Mapping::new();
        entry.insert(Value::String("id".to_owned()), Value::String(id));
        entry.insert(Value::String("name".to_owned()), Value::String(name));
        if model.supports_thinking != Some(false) {
            entry.insert(
                Value::String("reasoningEfforts".to_owned()),
                dsh_reasoning_efforts(),
            );
        }
        if let Some(context_window) = model.context_window {
            entry.insert(
                Value::String("contextWindow".to_owned()),
                Value::Number(context_window.into()),
            );
        }
        if model.supports_image == Some(true) {
            entry.insert(
                Value::String("input".to_owned()),
                Value::Sequence(vec![
                    Value::String("text".to_owned()),
                    Value::String("image".to_owned()),
                ]),
            );
        }
        models.push(Value::Mapping(entry));
    }
    for profile in &config.fusion_profiles {
        let id = profile.model_slug();
        if !seen.insert(id.clone()) {
            continue;
        }
        let mut entry = Mapping::new();
        entry.insert(Value::String("id".to_owned()), Value::String(id));
        entry.insert(
            Value::String("name".to_owned()),
            Value::String(format!(
                "Fusion ({}): {} -> {}",
                profile.id,
                profile.panel_models.join("+"),
                profile.judge_model
            )),
        );
        models.push(Value::Mapping(entry));
    }
    models
}

fn dsh_reasoning_efforts() -> Value {
    Value::Mapping(
        [
            ("off", Value::Null),
            ("minimal", Value::String("low".to_owned())),
            ("low", Value::String("low".to_owned())),
            ("medium", Value::String("medium".to_owned())),
            ("high", Value::String("high".to_owned())),
            ("xhigh", Value::String("max".to_owned())),
            ("max", Value::String("max".to_owned())),
        ]
        .into_iter()
        .map(|(effort, value)| (Value::String(effort.to_owned()), value))
        .collect(),
    )
}

pub(in crate::cli) fn sync_installed_dsh_client_key() -> anyhow::Result<()> {
    let dsh_home = resolve_dsh_home(None)?;
    codex_mixin::application::client::sync_managed_client_key(
        GatewayClient::Dsh,
        || codex_mixin::clients::dsh::is_managed(&dsh_home),
        |key| codex_mixin::clients::dsh::sync_client_key(&dsh_home, key),
    )?;
    Ok(())
}

/// Re-render the managed DSH provider profile from the current provider
/// configuration. A missing or unmanaged DSH home is left untouched.
pub(in crate::cli) fn sync_installed_dsh_models() -> anyhow::Result<bool> {
    let gateway_config = GatewayConfig::from_stored_config()?;
    let official_models = selected_official_models(&gateway_config)?;
    let bind = effective_gateway_bind(&gateway_config)?;
    sync_dsh_models(None, &gateway_config, &official_models, bind)
}

fn sync_dsh_models(
    dsh_home: Option<PathBuf>,
    gateway_config: &GatewayConfig,
    official_models: &[codex_mixin::provider::ProviderModel],
    bind: std::net::SocketAddr,
) -> anyhow::Result<bool> {
    let dsh_home = resolve_dsh_home(dsh_home)?;
    if !dsh_provider_is_managed(&dsh_home)? {
        return Ok(false);
    }
    install_dsh_with_models(Some(dsh_home), gateway_config, official_models, bind, false)
}

fn dsh_provider_is_managed(dsh_home: &Path) -> anyhow::Result<bool> {
    codex_mixin::clients::dsh::is_managed(dsh_home)
}
