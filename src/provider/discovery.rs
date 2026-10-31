use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use crate::anthropic::{BaiduAvailableModelsResponse, ModelInfo, ModelsResponse};

use super::resolver::{
    MetadataResolver, enrich_models_with_models_dev, fetch_models_dev_catalog,
    fill_missing_model_fields, fill_model_gaps_with_resolver, provider_models_from_catalog,
};
use super::spec::{OPEN_CODE_GO_PRESET_ID, spec_for};
use super::{
    AwsSigV4AuthConfig, ProviderDefinition, ProviderModel, ProviderModelSource, ProviderRuntime,
    sign_aws_request,
};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const DEFAULT_SELECTED_MODEL_LIMIT: usize = 10;

// Keep one raw response per provider, bounded for small gateway hosts. Matching
// the resolved headers also invalidates env-based credentials without logging them.
const MAX_MODEL_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_MODEL_CACHE_ENTRIES: usize = 16;
#[derive(Clone)]
struct ModelsHttpCache {
    url: reqwest::Url,
    headers: reqwest::header::HeaderMap,
    etag: Option<reqwest::header::HeaderValue>,
    modified: Option<reqwest::header::HeaderValue>,
    body: String,
}
static MODELS_HTTP_CACHE: OnceLock<Mutex<HashMap<String, ModelsHttpCache>>> = OnceLock::new();

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InferenceProfilesResponse {
    #[serde(default)]
    inference_profile_summaries: Vec<InferenceProfileSummary>,
    next_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InferenceProfileSummary {
    inference_profile_id: String,
    inference_profile_name: String,
    inference_profile_arn: String,
    #[serde(default)]
    models: Vec<InferenceProfileModel>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InferenceProfileModel {
    model_arn: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FoundationModelsResponse {
    #[serde(default)]
    model_summaries: Vec<FoundationModelSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FoundationModelSummary {
    model_id: String,
    model_name: String,
    provider_name: String,
    #[serde(default)]
    input_modalities: Vec<String>,
}

pub async fn discover_provider_models(
    client: &Client,
    definition: &ProviderDefinition,
) -> anyhow::Result<Vec<ProviderModel>> {
    let provider = ProviderRuntime::new(definition.clone(), &|name| std::env::var(name).ok())?;
    let native_headers = native_baidu_headers_if_needed(definition, &provider).await?;
    if definition.preset_id.as_deref() == Some("agy") {
        return super::agy::discover_models(client, definition, &provider).await;
    }
    if definition.preset_id.as_deref() == Some("agy-business") {
        return super::agy_business::discover_models(client, &provider).await;
    }
    if definition.preset_id.as_deref() == Some("kiro") {
        return super::kiro::discover_models(client, &provider).await;
    }
    match &definition.model_source {
        ProviderModelSource::Static => {
            let mut models = definition.cached_models.clone();
            enrich_from_models_dev(
                client,
                definition,
                &mut models,
                enrich_models_with_models_dev,
            )
            .await;
            Ok(models)
        }
        ProviderModelSource::OpenAiCompatible { .. } => {
            discover_openai_models(client, definition, &provider).await
        }
        ProviderModelSource::BaiduOneApi => {
            discover_baidu_models(client, definition, &provider, native_headers.as_ref()).await
        }
        ProviderModelSource::AwsBedrock => discover_aws_bedrock_models(client, definition).await,
    }
}

async fn discover_aws_bedrock_models(
    client: &Client,
    definition: &ProviderDefinition,
) -> anyhow::Result<Vec<ProviderModel>> {
    let mantle_auth = definition
        .auth
        .aws_sigv4
        .as_ref()
        .context("AWS Bedrock model discovery requires SigV4 credentials")?;
    let mut control_auth = mantle_auth.clone();
    control_auth.service = "bedrock".to_owned();
    let base_url = format!("https://bedrock.{}.amazonaws.com", control_auth.region);
    let mut models = Vec::new();
    models.extend(
        fetch_inference_profiles(client, &base_url, &control_auth, "APPLICATION", true).await?,
    );
    models.extend(
        fetch_inference_profiles(client, &base_url, &control_auth, "SYSTEM_DEFINED", false).await?,
    );
    models.extend(fetch_foundation_models(client, &base_url, &control_auth).await?);
    enrich_from_models_dev(client, definition, &mut models, enrich_bedrock_models).await;
    normalize_models(&mut models);
    Ok(models)
}

async fn fetch_inference_profiles(
    client: &Client,
    base_url: &str,
    auth: &AwsSigV4AuthConfig,
    profile_type: &str,
    use_arn: bool,
) -> anyhow::Result<Vec<ProviderModel>> {
    let mut models = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = client
            .get(format!("{base_url}/inference-profiles"))
            .query(&[("typeEquals", profile_type)])
            .build()?;
        if let Some(token) = &next_token {
            request
                .url_mut()
                .query_pairs_mut()
                .append_pair("nextToken", token);
        }
        sign_aws_request(
            &mut request,
            auth,
            EMPTY_SHA256.to_owned(),
            SystemTime::now(),
        )?;
        let response = client.execute(request).await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!(
                "AWS Bedrock {profile_type} inference profiles returned {status}: {body}"
            );
        }
        let page: InferenceProfilesResponse = serde_json::from_str(&body)
            .context("AWS Bedrock inference profiles returned invalid JSON")?;
        next_token.clone_from(&page.next_token);
        models.extend(inference_profile_models(page, use_arn));
        if next_token.is_none() {
            break;
        }
    }
    Ok(models)
}

fn inference_profile_models(page: InferenceProfilesResponse, use_arn: bool) -> Vec<ProviderModel> {
    page.inference_profile_summaries
        .into_iter()
        .map(|profile| {
            let id = if use_arn {
                profile.inference_profile_arn
            } else {
                profile.inference_profile_id
            };
            let aliases = profile
                .models
                .into_iter()
                .filter_map(|model| model.model_arn.rsplit('/').next().map(str::to_owned))
                .collect();
            bedrock_discovered_model(id, profile.inference_profile_name, aliases, None)
        })
        .collect()
}

async fn fetch_foundation_models(
    client: &Client,
    base_url: &str,
    auth: &AwsSigV4AuthConfig,
) -> anyhow::Result<Vec<ProviderModel>> {
    let mut request = client
        .get(format!("{base_url}/foundation-models"))
        .build()?;
    sign_aws_request(
        &mut request,
        auth,
        EMPTY_SHA256.to_owned(),
        SystemTime::now(),
    )?;
    let response = client.execute(request).await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("AWS Bedrock foundation models returned {status}: {body}");
    }
    let response: FoundationModelsResponse = serde_json::from_str(&body)
        .context("AWS Bedrock foundation models returned invalid JSON")?;
    Ok(foundation_model_entries(response))
}

fn foundation_model_entries(response: FoundationModelsResponse) -> Vec<ProviderModel> {
    response
        .model_summaries
        .into_iter()
        .map(|model| {
            let supports_image = model
                .input_modalities
                .iter()
                .any(|modality| modality.eq_ignore_ascii_case("IMAGE"));
            bedrock_discovered_model(
                model.model_id,
                format!("{} {}", model.provider_name, model.model_name),
                Vec::new(),
                Some(supports_image),
            )
        })
        .collect()
}

fn bedrock_discovered_model(
    id: String,
    display_name: String,
    aliases: Vec<String>,
    supports_image: Option<bool>,
) -> ProviderModel {
    ProviderModel {
        id,
        aliases,
        display_name: Some(display_name),
        // The Bedrock control plane does not report reasoning or tool
        // support; models.dev enrichment and family hints fill those in.
        supports_image,
        supports_web_search: Some(false),
        supports_tool_search: Some(false),
        ..ProviderModel::default()
    }
}

fn enrich_bedrock_models(models: &mut [ProviderModel], metadata: &HashMap<String, ProviderModel>) {
    for model in models {
        if let Some(source) = bedrock_metadata_for(model, metadata) {
            fill_missing_model_fields(model, source);
        }
    }
}

fn bedrock_metadata_for<'metadata>(
    model: &ProviderModel,
    metadata: &'metadata HashMap<String, ProviderModel>,
) -> Option<&'metadata ProviderModel> {
    std::iter::once(model.id.as_str())
        .chain(model.aliases.iter().map(String::as_str))
        .find_map(|key| lookup_without_region_prefix(metadata, key))
}

/// Cross-region inference ids ("us.anthropic. ...") wrap the foundation model
/// id that models.dev catalogs.
fn lookup_without_region_prefix<'metadata>(
    metadata: &'metadata HashMap<String, ProviderModel>,
    key: &str,
) -> Option<&'metadata ProviderModel> {
    if let Some(found) = metadata.get(key) {
        return Some(found);
    }
    let (prefix, rest) = key.split_once('.')?;
    if matches!(prefix, "us" | "eu" | "apac" | "jp" | "au" | "ca" | "global") {
        metadata.get(rest)
    } else {
        None
    }
}

async fn native_baidu_headers_if_needed(
    definition: &ProviderDefinition,
    provider: &ProviderRuntime,
) -> anyhow::Result<Option<reqwest::header::HeaderMap>> {
    if definition.model_source == ProviderModelSource::BaiduOneApi && provider.uses_ducx_loopback()
    {
        Ok(Some(super::native_baidu_headers(provider).await?))
    } else {
        Ok(None)
    }
}

async fn discover_openai_models(
    client: &Client,
    definition: &ProviderDefinition,
    provider: &ProviderRuntime,
) -> anyhow::Result<Vec<ProviderModel>> {
    let models = fetch_openai_catalog(client, provider).await?;
    let mut models = models
        .data
        .into_iter()
        .map(|model| {
            openai_model_to_provider_model(
                model,
                definition.preset_id.as_deref() == Some("openrouter"),
            )
        })
        .collect::<Vec<_>>();
    normalize_models(&mut models);
    enrich_from_models_dev(
        client,
        definition,
        &mut models,
        enrich_models_with_models_dev,
    )
    .await;
    normalize_models(&mut models);
    Ok(models)
}

async fn fetch_openai_catalog(
    client: &Client,
    provider: &ProviderRuntime,
) -> anyhow::Result<ModelsResponse> {
    let url = provider
        .models_url()
        .context("provider models URL is not configured")?
        .clone();
    let request_client =
        crate::ech::provider_http_client(client, &url, std::time::Duration::from_secs(30)).await?;
    let mut request = provider.apply_auth(request_client.get(url)).build()?;
    let request_headers = request.headers().clone();
    let request_url = request.url().clone();
    let cache = MODELS_HTTP_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let cached = {
        let entries = cache
            .lock()
            .map_err(|_| anyhow::anyhow!("models HTTP cache lock poisoned"))?;
        entries
            .get(provider.id())
            .filter(|entry| entry.url == request_url && entry.headers == request_headers)
            .cloned()
    };
    if let Some(entry) = &cached {
        if let Some(etag) = &entry.etag {
            request
                .headers_mut()
                .insert(reqwest::header::IF_NONE_MATCH, etag.clone());
        } else if let Some(modified) = &entry.modified {
            request
                .headers_mut()
                .insert(reqwest::header::IF_MODIFIED_SINCE, modified.clone());
        }
    }
    let response = request_client.execute(request).await?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let body = if status == reqwest::StatusCode::NOT_MODIFIED {
        let entry =
            cached.context("models endpoint returned 304 without a matching cached response")?;
        tracing::info!(
            event = "provider_models_not_modified",
            provider_id = provider.id(),
            status = 304,
            "provider catalog unchanged; cached body reused"
        );
        entry.body
    } else {
        let body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!(
                "provider {} models endpoint returned {status}: {body}",
                provider.id()
            );
        }
        body
    };
    let models: ModelsResponse =
        serde_json::from_str(&body).context("provider models endpoint returned invalid JSON")?;
    if status != reqwest::StatusCode::NOT_MODIFIED {
        let etag = response_headers.get(reqwest::header::ETAG).cloned();
        let modified = response_headers
            .get(reqwest::header::LAST_MODIFIED)
            .cloned();
        let no_store = response_headers
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(',')
                    .any(|directive| directive.trim().eq_ignore_ascii_case("no-store"))
            });
        let mut entries = cache
            .lock()
            .map_err(|_| anyhow::anyhow!("models HTTP cache lock poisoned"))?;
        entries.remove(provider.id());
        if !no_store
            && body.len() <= MAX_MODEL_CACHE_BYTES
            && (etag.is_some() || modified.is_some())
        {
            if entries.len() >= MAX_MODEL_CACHE_ENTRIES
                || entries
                    .values()
                    .map(|entry| entry.body.len())
                    .sum::<usize>()
                    + body.len()
                    > MAX_MODEL_CACHE_BYTES
            {
                entries.clear();
            }
            entries.insert(
                provider.id().to_owned(),
                ModelsHttpCache {
                    url: request_url,
                    headers: request_headers,
                    etag,
                    modified,
                    body,
                },
            );
        }
        tracing::info!(
            event = "provider_models_downloaded",
            provider_id = provider.id(),
            status = status.as_u16(),
            "provider model catalog downloaded"
        );
    }
    Ok(models)
}

fn models_dev_provider_for(definition: &ProviderDefinition) -> Option<&'static str> {
    if let Some(provider) = spec_for(definition.preset_id.as_deref()).models_dev_provider {
        return Some(provider);
    }
    // OpenCode Go providers configured before preset stamping carry the id only.
    (definition.id == OPEN_CODE_GO_PRESET_ID).then_some(OPEN_CODE_GO_PRESET_ID)
}

/// Complete missing model limits and capabilities from models.dev.
///
/// Providers with a direct models.dev mapping get exact-id enrichment first;
/// every provider then falls through to the fuzzy resolver so custom or
/// renamed upstream ids still pick up their model family's metadata. When the
/// live catalog fetch fails, the locally cached copy (refresh-metadata) keeps
/// the fuzzy pass working offline.
async fn enrich_from_models_dev(
    client: &Client,
    definition: &ProviderDefinition,
    models: &mut [ProviderModel],
    enrich_exact: fn(&mut [ProviderModel], &HashMap<String, ProviderModel>),
) {
    let resolver = match fetch_models_dev_catalog(client).await {
        Ok(catalog) => {
            if let Some(models_dev_provider) = models_dev_provider_for(definition) {
                match provider_models_from_catalog(&catalog, models_dev_provider) {
                    Some(metadata) => enrich_exact(models, &metadata),
                    None => tracing::warn!(
                        provider_id = %definition.id,
                        models_dev_provider,
                        "models.dev catalog does not include the mapped provider"
                    ),
                }
            }
            MetadataResolver::from_catalog(&catalog)
        }
        Err(error) => {
            tracing::warn!(
                provider_id = %definition.id,
                error = %format!("{error:#}"),
                "failed to fetch the models.dev catalog; falling back to the local cache"
            );
            MetadataResolver::from_default_files().unwrap_or_else(|_| MetadataResolver::empty())
        }
    };
    fill_model_gaps_with_resolver(&resolver, models);
}

fn openai_model_to_provider_model(model: ModelInfo, is_openrouter: bool) -> ProviderModel {
    let (supports_image, supports_thinking, supports_web_search, supports_function_tools) =
        if is_openrouter {
            let supports_parameter = |parameter| {
                model
                    .supported_parameters
                    .iter()
                    .any(|supported| supported == parameter)
            };
            let parameters_declared = !model.supported_parameters.is_empty();
            (
                model.supports_image.or_else(|| {
                    model.architecture.as_ref().map(|architecture| {
                        architecture
                            .input_modalities
                            .iter()
                            .any(|modality| modality.eq_ignore_ascii_case("image"))
                    })
                }),
                model.supports_thinking.or_else(|| {
                    (parameters_declared || model.reasoning.is_some())
                        .then(|| supports_parameter("reasoning") || model.reasoning.is_some())
                }),
                model.supports_web_search.or_else(|| {
                    parameters_declared.then(|| supports_parameter("web_search_options"))
                }),
                model
                    .supports_function_tools
                    .or_else(|| parameters_declared.then(|| supports_parameter("tools"))),
            )
        } else {
            (
                model.supports_image,
                model.supports_thinking,
                model.supports_web_search,
                model.supports_function_tools,
            )
        };
    ProviderModel {
        id: model.id,
        aliases: Vec::new(),
        manually_added: false,
        display_name: model.display_name,
        description: model.description,
        ratio: model.ratio,
        price_type: model.price_type,
        context_window: model.context_window,
        source_context_window: model.context_window,
        max_output_tokens: model.max_output_tokens,
        protocol: model.protocol,
        api_path: model.api_path,
        supports_image,
        supports_thinking,
        supports_web_search,
        supports_tool_search: model.supports_tool_search,
        supports_function_tools,
        capability_probe_error: model.capability_probe_error,
        capabilities_probed_at_ms: model.capabilities_probed_at_ms,
    }
}

async fn discover_baidu_models(
    client: &Client,
    definition: &ProviderDefinition,
    provider: &ProviderRuntime,
    native_headers: Option<&reqwest::header::HeaderMap>,
) -> anyhow::Result<Vec<ProviderModel>> {
    let url = provider
        .models_url()
        .context("provider available-models URL is not configured")?
        .clone();
    let request_client =
        crate::ech::provider_http_client(client, &url, std::time::Duration::from_secs(30)).await?;
    let request = match native_headers {
        Some(headers) => request_client.post(url).headers(headers.clone()),
        None => provider.apply_auth(request_client.post(url)),
    };
    let response = request.json(&json!({})).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!(
            "provider {} available-models endpoint returned {status}: {body}",
            provider.id()
        );
    }
    let available: BaiduAvailableModelsResponse = serde_json::from_str(&body)
        .context("provider available-models endpoint returned invalid JSON")?;
    if !available.success {
        anyhow::bail!(
            "provider {} available-models endpoint failed: {}",
            provider.id(),
            available.message
        );
    }
    let mut models = Vec::with_capacity(available.data.len());
    let mut model_indices = HashMap::with_capacity(available.data.len());
    for model in available.data {
        add_baidu_model(provider, model, &mut models, &mut model_indices);
    }
    enrich_from_models_dev(
        client,
        definition,
        &mut models,
        enrich_models_with_models_dev,
    )
    .await;
    normalize_models(&mut models);
    Ok(models)
}

fn add_baidu_model(
    provider: &ProviderRuntime,
    model: crate::anthropic::BaiduAvailableModel,
    models: &mut Vec<ProviderModel>,
    model_indices: &mut HashMap<String, usize>,
) {
    let (id, is_internal) = match model.model.strip_suffix("-内部") {
        Some(canonical) => (canonical.to_owned(), true),
        None => (model.model.clone(), false),
    };
    let Some(capability) = model.capability else {
        tracing::warn!(
            provider_id = provider.id(),
            model = %model.model,
            "excluding available-models entry without capability metadata"
        );
        return;
    };
    let declared_capability = |name| {
        model
            .capability_set
            .iter()
            .any(|capability| capability == name)
    };
    let description = capability.model_description;
    let converted = ProviderModel {
        id: id.clone(),
        display_name: Some(id.clone()),
        description: Some(description),
        ratio: Some(capability.ratio),
        price_type: Some(model.price_type),
        context_window: Some(capability.context_window),
        supports_image: Some(capability.supports_image || declared_capability("image")),
        supports_thinking: Some(true),
        // The endpoint currently ships an empty capability_set, so an absent
        // token means "unknown", not "unsupported". The catalog is never
        // probed; earlier web-search probe results still apply.
        supports_web_search: declared_capability("web_search").then_some(true),
        supports_tool_search: Some(false),
        supports_function_tools: Some(true),
        ..ProviderModel::default()
    };
    if let Some(&index) = model_indices.get(&id) {
        if !is_internal {
            models[index] = converted;
        }
    } else {
        model_indices.insert(id, models.len());
        models.push(converted);
    }
}

pub const AUTO_SELECT_NEW_MODEL_LIMIT: usize = 10;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModelDiscoveryChanges {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub auto_selected: Vec<String>,
    pub updated: Vec<String>,
}

pub fn apply_discovered_models(
    provider: &mut ProviderDefinition,
    mut models: Vec<ProviderModel>,
) -> anyhow::Result<ModelDiscoveryChanges> {
    let discovered_ids = models
        .iter()
        .map(|model| model.id.clone())
        .collect::<HashSet<_>>();
    models.extend(
        provider
            .cached_models
            .iter()
            .filter(|model| model.manually_added && !discovered_ids.contains(&model.id))
            .cloned(),
    );
    normalize_models(&mut models);
    for model in &mut models {
        if model.source_context_window.is_none() {
            model.source_context_window = model.context_window;
        }
        if let Some(&context_window) = provider.model_context_overrides.get(&model.id) {
            model.context_window = Some(context_window);
        }
    }
    let available_models = models
        .iter()
        .map(|model| model.id.as_str())
        .collect::<HashSet<_>>();
    if provider.preset_id.as_deref() != Some("agy") {
        provider
            .model_context_overrides
            .retain(|model, _| available_models.contains(model.as_str()));
    }
    let first_successful_refresh = provider.models_refreshed_at_ms.is_none();
    let mut changes = ModelDiscoveryChanges::default();
    if first_successful_refresh
        && (provider.preset_id.as_deref() != Some("agy") || provider.selected_models.is_empty())
    {
        provider.selected_models = models
            .iter()
            .take(DEFAULT_SELECTED_MODEL_LIMIT)
            .map(|model| model.id.clone())
            .collect();
        provider.new_models.clear();
    } else {
        let previous_models = provider
            .cached_models
            .iter()
            .map(|model| (model.id.as_str(), model))
            .collect::<HashMap<_, _>>();
        changes.added = models
            .iter()
            .filter(|model| !previous_models.contains_key(model.id.as_str()))
            .map(|model| model.id.clone())
            .collect();
        changes.removed = provider
            .cached_models
            .iter()
            .filter(|model| !model.manually_added && !available_models.contains(model.id.as_str()))
            .map(|model| model.id.clone())
            .collect();
        changes.updated = models
            .iter()
            .filter(|model| {
                previous_models
                    .get(model.id.as_str())
                    .is_some_and(|previous| **previous != **model)
            })
            .map(|model| model.id.clone())
            .collect();
        if provider.preset_id.as_deref() != Some("agy") {
            provider
                .selected_models
                .retain(|model| available_models.contains(model.as_str()));
        }
        if provider.preset_id.as_deref() != Some("agy")
            && !changes.added.is_empty()
            && changes.added.len() < AUTO_SELECT_NEW_MODEL_LIMIT
        {
            provider
                .selected_models
                .extend(changes.added.iter().cloned());
            changes.auto_selected = changes.added.clone();
        }
        provider.new_models = changes.added.clone();
    }
    provider.cached_models = models;
    provider.model_order.apply(&mut provider.selected_models);
    provider.models_refreshed_at_ms =
        Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64);
    provider.models_refresh_error = None;
    provider.prune_stale_auto_review_model();
    provider.validate()?;
    Ok(changes)
}

pub fn redact_provider_error(definition: &ProviderDefinition, error: &str) -> String {
    let mut redacted = error.to_owned();
    if !definition.auth.api_key.is_empty() {
        redacted = redacted.replace(&definition.auth.api_key, "<redacted>");
    }
    if let Some(aws) = &definition.auth.aws_sigv4 {
        for secret in [
            Some(aws.access_key_id.as_str()),
            Some(aws.secret_access_key.as_str()),
            aws.session_token.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|secret| !secret.is_empty())
        {
            redacted = redacted.replace(secret, "<redacted>");
        }
    }
    redacted.chars().take(8_000).collect()
}

fn normalize_models(models: &mut Vec<ProviderModel>) {
    models.retain(|model| !model.id.trim().is_empty());
    for model in models.iter_mut() {
        // External catalogs use zero for unknown or inapplicable token limits.
        for limit in [
            &mut model.context_window,
            &mut model.source_context_window,
            &mut model.max_output_tokens,
        ] {
            if *limit == Some(0) {
                *limit = None;
            }
        }
    }
    models.sort_by(|left, right| super::compare_model_ids(&left.id, &right.id));
    models.dedup_by(|left, right| left.id == right.id);
}
