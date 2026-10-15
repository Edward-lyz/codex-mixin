//! Account-scoped Antigravity discovery and measured quota windows.

use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, ensure};
use serde::Deserialize;
use serde_json::Value;

use super::{ProviderDefinition, ProviderModel, ProviderProtocol, ProviderRuntime};

pub(crate) fn require_native_account(provider: &ProviderDefinition) -> anyhow::Result<()> {
    ensure!(
        provider.auth.api_key.trim().is_empty(),
        "local AGY reports cannot represent a provider configured with an explicit API key"
    );
    let url = reqwest::Url::parse(&provider.base_url)?;
    ensure!(
        url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && matches!(
                url.host_str(),
                Some("daily-cloudcode-pa.googleapis.com" | "cloudcode-pa.googleapis.com")
            ),
        "local AGY authentication is restricted to the supported Google HTTPS endpoints"
    );
    Ok(())
}

pub(crate) async fn discover_models(
    client: &reqwest::Client,
    definition: &ProviderDefinition,
    provider: &ProviderRuntime,
) -> anyhow::Result<Vec<ProviderModel>> {
    if definition.auth.api_key.trim().is_empty() {
        require_native_account(definition)?;
        let report = crate::platform::agy_report(
            crate::platform::AgyReport::Models,
            super::auth::agy::resolve_agy_token_path(),
        )
        .await?;
        return parse_models(&report);
    }
    // An explicitly configured key must never borrow the ambient CLI account.
    let url =
        reqwest::Url::parse(&definition.base_url)?.join("/v1internal:fetchAvailableModels")?;
    let response = provider
        .apply_auth(client.post(url))
        .timeout(std::time::Duration::from_secs(20))
        .json(&serde_json::json!({"project": "aicode-consumers"}))
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "AGY models endpoint returned {}",
        response.status()
    );
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 1024 * 1024,
            "AGY model directory exceeds 1 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    parse_models(&serde_json::from_slice(&bytes).context("invalid AGY models JSON")?)
}

#[derive(Deserialize)]
struct ModelEntry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, alias = "displayName")]
    label: Option<String>,
    #[serde(default, alias = "inputTokenLimit")]
    context_window: Option<u64>,
    #[serde(default, alias = "outputTokenLimit")]
    max_output_tokens: Option<u64>,
    #[serde(default)]
    supports_image: Option<bool>,
    #[serde(default)]
    supports_thinking: Option<bool>,
    #[serde(default)]
    supports_web_search: Option<bool>,
    #[serde(default)]
    supports_function_tools: Option<bool>,
}

fn parse_models(value: &Value) -> anyhow::Result<Vec<ProviderModel>> {
    let models = value
        .get("models")
        .context("AGY directory is missing models")?;
    let entries: Vec<(Option<String>, ModelEntry)> = match models {
        Value::Array(models) => models
            .iter()
            .map(|model| serde_json::from_value(model.clone()).map(|entry| (None, entry)))
            .collect::<Result<_, _>>()?,
        Value::Object(models) => models
            .iter()
            .map(|(id, model)| {
                serde_json::from_value(model.clone()).map(|entry| (Some(id.clone()), entry))
            })
            .collect::<Result<_, _>>()?,
        _ => anyhow::bail!("AGY models must be an array or object"),
    };
    let mut ids = HashSet::new();
    let mut discovered = Vec::new();
    for (key, entry) in entries {
        let id = key
            .or(entry.id)
            .context("AGY model is missing its upstream ID")?;
        ensure!(
            !id.trim().is_empty() && !id.chars().any(char::is_control),
            "invalid AGY model ID"
        );
        ensure!(ids.insert(id.clone()), "duplicate AGY model ID: {id}");
        ensure!(
            entry.context_window != Some(0) && entry.max_output_tokens != Some(0),
            "AGY model {id} has an invalid token limit"
        );
        discovered.push(ProviderModel {
            id,
            display_name: entry.label,
            protocol: Some(ProviderProtocol::GoogleGemini),
            context_window: entry.context_window,
            max_output_tokens: entry.max_output_tokens,
            supports_image: entry.supports_image,
            supports_thinking: entry.supports_thinking,
            supports_web_search: entry.supports_web_search,
            supports_function_tools: entry.supports_function_tools,
            ..Default::default()
        });
    }
    Ok(discovered)
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgyQuotaWindow {
    pub id: String,
    pub label: String,
    pub remaining_percent: f64,
    pub reset_at: Option<String>,
}

pub async fn fetch_agy_quota(provider: &ProviderDefinition) -> anyhow::Result<Vec<AgyQuotaWindow>> {
    require_native_account(provider)?;
    let report = crate::platform::agy_report(
        crate::platform::AgyReport::Usage,
        super::auth::agy::resolve_agy_token_path(),
    )
    .await?;
    parse_quota(&report)
}

#[derive(Deserialize)]
struct QuotaGroup {
    name: String,
    buckets: Vec<QuotaBucket>,
}

#[derive(Deserialize)]
struct QuotaBucket {
    name: String,
    remaining_fraction: Option<f64>,
    reset_time: Option<String>,
    enabled: Option<bool>,
}

fn parse_quota(value: &Value) -> anyhow::Result<Vec<AgyQuotaWindow>> {
    let groups: Vec<QuotaGroup> = serde_json::from_value(
        value
            .get("groups")
            .context("AGY usage is missing quota groups")?
            .clone(),
    )?;
    let mut windows = BTreeMap::new();
    for group in groups {
        ensure!(
            !group.name.trim().is_empty(),
            "AGY quota group has an empty name"
        );
        for bucket in group.buckets {
            if bucket.enabled == Some(false) {
                continue;
            }
            ensure!(
                !bucket.name.trim().is_empty(),
                "AGY quota bucket has an empty name"
            );
            let fraction = bucket
                .remaining_fraction
                .context("AGY quota bucket has no measured remaining fraction")?;
            ensure!(
                fraction.is_finite() && (0.0..=1.0).contains(&fraction),
                "invalid AGY quota fraction"
            );
            if let Some(reset) = &bucket.reset_time {
                chrono::DateTime::parse_from_rfc3339(reset)
                    .context("invalid AGY quota reset time")?;
            }
            let id = format!("{}/{}", group.name, bucket.name);
            let window = AgyQuotaWindow {
                id: id.clone(),
                label: format!("{} - {}", group.name, bucket.name),
                remaining_percent: fraction * 100.0,
                reset_at: bucket.reset_time,
            };
            ensure!(
                windows.insert(id, window).is_none(),
                "duplicate AGY quota bucket"
            );
        }
    }
    ensure!(
        !windows.is_empty(),
        "AGY returned no measured quota windows"
    );
    Ok(windows.into_values().collect())
}

#[cfg(test)]
mod tests;
