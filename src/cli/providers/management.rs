use std::collections::{HashMap, HashSet};

use codex_mixin::config::StoredGatewayConfig;
use codex_mixin::provider::{
    AWS_BEDROCK_DEFAULT_REGION, AwsSigV4AuthConfig, ProviderModel, ProviderModelSource,
    ProviderPreset, ProviderQuotaParser, aws_bedrock_aksk_provider, aws_bedrock_mantle_base_url,
    catalog_model_slug,
};

use super::{
    AddProviderOptions, UpdateProviderOptions, apply_baidu_auth_options, data_report_sibling,
    discover_models_with_output,
    discovery::{
        apply_inferred_custom_endpoint, detect_custom_provider_protocol,
        infer_custom_provider_endpoint,
    },
    ensure_has_providers, find_provider_mut, mutate_and_invalidate,
    mutate_and_invalidate_provider_capabilities, normalize_base_url, normalize_currency,
    normalize_model_ids, normalize_path, parse_header_env, parse_protocol, parse_quota_parser,
    required_config, sync_imagegen_skill, trim_required,
};

pub(crate) async fn add_provider(options: AddProviderOptions) -> anyhow::Result<()> {
    let preset = ProviderPreset::parse(options.preset.trim())?;
    let id = options.id.unwrap_or_else(|| preset.default_id().to_owned());
    let mut provider = if preset == ProviderPreset::AwsBedrock && options.key.is_none() {
        let access_key_id = trim_required(
            "AWS access key ID",
            options
                .aws_access_key_id
                .ok_or_else(|| anyhow::anyhow!("aws-bedrock requires --aws-access-key-id"))?,
        )?;
        let secret_access_key = trim_required(
            "AWS secret access key",
            options
                .aws_secret_access_key
                .ok_or_else(|| anyhow::anyhow!("aws-bedrock requires --aws-secret-access-key"))?,
        )?;
        let session_token = options
            .aws_session_token
            .map(|value| trim_required("AWS session token", value))
            .transpose()?;
        let region = trim_required(
            "AWS region",
            options
                .aws_region
                .unwrap_or_else(|| AWS_BEDROCK_DEFAULT_REGION.to_owned()),
        )?;
        aws_bedrock_aksk_provider(
            id.clone(),
            access_key_id,
            secret_access_key,
            session_token,
            region,
        )
    } else {
        anyhow::ensure!(
            options.aws_access_key_id.is_none()
                && options.aws_secret_access_key.is_none()
                && options.aws_session_token.is_none()
                && options.aws_region.is_none(),
            "AWS credential options require the aws-bedrock preset without --key"
        );
        preset.create(
            id.clone(),
            trim_required(
                "key",
                options
                    .key
                    .ok_or_else(|| anyhow::anyhow!("provider requires --key"))?,
            )?,
        )
    };
    if let Some(display_name) = options.display_name {
        provider.display_name = trim_required("display name", display_name)?;
    }
    let user_set_protocol = options.protocol.is_some();
    let user_set_api_path = options.api_path.is_some();
    let user_set_models_path = options.models_path.is_some();
    let has_static_models = !options.static_models.is_empty();
    let inferred_endpoint = if preset == ProviderPreset::Custom {
        options
            .base_url
            .as_deref()
            .map(infer_custom_provider_endpoint)
            .transpose()?
    } else {
        None
    };
    let path_explicit = inferred_endpoint
        .as_ref()
        .is_some_and(|endpoint| endpoint.path_explicit);
    if let Some(endpoint) = inferred_endpoint {
        apply_inferred_custom_endpoint(&mut provider, endpoint);
    } else if let Some(base_url) = options.base_url {
        provider.base_url = normalize_base_url(base_url)?;
    }
    if let Some(website_url) = options.website_url {
        provider.website_url = Some(normalize_base_url(website_url)?);
    }
    if preset == ProviderPreset::Custom && provider.base_url.is_empty() {
        anyhow::bail!("custom provider requires --base-url");
    }
    if let Some(protocol) = options.protocol {
        provider.protocol = parse_protocol(&protocol)?;
    }
    if let Some(api_path) = options.api_path {
        provider.api_path = normalize_path("API path", api_path)?;
    }
    if let Some(models_path) = options.models_path {
        provider.model_source = ProviderModelSource::OpenAiCompatible {
            path: normalize_path("models path", models_path)?,
        };
    }
    if !options.static_models.is_empty() {
        let models = normalize_model_ids(options.static_models)?;
        provider.model_source = ProviderModelSource::Static;
        provider.cached_models = models
            .iter()
            .map(|id| ProviderModel {
                id: id.clone(),
                ..ProviderModel::default()
            })
            .collect();
        provider.selected_models = models;
    }
    if let Some(path) = options.image_generation_path {
        provider.image_generation_path = Some(normalize_path("image generation path", path)?);
    }
    if let Some(quota_url) = options.quota_url {
        provider.quota_url = Some(normalize_base_url(quota_url)?);
    }
    let has_opencode_go_quota_fields =
        options.quota_workspace_id.is_some() || options.quota_auth_cookie.is_some();
    if let Some(username) = options.quota_username {
        provider.quota_username = Some(trim_required("quota username", username)?);
    }
    if let Some(workspace_id) = options.quota_workspace_id {
        provider.quota_workspace_id = Some(trim_required("quota workspace ID", workspace_id)?);
    }
    if let Some(auth_cookie) = options.quota_auth_cookie {
        provider.quota_auth_cookie = Some(trim_required("quota auth cookie", auth_cookie)?);
    }
    if let Some(currency) = options.quota_currency {
        provider.quota_currency = Some(normalize_currency(currency)?);
    }
    if let Some(parser) = options.quota_parser {
        provider.quota_parser = parse_quota_parser(&parser)?;
    }
    if provider.preset_id.as_deref() == Some("opencode-go") && has_opencode_go_quota_fields {
        provider.quota_parser = ProviderQuotaParser::OpenCodeGo;
        provider.quota_currency = Some("USD".to_owned());
    }
    provider.request_policy.custom_headers_from_env = parse_header_env(&options.header_env)?;
    apply_baidu_auth_options(
        &mut provider,
        options.baidu_auth_bridge.as_deref(),
        options.ducx_executable,
    )?;
    if let Some(report) = options.baidu_code_report {
        provider.request_policy.baidu_code_report = report;
    }
    if provider.request_policy.baidu_code_report
        && provider.request_policy.data_report_executable.is_none()
    {
        provider.request_policy.data_report_executable = provider
            .request_policy
            .ducx_executable
            .as_deref()
            .and_then(data_report_sibling);
    }
    provider.auxiliary_model_upstream = options.auxiliary_model_upstream.unwrap_or(false);
    let mut detected_protocol = None;
    // Baidu uses its curated protocol. Custom sites get a live protocol probe so
    // users do not have to know the path.
    if preset == ProviderPreset::Custom
        && !user_set_protocol
        && !user_set_api_path
        && !user_set_models_path
        && !path_explicit
        && !has_static_models
        && let Some(endpoint) = detect_custom_provider_protocol(&provider).await?
    {
        detected_protocol = Some(super::protocol_name(endpoint.protocol).to_owned());
        apply_inferred_custom_endpoint(&mut provider, endpoint);
    }
    provider.validate()?;
    let gateway_api_key = options
        .gateway_key
        .map(|key| trim_required("gateway key", key))
        .transpose()?;
    mutate_and_invalidate_provider_capabilities(|config| {
        if config.providers.iter().any(|provider| provider.id == id) {
            anyhow::bail!("provider already exists: {id}");
        }
        if gateway_api_key.is_some() {
            config.gateway_api_key = gateway_api_key;
        }
        if provider.auxiliary_model_upstream {
            for existing_provider in &mut config.providers {
                existing_provider.auxiliary_model_upstream = false;
            }
        }
        config.providers.push(provider);
        Ok(())
    })?;
    sync_imagegen_skill()?;
    println!("provider added: {id}");
    if let Some(protocol) = detected_protocol {
        println!("provider protocol detected: {id} ({protocol})");
    }
    discover_models_with_output(&id, false).await?;
    Ok(())
}

#[allow(clippy::cognitive_complexity)]
pub(crate) async fn update_provider(options: UpdateProviderOptions) -> anyhow::Result<()> {
    let id = options.id.clone();
    let should_refresh_capabilities = options.key.is_some()
        || options.base_url.is_some()
        || options.protocol.is_some()
        || options.api_path.is_some()
        || options.models_path.is_some()
        || !options.header_env.is_empty();
    let header_env = parse_header_env(&options.header_env)?;
    let user_set_protocol = options.protocol.is_some();
    let user_set_api_path = options.api_path.is_some();
    let user_set_models_path = options.models_path.is_some();
    let base_url_updated = options.base_url.is_some();
    let mut should_probe_protocol = false;
    mutate_and_invalidate_provider_capabilities(|config| {
        if let Some(enabled) = options.auxiliary_model_upstream {
            set_auxiliary_model_upstream(config, &id, enabled)?;
        }
        let provider = find_provider_mut(config, &id)?;
        if options.clear_key {
            provider.auth.api_key.clear();
        } else if let Some(key) = &options.key {
            provider.auth.api_key = trim_required("key", key.clone())?;
            provider.auth.aws_sigv4 = None;
        }
        apply_aws_auth_options(provider, &options)?;
        if let Some(display_name) = options.display_name {
            provider.display_name = trim_required("display name", display_name)?;
        }
        let inferred_endpoint = if provider.preset_id.as_deref() == Some("custom") {
            options
                .base_url
                .as_deref()
                .map(infer_custom_provider_endpoint)
                .transpose()?
        } else {
            None
        };
        let path_explicit = inferred_endpoint
            .as_ref()
            .is_some_and(|endpoint| endpoint.path_explicit);
        if let Some(endpoint) = inferred_endpoint {
            apply_inferred_custom_endpoint(provider, endpoint);
        } else if let Some(base_url) = options.base_url {
            provider.base_url = normalize_base_url(base_url)?;
        }
        if let Some(website_url) = options.website_url {
            provider.website_url = if website_url.trim().is_empty() {
                None
            } else {
                Some(normalize_base_url(website_url)?)
            };
        }
        if let Some(protocol) = options.protocol {
            provider.protocol = super::parse_protocol(&protocol)?;
        }
        if let Some(api_path) = options.api_path {
            provider.api_path = normalize_path("API path", api_path)?;
        }
        should_probe_protocol = provider.preset_id.as_deref() == Some("custom")
            && base_url_updated
            && !user_set_protocol
            && !user_set_api_path
            && !user_set_models_path
            && !path_explicit;
        if let Some(models_path) = options.models_path {
            provider.model_source = ProviderModelSource::OpenAiCompatible {
                path: normalize_path("models path", models_path)?,
            };
        }
        if options.clear_image_generation {
            provider.image_generation_path = None;
        } else if let Some(path) = options.image_generation_path {
            provider.image_generation_path = Some(normalize_path("image generation path", path)?);
        }
        if options.clear_quota {
            provider.quota_url = None;
            provider.quota_username = None;
            provider.quota_workspace_id = None;
            provider.quota_auth_cookie = None;
            provider.quota_currency = None;
            provider.quota_parser = match provider.preset_id.as_deref() {
                Some("deepseek") => ProviderQuotaParser::DeepSeek,
                Some("opencode-go") => ProviderQuotaParser::OpenCodeGo,
                _ => ProviderQuotaParser::Generic,
            };
        } else {
            if let Some(quota_url) = options.quota_url {
                provider.quota_url = Some(normalize_base_url(quota_url)?);
            }
            if let Some(username) = options.quota_username {
                provider.quota_username = Some(trim_required("quota username", username)?);
            }
            let has_opencode_go_quota_fields =
                options.quota_workspace_id.is_some() || options.quota_auth_cookie.is_some();
            if options.clear_quota_workspace_id {
                provider.quota_workspace_id = None;
            } else if let Some(workspace_id) = options.quota_workspace_id {
                provider.quota_workspace_id =
                    Some(trim_required("quota workspace ID", workspace_id)?);
            }
            if options.clear_quota_auth_cookie {
                provider.quota_auth_cookie = None;
            } else if let Some(auth_cookie) = options.quota_auth_cookie {
                provider.quota_auth_cookie = Some(trim_required("quota auth cookie", auth_cookie)?);
            }
            if let Some(currency) = options.quota_currency {
                provider.quota_currency = Some(normalize_currency(currency)?);
            }
            if let Some(parser) = options.quota_parser {
                provider.quota_parser = parse_quota_parser(&parser)?;
            }
            if provider.preset_id.as_deref() == Some("opencode-go") && has_opencode_go_quota_fields
            {
                provider.quota_parser = ProviderQuotaParser::OpenCodeGo;
                provider.quota_currency = Some("USD".to_owned());
            }
        }
        if options.clear_header_env {
            provider.request_policy.custom_headers_from_env.clear();
        }
        if !header_env.is_empty() {
            provider
                .request_policy
                .custom_headers_from_env
                .extend(header_env.clone());
        }
        apply_baidu_auth_options(
            provider,
            options.baidu_auth_bridge.as_deref(),
            options.ducx_executable,
        )?;
        if let Some(report) = options.baidu_code_report {
            provider.request_policy.baidu_code_report = report;
        }
        if provider.request_policy.baidu_code_report
            && provider.request_policy.data_report_executable.is_none()
        {
            provider.request_policy.data_report_executable = provider
                .request_policy
                .ducx_executable
                .as_deref()
                .and_then(data_report_sibling);
        }
        provider.validate()
    })?;
    let mut detected_protocol = None;
    if should_probe_protocol {
        let provider = required_config()?
            .providers
            .into_iter()
            .find(|provider| provider.id == id)
            .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?;
        if let Some(endpoint) = detect_custom_provider_protocol(&provider).await? {
            detected_protocol = Some(super::protocol_name(endpoint.protocol).to_owned());
            mutate_and_invalidate_provider_capabilities(|config| {
                let current = find_provider_mut(config, &id)?;
                apply_inferred_custom_endpoint(current, endpoint);
                current.validate()
            })?;
        }
    }
    sync_imagegen_skill()?;
    println!("provider updated: {id}");
    if let Some(protocol) = detected_protocol {
        println!("provider protocol detected: {id} ({protocol})");
    }
    if should_refresh_capabilities {
        let provider = required_config()?
            .providers
            .into_iter()
            .find(|provider| provider.id == id)
            .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?;
        if provider.model_source != ProviderModelSource::BaiduOneApi {
            discover_models_with_output(&id, false).await?;
        }
    }
    Ok(())
}

fn apply_aws_auth_options(
    provider: &mut codex_mixin::provider::ProviderDefinition,
    options: &UpdateProviderOptions,
) -> anyhow::Result<()> {
    let has_update = options.aws_access_key_id.is_some()
        || options.aws_secret_access_key.is_some()
        || options.aws_session_token.is_some()
        || options.aws_region.is_some()
        || options.clear_aws_session_token
        || options.clear_aws_credentials;
    if !has_update {
        return Ok(());
    }
    anyhow::ensure!(
        provider.preset_id.as_deref() == Some("aws-bedrock"),
        "AWS credential options require an aws-bedrock provider"
    );
    if options.clear_aws_credentials {
        provider.auth.aws_sigv4 = None;
        return Ok(());
    }
    let mut aws = provider
        .auth
        .aws_sigv4
        .take()
        .unwrap_or(AwsSigV4AuthConfig {
            access_key_id: String::new(),
            secret_access_key: String::new(),
            session_token: None,
            region: AWS_BEDROCK_DEFAULT_REGION.to_owned(),
            service: codex_mixin::provider::AWS_BEDROCK_MANTLE_SERVICE.to_owned(),
        });
    if let Some(value) = &options.aws_access_key_id {
        aws.access_key_id = trim_required("AWS access key ID", value.clone())?;
    }
    if let Some(value) = &options.aws_secret_access_key {
        aws.secret_access_key = trim_required("AWS secret access key", value.clone())?;
    }
    if options.clear_aws_session_token {
        aws.session_token = None;
    } else if let Some(value) = &options.aws_session_token {
        aws.session_token = Some(trim_required("AWS session token", value.clone())?);
    }
    if let Some(value) = &options.aws_region {
        aws.region = trim_required("AWS region", value.clone())?;
        if options.base_url.is_none() {
            provider.base_url = aws_bedrock_mantle_base_url(&aws.region);
        }
    }
    provider.auth.api_key.clear();
    provider.auth.aws_sigv4 = Some(aws);
    Ok(())
}

pub(crate) fn set_provider_enabled(id: &str, enabled: bool) -> anyhow::Result<()> {
    mutate_and_invalidate(|config| {
        ensure_has_providers(config)?;
        find_provider_mut(config, id)?.enabled = enabled;
        Ok(())
    })?;
    sync_imagegen_skill()?;
    println!(
        "provider {}: {id}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

pub(crate) fn remove_provider(id: &str) -> anyhow::Result<()> {
    let renames = mutate_and_invalidate_provider_capabilities(|config| {
        ensure_has_providers(config)?;
        remove_provider_from_config(config, id)
    })?;
    sync_imagegen_skill()?;
    println!("provider removed: {id}");
    for (old_id, new_id) in renames {
        println!("provider renumbered: {old_id} -> {new_id}");
    }
    Ok(())
}

pub(crate) fn reorder_providers(ids: Vec<String>) -> anyhow::Result<()> {
    let order = ids.join(", ");
    mutate_and_invalidate(|config| reorder_provider_ids(config, &ids))?;
    println!("provider order updated: {order}");
    Ok(())
}

pub(super) fn set_auxiliary_model_upstream(
    config: &mut StoredGatewayConfig,
    id: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    ensure_has_providers(config)?;
    let selected_index = config
        .providers
        .iter()
        .position(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?;
    if enabled {
        for provider in &mut config.providers {
            provider.auxiliary_model_upstream = false;
        }
    }
    config.providers[selected_index].auxiliary_model_upstream = enabled;
    Ok(())
}

pub(super) fn reorder_provider_ids(
    config: &mut StoredGatewayConfig,
    ids: &[String],
) -> anyhow::Result<()> {
    ensure_has_providers(config)?;
    anyhow::ensure!(
        ids.len() == config.providers.len(),
        "provider reorder requires all {} provider IDs in the desired order",
        config.providers.len()
    );
    let mut seen = HashSet::new();
    for id in ids {
        anyhow::ensure!(
            seen.insert(id),
            "provider reorder contains duplicate provider ID: {id}"
        );
        anyhow::ensure!(
            config.providers.iter().any(|provider| provider.id == *id),
            "unknown provider: {id}"
        );
    }
    let mut reordered = Vec::with_capacity(config.providers.len());
    let mut remaining = std::mem::take(&mut config.providers);
    for id in ids {
        let index = remaining
            .iter()
            .position(|provider| provider.id == *id)
            .ok_or_else(|| anyhow::anyhow!("unknown or duplicate provider ID: {id}"))?;
        reordered.push(remaining.remove(index));
    }
    config.providers = reordered;
    Ok(())
}

pub(super) fn remove_provider_from_config(
    config: &mut StoredGatewayConfig,
    id: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    let index = config
        .providers
        .iter()
        .position(|provider| provider.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {id}"))?;
    let removed = config.providers.remove(index);
    let Some(preset_id) = removed.preset_id else {
        return Ok(Vec::new());
    };
    let preset = ProviderPreset::parse(&preset_id)?;
    Ok(compact_generated_provider_ids(config, preset))
}

fn compact_generated_provider_ids(
    config: &mut StoredGatewayConfig,
    preset: ProviderPreset,
) -> Vec<(String, String)> {
    let base_id = preset.default_id();
    let mut generated = config
        .providers
        .iter()
        .enumerate()
        .filter_map(|(index, provider)| {
            (provider.preset_id.as_deref() == Some(preset.as_str()))
                .then(|| generated_provider_ordinal(&provider.id, base_id))
                .flatten()
                .map(|ordinal| (ordinal, index))
        })
        .collect::<Vec<_>>();
    generated.sort_unstable_by_key(|(ordinal, _)| *ordinal);

    let renames = generated
        .into_iter()
        .enumerate()
        .filter_map(|(position, (_, provider_index))| {
            let new_id = if position == 0 {
                base_id.to_owned()
            } else {
                format!("{base_id}-{}", position + 1)
            };
            let old_id = config.providers[provider_index].id.clone();
            (old_id != new_id).then_some((provider_index, old_id, new_id))
        })
        .collect::<Vec<_>>();

    let mut model_reference_renames = HashMap::new();
    for (provider_index, old_id, new_id) in &renames {
        let provider = &config.providers[*provider_index];
        for upstream_model_id in provider
            .selected_models
            .iter()
            .chain(provider.cached_models.iter().map(|model| &model.id))
        {
            model_reference_renames.insert(
                catalog_model_slug(upstream_model_id, old_id),
                catalog_model_slug(upstream_model_id, new_id),
            );
        }
    }

    for (provider_index, _, new_id) in &renames {
        config.providers[*provider_index].id.clone_from(new_id);
    }
    for profile in &mut config.fusion_profiles {
        for reference in profile
            .panel_models
            .iter_mut()
            .chain([&mut profile.judge_model, &mut profile.final_model])
        {
            if let Some(new_reference) = model_reference_renames.get(reference) {
                reference.clone_from(new_reference);
            }
        }
    }

    renames
        .into_iter()
        .map(|(_, old_id, new_id)| (old_id, new_id))
        .collect()
}

fn generated_provider_ordinal(id: &str, base_id: &str) -> Option<usize> {
    if id == base_id {
        return Some(1);
    }
    id.strip_prefix(base_id)?
        .strip_prefix('-')?
        .parse::<usize>()
        .ok()
        .filter(|ordinal| *ordinal >= 2)
}
