use codex_mixin::config::{
    ProviderPreset, StoredGatewayConfig, delete_stored_config, load_stored_config,
    save_stored_config, stored_config_path,
};
use codex_mixin::web_search::WebSearchCapabilities;

use super::config_input::{first_env_value, normalize_base_url, prompt_required, trim_required};

pub(super) fn login(
    provider: Option<String>,
    key: Option<String>,
    base_url: Option<String>,
    image_generation_path: Option<String>,
    gateway_key: Option<String>,
    quota_url: Option<String>,
    quota_username: Option<String>,
) -> anyhow::Result<()> {
    let explicit_key = key.is_some();
    let existing = load_stored_config()?.unwrap_or_default();
    let provider_preset = provider
        .or(existing.provider_preset.clone())
        .or_else(|| first_env_value(&["CODEX_GATEWAY_PROVIDER"]))
        .map(|value| ProviderPreset::parse(&value))
        .transpose()?
        .unwrap_or(ProviderPreset::Custom);
    let provider_changed = existing.provider_preset.as_deref() != Some(provider_preset.as_str());
    let upstream_api_key = match key {
        Some(key) => trim_required("key", key)?,
        None => existing
            .upstream_api_key
            .clone()
            .map(Ok)
            .unwrap_or_else(|| prompt_required("upstream API key"))?,
    };
    let upstream_base_url = provider_preset.normalize_upstream_base_url(normalize_base_url(
        base_url
            .or(existing.upstream_base_url)
            .or_else(|| first_env_value(&["CODEX_GATEWAY_UPSTREAM_BASE_URL", "ANTHROPIC_BASE_URL"]))
            .or_else(|| {
                provider_preset
                    .default_base_url()
                    .map(std::borrow::ToOwned::to_owned)
            })
            .map(Ok)
            .unwrap_or_else(|| prompt_required("upstream base URL"))?,
    )?);
    let upstream_kind = provider_preset.default_upstream_kind();
    let config = StoredGatewayConfig {
        gateway_bind: existing.gateway_bind,
        provider_preset: Some(provider_preset.as_str().to_owned()),
        upstream_kind: Some(upstream_kind.as_str().to_owned()),
        upstream_base_url: Some(upstream_base_url.clone()),
        upstream_messages_path: Some(provider_preset.default_messages_path().to_owned()),
        upstream_models_path: Some(provider_preset.default_models_path().to_owned()),
        upstream_image_generation_path: resolve_image_generation_path(
            image_generation_path,
            provider_changed,
            existing.upstream_image_generation_path,
            provider_preset.default_image_generation_path(),
        ),
        upstream_api_key: Some(upstream_api_key),
        gateway_api_key: gateway_key
            .map(|key| trim_required("gateway key", key))
            .transpose()?
            .or(existing.gateway_api_key),
        quota_url: quota_url
            .map(normalize_base_url)
            .transpose()?
            .or(existing.quota_url)
            .or_else(|| provider_preset.default_quota_url(&upstream_base_url)),
        quota_username: quota_username
            .map(|username| trim_required("quota username", username))
            .transpose()?
            .or(existing.quota_username),
        fusion_profiles: existing.fusion_profiles,
    };
    let path = save_stored_config(&config)?;
    if provider_changed || explicit_key {
        WebSearchCapabilities::clear_default_cache()?;
    }
    println!("login saved: {}", path.display());
    println!("provider: {}", provider_preset.as_str());
    println!("upstream kind: {}", upstream_kind.as_str());
    println!("upstream: {upstream_base_url}");
    println!(
        "upstream image generation: {}",
        config
            .upstream_image_generation_path
            .as_deref()
            .filter(|path| !path.is_empty())
            .unwrap_or("not configured")
    );
    if config.gateway_api_key.is_some() {
        println!("gateway auth: configured");
    } else {
        println!("gateway auth: disabled");
    }
    if config.quota_url.is_some() {
        println!("quota: configured");
    } else {
        println!("quota: not configured");
    }
    println!(
        "quota username: {}",
        config.quota_username.as_deref().unwrap_or("not configured")
    );
    Ok(())
}

pub(super) fn resolve_image_generation_path(
    explicit: Option<String>,
    provider_changed: bool,
    existing: Option<String>,
    provider_default: Option<&str>,
) -> Option<String> {
    match explicit {
        // Persist an empty string as an explicit opt-out so preset defaults stay disabled.
        Some(path) if path.trim().is_empty() => Some(String::new()),
        Some(path) => Some(path.trim().to_owned()),
        None if !provider_changed => existing,
        None => provider_default.map(str::to_owned),
    }
}

pub(super) fn logout() -> anyhow::Result<()> {
    let path = stored_config_path();
    if delete_stored_config()? {
        println!("login removed: {}", path.display());
    } else {
        println!("no stored login: {}", path.display());
    }
    Ok(())
}
