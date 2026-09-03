use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use crate::anthropic::{BaiduAvailableModelsResponse, ModelInfo, ModelsResponse};

use super::resolver::{
    enrich_models_with_models_dev, fetch_models_dev_provider_models, uses_models_dev_capabilities,
};
use super::{
    AwsSigV4AuthConfig, ProviderDefinition, ProviderModel, ProviderModelSource, ProviderRuntime,
    sign_aws_request,
};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const DEFAULT_SELECTED_MODEL_LIMIT: usize = 10;

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
    match &definition.model_source {
        ProviderModelSource::Static => Ok(definition.cached_models.clone()),
        ProviderModelSource::OpenAiCompatible { .. } => {
            discover_openai_models(client, definition, &provider).await
        }
        ProviderModelSource::BaiduOneApi => {
            discover_baidu_models(client, &provider, native_headers.as_ref()).await
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
            bedrock_discovered_model(id, profile.inference_profile_name, aliases, true)
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
                supports_image,
            )
        })
        .collect()
}

fn bedrock_discovered_model(
    id: String,
    display_name: String,
    aliases: Vec<String>,
    supports_image: bool,
) -> ProviderModel {
    ProviderModel {
        id,
        aliases,
        display_name: Some(display_name),
        supports_image: Some(supports_image),
        supports_thinking: Some(true),
        supports_web_search: Some(false),
        supports_tool_search: Some(false),
        supports_function_tools: Some(true),
        ..ProviderModel::default()
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
    let url = provider
        .models_url()
        .context("provider models URL is not configured")?
        .clone();
    let response = provider.apply_auth(client.get(url)).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        anyhow::bail!(
            "provider {} models endpoint returned {status}: {body}",
            provider.id()
        );
    }
    let models: ModelsResponse =
        serde_json::from_str(&body).context("provider models endpoint returned invalid JSON")?;
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
    if uses_models_dev_capabilities(definition.preset_id.as_deref(), definition.id.as_str()) {
        match fetch_models_dev_provider_models(client, "opencode-go").await {
            Ok(metadata) => enrich_models_with_models_dev(&mut models, &metadata),
            Err(error) => tracing::warn!(
                provider_id = %definition.id,
                error = %format!("{error:#}"),
                "failed to enrich OpenCode Go models from models.dev"
            ),
        }
    }
    normalize_models(&mut models);
    Ok(models)
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
            (
                Some(model.architecture.as_ref().is_some_and(|architecture| {
                    architecture
                        .input_modalities
                        .iter()
                        .any(|modality| modality == "image")
                })),
                Some(true),
                Some(supports_parameter("web_search_options")),
                Some(true),
            )
        } else {
            (Some(false), Some(true), Some(false), Some(true))
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
        protocol: model.protocol,
        api_path: model.api_path,
        supports_image,
        supports_thinking,
        supports_web_search,
        supports_tool_search: Some(false),
        supports_function_tools,
        capability_probe_error: model.capability_probe_error,
        capabilities_probed_at_ms: model.capabilities_probed_at_ms,
    }
}

async fn discover_baidu_models(
    client: &Client,
    provider: &ProviderRuntime,
    native_headers: Option<&reqwest::header::HeaderMap>,
) -> anyhow::Result<Vec<ProviderModel>> {
    let url = provider
        .models_url()
        .context("provider available-models URL is not configured")?
        .clone();
    let request = match native_headers {
        Some(headers) => client.post(url).headers(headers.clone()),
        None => provider.apply_auth(client.post(url)),
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
        display_name: Some(description.clone()),
        description: Some(description),
        ratio: Some(capability.ratio),
        price_type: Some(model.price_type),
        context_window: Some(capability.context_window),
        supports_image: Some(capability.supports_image || declared_capability("image")),
        supports_thinking: Some(true),
        supports_web_search: Some(declared_capability("web_search")),
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

pub fn apply_discovered_models(
    provider: &mut ProviderDefinition,
    mut models: Vec<ProviderModel>,
) -> anyhow::Result<()> {
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
    let first_successful_refresh = provider.models_refreshed_at_ms.is_none();
    if first_successful_refresh {
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
            .map(|model| model.id.as_str())
            .collect::<HashSet<_>>();
        let available_models = models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<HashSet<_>>();
        provider
            .selected_models
            .retain(|model| available_models.contains(model.as_str()));
        // Refreshes preserve explicit user selection. New models remain visible for
        // review, while models removed upstream leave the selection immediately.
        provider.new_models = models
            .iter()
            .filter(|model| !previous_models.contains(model.id.as_str()))
            .map(|model| model.id.clone())
            .collect();
    }
    provider.cached_models = models;
    provider.models_refreshed_at_ms =
        Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64);
    provider.models_refresh_error = None;
    provider.validate()
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
    models.sort_by(|left, right| {
        left.id
            .to_ascii_lowercase()
            .cmp(&right.id.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    models.dedup_by(|left, right| left.id == right.id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_all_aws_credentials_from_provider_errors() {
        let provider = crate::provider::aws_bedrock_aksk_provider(
            "aws-bedrock",
            "AKIDEXAMPLE",
            "secret-example",
            Some("session-example".to_owned()),
            "us-east-1",
        );

        let redacted = redact_provider_error(
            &provider,
            "AKIDEXAMPLE secret-example session-example request failed",
        );

        assert_eq!(redacted, "<redacted> <redacted> <redacted> request failed");
    }

    fn model(id: &str) -> ProviderModel {
        ProviderModel {
            id: id.to_owned(),
            ..ProviderModel::default()
        }
    }

    #[test]
    fn openrouter_declarations_populate_capabilities_without_a_probe() {
        let response: ModelsResponse = serde_json::from_str(
            r#"{"data":[{"id":"vision-tool-model","name":"Vision Tool","context_length":128000,"architecture":{"input_modalities":["text","image"]},"supported_parameters":["tools","tool_choice","reasoning","web_search_options"],"reasoning":{}}]}"#,
        )
        .unwrap();

        let model = openai_model_to_provider_model(response.data.into_iter().next().unwrap(), true);

        assert_eq!(model.display_name.as_deref(), Some("Vision Tool"));
        assert_eq!(model.context_window, Some(128000));
        assert_eq!(model.supports_image, Some(true));
        assert_eq!(model.supports_thinking, Some(true));
        assert_eq!(model.supports_function_tools, Some(true));
        assert_eq!(model.supports_web_search, Some(true));
        assert_eq!(model.supports_tool_search, Some(false));
    }

    #[test]
    fn openrouter_models_without_declarations_get_safe_defaults() {
        let response: ModelsResponse =
            serde_json::from_str(r#"{"data":[{"id":"unknown"}]}"#).unwrap();

        let model = openai_model_to_provider_model(response.data.into_iter().next().unwrap(), true);

        assert_eq!(model.supports_thinking, Some(true));
        assert_eq!(model.supports_function_tools, Some(true));
        assert_eq!(model.supports_image, Some(false));
        assert_eq!(model.supports_web_search, Some(false));
        assert_eq!(model.supports_tool_search, Some(false));
    }

    #[test]
    fn undeclared_openai_compatible_models_get_safe_defaults() {
        let response: ModelsResponse =
            serde_json::from_str(r#"{"data":[{"id":"unknown"}]}"#).unwrap();

        let model =
            openai_model_to_provider_model(response.data.into_iter().next().unwrap(), false);

        assert_eq!(model.supports_image, Some(false));
        assert_eq!(model.supports_thinking, Some(true));
        assert_eq!(model.supports_function_tools, Some(true));
        assert_eq!(model.supports_web_search, Some(false));
        assert_eq!(model.supports_tool_search, Some(false));
    }

    #[test]
    fn refresh_preserves_manually_added_models_missing_upstream() {
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();
        provider.models_refreshed_at_ms = Some(1);
        provider.cached_models = vec![ProviderModel {
            id: "manual".to_owned(),
            manually_added: true,
            supports_image: Some(false),
            supports_thinking: Some(true),
            supports_web_search: Some(false),
            supports_tool_search: Some(false),
            supports_function_tools: Some(true),
            ..ProviderModel::default()
        }];
        provider.selected_models = vec!["manual".to_owned()];

        apply_discovered_models(&mut provider, vec![model("upstream")]).unwrap();

        assert_eq!(provider.selected_models, ["manual"]);
        assert!(
            provider
                .cached_models
                .iter()
                .any(|model| model.id == "manual" && model.manually_added)
        );
    }

    #[test]
    fn first_refresh_selects_first_ten_models_without_marking_them_new() {
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();
        let models = (0..12)
            .rev()
            .map(|index| model(&format!("model-{index:02}")))
            .collect();

        apply_discovered_models(&mut provider, models).unwrap();

        assert_eq!(
            provider.selected_models,
            (0..10)
                .map(|index| format!("model-{index:02}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(provider.cached_models.len(), 12);
        assert!(provider.new_models.is_empty());
    }

    #[test]
    fn later_refresh_keeps_new_models_unselected_and_removes_unavailable_models() {
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();
        provider.models_refreshed_at_ms = Some(1);
        provider.cached_models = vec![model("a"), model("gone")];
        provider.selected_models = vec!["a".to_owned(), "gone".to_owned()];

        apply_discovered_models(&mut provider, vec![model("a"), model("new")]).unwrap();

        assert_eq!(provider.selected_models, ["a"]);
        assert_eq!(provider.new_models, ["new"]);
        assert_eq!(
            provider
                .cached_models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "new"]
        );
    }

    #[test]
    fn reappearing_model_stays_unselected_after_leaving_cached_models() {
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();
        provider.models_refreshed_at_ms = Some(1);
        provider.cached_models = vec![model("flap")];
        provider.selected_models = vec!["flap".to_owned()];

        // User deselects "flap" manually; apply_model_selection clears new_models.
        provider.selected_models.clear();

        // Upstream drops "flap" entirely.
        apply_discovered_models(&mut provider, vec![model("other")]).unwrap();
        assert!(provider.selected_models.is_empty());
        assert_eq!(provider.new_models, ["other"]);

        apply_discovered_models(&mut provider, vec![model("flap"), model("other")]).unwrap();
        assert!(provider.selected_models.is_empty());
        assert_eq!(provider.new_models, ["flap"]);
    }

    #[test]
    fn discovery_then_manual_select_then_rediscover_does_not_dup_or_retag() {
        // The first discovery keeps the bootstrap behavior for a newly configured provider.
        let mut provider = crate::provider::custom_provider("custom", "key");
        provider.base_url = "https://example.test".to_owned();

        apply_discovered_models(&mut provider, vec![model("a"), model("new")]).unwrap();
        assert_eq!(provider.selected_models, ["a", "new"]);
        assert!(provider.new_models.is_empty());

        // Simulate apply_model_selection clearing new_models after a manual save
        // that keeps the same selection.
        provider.new_models.clear();

        apply_discovered_models(&mut provider, vec![model("a"), model("new")]).unwrap();
        assert_eq!(provider.selected_models, ["a", "new"]);
        assert!(provider.new_models.is_empty());
    }

    #[test]
    fn parses_application_and_system_inference_profiles() {
        let response: InferenceProfilesResponse = serde_json::from_str(
            r#"{
              "inferenceProfileSummaries": [{
                "inferenceProfileName": "Claude Opus 5",
                "inferenceProfileArn": "arn:aws:bedrock:us-east-2:123:application-inference-profile/abc",
                "inferenceProfileId": "abc",
                "models": [{"modelArn":"arn:aws:bedrock:us-east-2::foundation-model/anthropic.claude-opus-5-20251101-v1:0"}]
              }]
            }"#,
        )
        .unwrap();

        let application = inference_profile_models(response, true);

        assert_eq!(
            application[0].id,
            "arn:aws:bedrock:us-east-2:123:application-inference-profile/abc"
        );
        assert_eq!(
            application[0].aliases,
            ["anthropic.claude-opus-5-20251101-v1:0"]
        );

        let response: InferenceProfilesResponse = serde_json::from_str(
            r#"{"inferenceProfileSummaries":[{"inferenceProfileName":"US Claude Opus 5","inferenceProfileArn":"arn:system","inferenceProfileId":"us.anthropic.claude-opus-5-v1:0"}]}"#,
        )
        .unwrap();
        let system = inference_profile_models(response, false);
        assert_eq!(system[0].id, "us.anthropic.claude-opus-5-v1:0");
    }

    #[test]
    fn parses_foundation_model_catalog_capabilities() {
        let response: FoundationModelsResponse = serde_json::from_str(
            r#"{"modelSummaries":[{"modelId":"anthropic.claude-sonnet-5","modelName":"Claude Sonnet 5","providerName":"Anthropic","inputModalities":["TEXT","IMAGE"]}]}"#,
        )
        .unwrap();

        let models = foundation_model_entries(response);

        assert_eq!(models[0].id, "anthropic.claude-sonnet-5");
        assert_eq!(
            models[0].display_name.as_deref(),
            Some("Anthropic Claude Sonnet 5")
        );
        assert_eq!(models[0].supports_image, Some(true));
    }
}
