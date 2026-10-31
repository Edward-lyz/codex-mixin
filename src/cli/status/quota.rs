use std::time::Duration;

use codex_mixin::config::load_stored_config;
use codex_mixin::provider::{ProviderQuotaParser, ProviderRegistry, quota_usage};
use futures_util::stream::{self, StreamExt};

use super::super::codex::codex_home_path;
use super::opencode_go::{
    OPENCODE_GO_DASHBOARD_BASE, fetch_opencode_go_quota_results, official_quota_rows,
};

pub(crate) async fn quota(json_output: bool, provider_filter: Option<&str>) -> anyhow::Result<()> {
    let official_enabled = super::super::providers::official_provider_is_available()?;
    let stored = load_stored_config()?.unwrap_or_default();
    let registry = ProviderRegistry::new(codex_mixin::application::provider::runtime_providers(
        stored.providers,
    )?)?;
    if registry.providers().is_empty() && !official_enabled {
        anyhow::bail!("provider configuration is missing");
    }
    validate_quota_filter(&registry, official_enabled, provider_filter)?;
    let client = codex_mixin::platform::proxy::auto_proxy(reqwest::Client::builder())
        .timeout(Duration::from_secs(10))
        .build()?;
    let provider_jobs = registry
        .providers()
        .iter()
        .enumerate()
        .filter(|(_, provider)| {
            provider.definition().enabled
                && provider_filter.is_none_or(|filter| filter == provider.id())
        })
        .map(|(index, provider)| {
            let client = client.clone();
            async move {
                let mut provider_results = Vec::new();
                if provider.definition().preset_id.as_deref() == Some("kiro") && provider.quota_url().is_none() {
                    match codex_mixin::provider::fetch_kiro_quota(provider).await {
                        Ok(rows) => for mut row in rows {
                            row["provider_id"] = serde_json::json!(provider.id());
                            row["provider_display_name"] = serde_json::json!(provider.display_name());
                            row["display_name"] = serde_json::json!(format!("{} {}", provider.display_name(), row["label"].as_str().unwrap_or("Credits")));
                            row["value"] = row["used"].clone();
                            row["stale_at"] = serde_json::Value::Null;
                            provider_results.push(row);
                        },
                        Err(error) => provider_results.push(serde_json::json!({
                            "provider_id": provider.id(), "provider_display_name": provider.display_name(),
                            "display_name": provider.display_name(), "quota_id": "quota", "label": "Quota",
                            "value": null, "error": format!("{error:#}"), "stale_at": null,
                        })),
                    }
                    return (index, provider_results);
                }
                if provider.definition().preset_id.as_deref() == Some("agy")
                    && provider.quota_url().is_none() {
                    match codex_mixin::provider::fetch_agy_quota(provider.definition()).await {
                        Ok(windows) => for window in windows {
                            provider_results.push(serde_json::json!({
                                "provider_id": provider.id(),
                                "provider_display_name": provider.display_name(),
                                "display_name": format!("{} {}", provider.display_name(), window.label),
                                "quota_id": window.id, "label": window.label,
                                "currency": null, "unit": "percent", "source": "agy_oauth",
                                "value": 100.0 - window.remaining_percent,
                                "used": 100.0 - window.remaining_percent, "limit": 100.0,
                                "remaining": window.remaining_percent, "reset_at": window.reset_at,
                                "error": null, "stale_at": null,
                            }));
                        },
                        Err(error) => provider_results.push(serde_json::json!({
                            "provider_id": provider.id(), "provider_display_name": provider.display_name(),
                            "display_name": provider.display_name(), "quota_id": "quota", "label": "Quota",
                            "currency": null, "value": null, "error": format!("{error:#}"), "stale_at": null,
                        })),
                    }
                    return (index, provider_results);
                }
                if provider.quota_parser() == ProviderQuotaParser::OpenCodeGo {
                    provider_results.extend(
                        fetch_opencode_go_quota_results(
                            &client,
                            provider.definition(),
                            OPENCODE_GO_DASHBOARD_BASE,
                        )
                        .await,
                    );
                    return (index, provider_results);
                }
                let Some(url) = provider.quota_url() else {
                    provider_results.push(serde_json::json!({
                        "provider_id": provider.id(),
                        "provider_display_name": provider.display_name(),
                        "display_name": provider.display_name(),
                        "quota_id": "quota",
                        "label": "Quota",
                        "currency": provider.quota_currency(),
                        "value": null,
                        "error": "quota endpoint is not configured",
                        "stale_at": null,
                    }));
                    return (index, provider_results);
                };
                let result = async {
                    let response = provider.apply_auth(client.get(url)).send().await?;
                    let status = response.status();
                    let body = response.text().await?;
                    if !status.is_success() {
                        anyhow::bail!("quota endpoint returned {status}: {body}");
                    }
                    let value: serde_json::Value = serde_json::from_str(&body)?;
                    let usage = quota_usage(provider.quota_parser(), &value)?;
                    Ok::<_, anyhow::Error>((usage, value))
                }
                .await;
                match result {
                    Ok((usage, raw)) => provider_results.push(serde_json::json!({
                        "provider_id": provider.id(),
                        "provider_display_name": provider.display_name(),
                        "display_name": provider.display_name(),
                        "quota_id": "quota",
                        "label": "Quota",
                        "currency": usage.currency.as_deref().or(provider.quota_currency()),
                        "value": usage.used,
                        "used": usage.used,
                        "limit": usage.limit,
                        "remaining": usage.remaining,
                        "error": null,
                        "stale_at": null,
                        "raw": raw,
                    })),
                    Err(error) => provider_results.push(serde_json::json!({
                        "provider_id": provider.id(),
                        "provider_display_name": provider.display_name(),
                        "display_name": provider.display_name(),
                        "quota_id": "quota",
                        "label": "Quota",
                        "currency": provider.quota_currency(),
                        "value": null,
                        "error": error.to_string(),
                        "stale_at": null,
                    })),
                }
                (index, provider_results)
            }
        });
    let mut completed_jobs = stream::iter(provider_jobs)
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
    completed_jobs.sort_by_key(|(index, _)| *index);
    let results = completed_jobs
        .into_iter()
        .flat_map(|(_, provider_results)| provider_results)
        .collect::<Vec<_>>();
    let mut results = results;
    if official_enabled && provider_filter.is_none_or(|filter| filter == "official") {
        results.extend(fetch_official_quota_results().await);
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    for result in results {
        let provider_label = result["display_name"]
            .as_str()
            .unwrap_or_else(|| result["provider_id"].as_str().unwrap_or("<unknown>"));
        if let Some(error) = result["error"].as_str() {
            println!("{provider_label}: error: {error}");
        } else {
            let used = &result["used"];
            let limit = &result["limit"];
            let remaining = &result["remaining"];
            let currency = result["currency"].as_str().unwrap_or("");
            if !used.is_null() && !limit.is_null() {
                println!(
                    "{provider_label}: used {used} / {limit} {currency}, remaining {remaining}"
                );
            } else if !used.is_null() {
                println!("{provider_label}: used {used} {currency}");
            } else {
                println!("{provider_label}: remaining {remaining} {currency}");
            }
        }
    }
    Ok(())
}

pub(super) fn validate_quota_filter(
    registry: &ProviderRegistry,
    official_enabled: bool,
    provider_filter: Option<&str>,
) -> anyhow::Result<()> {
    let Some(provider_id) = provider_filter else {
        return Ok(());
    };
    if provider_id == "official" {
        if official_enabled {
            return Ok(());
        }
        anyhow::bail!("official provider is unavailable outside Codex OAuth proxy mode");
    }
    let provider = registry
        .provider(provider_id)
        .ok_or_else(|| anyhow::anyhow!("unknown provider: {provider_id}"))?;
    if !provider.definition().enabled {
        anyhow::bail!("provider is disabled: {provider_id}");
    }
    Ok(())
}

async fn fetch_official_quota_results() -> Vec<serde_json::Value> {
    let result = async {
        let value = codex_mixin::provider::fetch_official_resource(
            &codex_home_path().join("auth.json"),
            false,
        )
        .await?;
        official_quota_rows(&value)
    }
    .await;
    match result {
        Ok(rows) => rows,
        Err(error) => vec![serde_json::json!({
            "provider_id": "official",
            "provider_display_name": "OpenAI",
            "display_name": "OpenAI",
            "quota_id": "quota",
            "label": "Quota",
            "value": null,
            "error": error.to_string(),
            "stale_at": null,
        })],
    }
}
