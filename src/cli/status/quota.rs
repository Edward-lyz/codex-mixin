use std::time::Duration;

use codex_mixin::config::load_stored_config;
use codex_mixin::provider::{ProviderQuotaParser, ProviderRegistry};
use futures_util::stream::{self, StreamExt};

use super::super::codex::{
    codex_home_path, managed_codex_install_mode, request_app_server, resolve_codex_cli,
    resolve_codex_config_path,
};
use super::opencode_go::{
    OPENCODE_GO_DASHBOARD_BASE, fetch_opencode_go_quota_results, official_quota_rows,
};

pub(crate) async fn quota(json_output: bool, provider_filter: Option<&str>) -> anyhow::Result<()> {
    let official_enabled =
        managed_codex_install_mode(&resolve_codex_config_path(None)?)? == Some("codex_oauth_proxy");
    let stored = match load_stored_config()? {
        Some(stored) => stored,
        None if official_enabled => Default::default(),
        None => anyhow::bail!("provider configuration is missing"),
    };
    let registry = ProviderRegistry::new(stored.providers)?;
    validate_quota_filter(&registry, official_enabled, provider_filter)?;
    let client = reqwest::Client::builder()
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
        let cli = resolve_codex_cli()?;
        let codex_home = codex_home_path();
        let reply = tokio::task::spawn_blocking(move || {
            request_app_server(
                &cli,
                &codex_home,
                "account/rateLimits/read",
                None,
                Duration::from_secs(10),
                "codex-mixin-quota",
            )
        })
        .await??;
        official_quota_rows(&reply.result)
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QuotaUsageSummary {
    pub(crate) used: Option<f64>,
    pub(crate) limit: Option<f64>,
    pub(crate) remaining: Option<f64>,
    pub(crate) currency: Option<String>,
}

pub(crate) fn quota_usage(
    parser: ProviderQuotaParser,
    value: &serde_json::Value,
) -> anyhow::Result<QuotaUsageSummary> {
    if parser == ProviderQuotaParser::DeepSeek {
        return deepseek_quota_usage(value);
    }
    let (used_fields, limit_fields, remaining_fields): (&[&str], &[&str], &[&str]) = match parser {
        ProviderQuotaParser::BaiduOneApi => (
            &["used_quota", "used", "usage"],
            &[
                "month_quota_limit",
                "quota_limit",
                "limit",
                "total",
                "quota",
            ],
            &["remaining_quota", "remaining", "available"],
        ),
        ProviderQuotaParser::OpenRouter => (
            &["total_usage", "used", "usage"],
            &["total_credits", "limit", "total", "budget"],
            &["remaining", "remaining_quota", "available"],
        ),
        ProviderQuotaParser::Generic => (
            &[
                "used",
                "used_quota",
                "usage",
                "total_usage",
                "total_used",
                "spent",
                "cost",
                "consumed",
                "actual_cost",
            ],
            &[
                "limit",
                "total",
                "total_credits",
                "total_granted",
                "quota",
                "quota_limit",
                "month_quota_limit",
                "budget",
            ],
            &[
                "remaining",
                "remaining_quota",
                "available",
                "total_available",
                "balance",
            ],
        ),
        ProviderQuotaParser::DeepSeek => unreachable!("DeepSeek quota handled above"),
        ProviderQuotaParser::OpenCodeGo => {
            unreachable!("OpenCode Go quota is fetched from the dashboard HTML")
        }
    };
    let used = first_quota_value(value, used_fields)
        .ok_or_else(|| anyhow::anyhow!("quota response does not contain a valid used amount"))?;
    let reported_remaining = first_quota_value(value, remaining_fields);
    let limit = first_quota_value(value, limit_fields)
        .or_else(|| reported_remaining.map(|remaining| used + remaining));
    let remaining = reported_remaining.or_else(|| limit.map(|limit| (limit - used).max(0.0)));
    Ok(QuotaUsageSummary {
        used: Some(used),
        limit,
        remaining,
        currency: None,
    })
}

fn deepseek_quota_usage(value: &serde_json::Value) -> anyhow::Result<QuotaUsageSummary> {
    let balances = value
        .get("balance_infos")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("DeepSeek quota response has no balance_infos array"))?;
    let parsed = balances
        .iter()
        .filter_map(|entry| {
            let amount = entry.get("total_balance").and_then(json_f64)?;
            (amount.is_finite() && amount >= 0.0).then(|| {
                let currency = entry
                    .get("currency")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|currency| {
                        currency.len() == 3
                            && currency.bytes().all(|byte| byte.is_ascii_alphabetic())
                    })
                    .map(str::to_ascii_uppercase);
                (amount, currency)
            })
        })
        .collect::<Vec<_>>();
    let balance = parsed
        .iter()
        .find(|(amount, _)| *amount > 0.0)
        .or_else(|| parsed.first())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("DeepSeek quota response has no valid total_balance"))?;
    Ok(QuotaUsageSummary {
        used: None,
        limit: None,
        remaining: Some(balance.0),
        currency: balance.1,
    })
}

fn first_quota_value(value: &serde_json::Value, fields: &[&str]) -> Option<f64> {
    [
        "",
        "/data",
        "/quota",
        "/data/quota",
        "/usage",
        "/data/usage",
        "/usage/total",
        "/data/usage/total",
    ]
    .iter()
    .find_map(|base| {
        fields.iter().find_map(|field| {
            let pointer = if base.is_empty() {
                format!("/{field}")
            } else {
                format!("{base}/{field}")
            };
            value.pointer(&pointer).and_then(json_f64)
        })
    })
    .filter(|value| value.is_finite() && *value >= 0.0)
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

#[cfg(test)]
pub(crate) fn summarize_quota_json(value: &serde_json::Value) -> String {
    for base in [
        "",
        "/data",
        "/quota",
        "/data/quota",
        "/usage",
        "/data/usage",
    ] {
        if let Some(used) = first_json_number(
            value,
            base,
            &[
                "used",
                "used_quota",
                "usage",
                "total_usage",
                "spent",
                "cost",
                "consumed",
            ],
        ) {
            let limit = first_json_number(
                value,
                base,
                &[
                    "limit",
                    "total",
                    "total_credits",
                    "quota",
                    "quota_limit",
                    "month_quota_limit",
                    "budget",
                ],
            );
            let remaining =
                first_json_number(value, base, &["remaining", "remaining_quota", "available"]);
            if let Some(limit) = limit {
                if let Some(remaining) = remaining {
                    return format!("quota used: {used} / {limit}, remaining: {remaining}");
                }
                return format!("quota used: {used} / {limit}");
            }
            return format!("quota used: {used}");
        }
    }
    for path in [
        "/remaining",
        "/quota/remaining",
        "/data/remaining",
        "/data/quota/remaining",
        "/data/available",
        "/data/used",
        "/data/total",
        "/data/ratio",
        "/balance",
        "/data/balance",
        "/data/quota",
        "/total_available",
    ] {
        if let Some(value) = value.pointer(path) {
            return format!("quota {path}: {value}");
        }
    }
    value.to_string()
}

#[cfg(test)]
pub(crate) fn first_json_number(
    value: &serde_json::Value,
    base: &str,
    fields: &[&str],
) -> Option<serde_json::Number> {
    fields.iter().find_map(|field| {
        let pointer = if base.is_empty() {
            format!("/{field}")
        } else {
            format!("{base}/{field}")
        };
        value.pointer(&pointer).and_then(json_number)
    })
}

#[cfg(test)]
pub(crate) fn json_number(value: &serde_json::Value) -> Option<serde_json::Number> {
    match value {
        serde_json::Value::Number(number) => Some(number.clone()),
        serde_json::Value::String(value) => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64),
        _ => None,
    }
}
