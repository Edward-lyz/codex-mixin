//! Account-scoped Antigravity discovery and measured quota windows.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use serde::Deserialize;
use serde_json::Value;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

use super::{ProviderDefinition, ProviderModel, ProviderProtocol, ProviderRuntime};

pub(crate) fn require_native_account(provider: &ProviderDefinition) -> anyhow::Result<()> {
    ensure!(
        provider.auth.api_key.trim().is_empty(),
        "local AGY OAuth cannot represent a provider configured with an explicit API key"
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
    let headers = auth_headers(definition).await?;
    let project = match headers.as_ref() {
        Some(headers) => project_id(client, provider, headers).await?,
        None => "aicode-consumers".to_owned(),
    };
    let value = fetch_json(
        client,
        provider,
        headers,
        "fetchAvailableModels",
        serde_json::json!({"project":project}),
    )
    .await?;
    parse_models(&value)
}

type ProjectCache = HashMap<(String, reqwest::header::HeaderValue), (Instant, String)>;
static PROJECTS: LazyLock<std::sync::Mutex<ProjectCache>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));
const PROJECT_CACHE_LIMIT: usize = 16;
const PROJECT_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

pub(crate) async fn project_id(
    client: &reqwest::Client,
    provider: &ProviderRuntime,
    headers: &reqwest::header::HeaderMap,
) -> anyhow::Result<String> {
    let authorization = headers
        .get(reqwest::header::AUTHORIZATION)
        .context("AGY OAuth authorization is missing")?
        .clone();
    let key = (provider.definition().base_url.clone(), authorization);
    {
        let cache = PROJECTS
            .lock()
            .map_err(|_| anyhow::anyhow!("AGY project cache lock failed"))?;
        if let Some((_, project)) = cache
            .get(&key)
            .filter(|(expires, _)| *expires > Instant::now())
        {
            return Ok(project.clone());
        }
    }
    let value = fetch_json(client, provider, Some(headers.clone()), "loadCodeAssist", serde_json::json!({"metadata":{"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}})).await?;
    let project = value.get("cloudaicompanionProject").or_else(|| value.get("antigravityProjectId")).or_else(|| value.get("projectId"))
        .and_then(|value| value.as_str().or_else(|| value.get("id").and_then(Value::as_str)))
        .filter(|project| !project.trim().is_empty()).context("AGY account has no Cloud Code project; complete account onboarding before using the provider")?.to_owned();
    let mut cache = PROJECTS
        .lock()
        .map_err(|_| anyhow::anyhow!("AGY project cache lock failed"))?;
    cache.retain(|_, (expires, _)| *expires > Instant::now());
    if cache.len() >= PROJECT_CACHE_LIMIT
        && let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (expires, _))| *expires)
            .map(|(key, _)| key.clone())
    {
        cache.remove(&oldest);
    }
    cache.insert(key, (Instant::now() + PROJECT_CACHE_TTL, project.clone()));
    Ok(project)
}

async fn auth_headers(
    definition: &ProviderDefinition,
) -> anyhow::Result<Option<reqwest::header::HeaderMap>> {
    if !definition.auth.api_key.trim().is_empty() {
        return Ok(None);
    }
    require_native_account(definition)?;
    Ok(Some(super::auth::agy::get_agy_headers().await?))
}

async fn fetch_json(
    client: &reqwest::Client,
    provider: &ProviderRuntime,
    headers: Option<reqwest::header::HeaderMap>,
    operation: &str,
    body: Value,
) -> anyhow::Result<Value> {
    let url = reqwest::Url::parse(&provider.definition().base_url)?
        .join(&format!("/v1internal:{operation}"))?;
    let request = client
        .post(url)
        .timeout(std::time::Duration::from_secs(20))
        .json(&body);
    let request = match headers {
        Some(headers) => request.headers(headers),
        None => provider.apply_auth(request),
    };
    let mut response = request.send().await?;
    ensure!(
        response.status().is_success(),
        "AGY {operation} returned {}",
        response.status()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= MAX_RESPONSE_BYTES,
            "AGY {operation} response exceeds 1 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).context("invalid AGY response JSON")
}

#[derive(Deserialize)]
struct ModelEntry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, alias = "displayName")]
    label: Option<String>,
    #[serde(default, alias = "inputTokenLimit", alias = "maxTokens")]
    context_window: Option<u64>,
    #[serde(default, alias = "outputTokenLimit", alias = "maxOutputTokens")]
    max_output_tokens: Option<u64>,
    #[serde(default, alias = "supportsImages")]
    supports_image: Option<bool>,
    #[serde(default, alias = "supportsThinking")]
    supports_thinking: Option<bool>,
    #[serde(default)]
    supports_web_search: Option<bool>,
    #[serde(default)]
    supports_function_tools: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct AgentModels {
    agent_model_sorts: Vec<AgentModelSort>,
    default_agent_model_id: Option<String>,
    web_search_model_ids: Option<HashSet<String>>,
}

#[derive(Deserialize)]
struct AgentModelSort {
    groups: Vec<AgentModelGroup>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentModelGroup {
    model_ids: Vec<String>,
}

fn parse_models(value: &Value) -> anyhow::Result<Vec<ProviderModel>> {
    let agent_models = AgentModels::deserialize(value).context("invalid AGY agent model groups")?;
    let agent_model_ids = agent_models
        .agent_model_sorts
        .into_iter()
        .flat_map(|sort| sort.groups)
        .flat_map(|group| group.model_ids)
        .chain(agent_models.default_agent_model_id)
        .collect::<HashSet<_>>();
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
        // Native agent models use function calls, but the directory omits a
        // per-model tools flag. Keep unclassified models unknown.
        let supports_function_tools = entry
            .supports_function_tools
            .or_else(|| agent_model_ids.contains(&id).then_some(true));
        let supports_web_search = entry.supports_web_search.or_else(|| {
            agent_models
                .web_search_model_ids
                .as_ref()
                .map(|ids| ids.contains(&id))
        });
        discovered.push(ProviderModel {
            id,
            display_name: entry.label,
            protocol: Some(ProviderProtocol::GoogleGemini),
            context_window: entry.context_window,
            max_output_tokens: entry.max_output_tokens,
            supports_image: entry.supports_image,
            supports_thinking: entry.supports_thinking,
            supports_web_search,
            supports_function_tools,
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
    let headers = auth_headers(provider).await?;
    let runtime = ProviderRuntime::new(provider.clone(), &|name| std::env::var(name).ok())?;
    let client = reqwest::Client::new();
    let report = fetch_json(
        &client,
        &runtime,
        headers,
        "retrieveUserQuotaSummary",
        serde_json::json!({}),
    )
    .await?;
    parse_quota(&report)
}

#[derive(Deserialize)]
struct QuotaGroup {
    #[serde(alias = "displayName")]
    name: String,
    buckets: Vec<QuotaBucket>,
}

#[derive(Deserialize)]
struct QuotaBucket {
    #[serde(default, rename = "bucketId")]
    id: Option<String>,
    #[serde(alias = "displayName")]
    name: String,
    #[serde(alias = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(alias = "resetTime")]
    reset_time: Option<String>,
    enabled: Option<bool>,
}

// AGY names windows like "Claude and GPT models" / "Weekly Limit Remaining".
// Menus and rings only have room for "Claude 7d"; unknown names pass through
// with the redundant suffixes removed, so new upstream windows stay readable.
fn compact_quota_label(group: &str, bucket: &str) -> String {
    let family = strip_suffix_ignore_case(group.trim(), "models");
    let family = family.split(" and ").next().unwrap_or(family).trim();
    let mut window = bucket.trim();
    for suffix in ["remaining", "limit"] {
        window = strip_suffix_ignore_case(window, suffix);
    }
    let window = match window.to_ascii_lowercase().as_str() {
        "five hour" | "5 hour" | "5h" => "5h",
        "weekly" | "week" | "7 day" => "7d",
        _ => window,
    };
    if window.is_empty() {
        return family.to_owned();
    }
    format!("{family} {window}")
}

fn strip_suffix_ignore_case<'a>(text: &'a str, suffix: &str) -> &'a str {
    let split = text.len().saturating_sub(suffix.len());
    match (text.get(..split), text.get(split..)) {
        (Some(head), Some(tail)) if tail.eq_ignore_ascii_case(suffix) => head.trim_end(),
        _ => text,
    }
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
            let id = bucket
                .id
                .unwrap_or_else(|| format!("{}/{}", group.name, bucket.name));
            let window = AgyQuotaWindow {
                id: id.clone(),
                label: compact_quota_label(&group.name, &bucket.name),
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
