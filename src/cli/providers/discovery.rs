use std::collections::HashSet;
use std::time::Duration;

use anyhow::Context;
use codex_mixin::anthropic::ModelsResponse;
use codex_mixin::provider::{
    ProviderModelSource, ProviderProtocol, ProviderQuotaParser, ProviderRegistry,
};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};

use super::super::config_input::normalize_base_url;
use super::super::status::{QuotaUsageSummary, quota_usage};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct DiscoveredQuotaEndpoint {
    pub(super) url: reqwest::Url,
    pub(super) parser: ProviderQuotaParser,
    pub(super) currency: Option<String>,
    pub(super) usage: QuotaUsageSummary,
}

pub(super) async fn discover_custom_quota(
    client: &reqwest::Client,
    provider: &codex_mixin::provider::ProviderDefinition,
) -> anyhow::Result<Option<DiscoveredQuotaEndpoint>> {
    let registry = ProviderRegistry::new(vec![provider.clone()])?;
    let runtime = registry
        .provider(&provider.id)
        .expect("newly constructed provider registry contains the custom provider");
    let probes = stream::iter(
        custom_quota_candidate_urls(&provider.base_url)?
            .into_iter()
            .map(|url| {
                let runtime = &runtime;
                async move {
                    let response = runtime
                        .apply_auth(
                            client
                                .get(url.clone())
                                .header(reqwest::header::ACCEPT, "application/json")
                                .timeout(Duration::from_secs(5)),
                        )
                        .send()
                        .await
                        .ok()?;
                    if !response.status().is_success() {
                        return None;
                    }
                    let body = response.bytes().await.ok()?;
                    let value: serde_json::Value = serde_json::from_slice(&body).ok()?;
                    let parser = ProviderQuotaParser::Generic;
                    let usage = quota_usage(parser, &value).ok()?;
                    Some(DiscoveredQuotaEndpoint {
                        url,
                        parser,
                        currency: quota_currency(&value),
                        usage,
                    })
                }
            }),
    )
    .buffer_unordered(4)
    .filter_map(|result| async move { result });
    tokio::pin!(probes);
    let discovered = tokio::time::timeout(Duration::from_secs(8), probes.next())
        .await
        .unwrap_or(None);
    Ok(discovered)
}

fn custom_quota_candidate_urls(base_url: &str) -> anyhow::Result<Vec<reqwest::Url>> {
    let base = reqwest::Url::parse(base_url)?;
    let mut origin = base.clone();
    origin.set_path("/");
    origin.set_query(None);
    origin.set_fragment(None);
    let paths = [
        // New API's canonical read-only token endpoint includes the trailing slash.
        "api/usage/token/",
        "api/usage/token",
        // Sub2API exposes key-level quota, subscription, and wallet usage here.
        "v1/usage",
        "api/v1/credits",
        "v1/credits",
        "credits",
        "api/usage",
        "usage",
        "api/token/usage",
        "api/user/usage",
        "v1/dashboard/billing/usage",
        "dashboard/billing/usage",
        "api/user/self",
    ];
    let mut bases = vec![origin];
    if base.path() != "/" {
        let mut base = base;
        let path = format!("{}/", base.path().trim_end_matches('/'));
        base.set_path(&path);
        base.set_query(None);
        base.set_fragment(None);
        bases.push(base);
    }
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for base in bases {
        for path in paths {
            let url = base.join(path)?;
            if seen.insert(url.as_str().to_owned()) {
                urls.push(url);
            }
        }
    }
    Ok(urls)
}

fn quota_currency(value: &serde_json::Value) -> Option<String> {
    [
        "/currency",
        "/unit",
        "/data/currency",
        "/data/unit",
        "/quota/currency",
        "/quota/unit",
        "/data/quota/currency",
        "/data/quota/unit",
        "/usage/currency",
        "/usage/unit",
        "/data/usage/currency",
        "/data/usage/unit",
    ]
    .into_iter()
    .find_map(|pointer| value.pointer(pointer).and_then(serde_json::Value::as_str))
    .map(str::trim)
    .filter(|currency| {
        currency.len() == 3 && currency.bytes().all(|byte| byte.is_ascii_alphabetic())
    })
    .map(str::to_ascii_uppercase)
}

pub(super) fn apply_discovered_quota(
    provider: &mut codex_mixin::provider::ProviderDefinition,
    discovered: &DiscoveredQuotaEndpoint,
) {
    provider.quota_url = Some(discovered.url.to_string());
    provider.quota_parser = discovered.parser;
    provider.quota_currency = discovered.currency.clone();
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InferredCustomProviderEndpoint {
    pub(super) base_url: String,
    pub(super) protocol: ProviderProtocol,
    pub(super) api_path: String,
    pub(super) models_path: String,
    pub(super) path_explicit: bool,
}

pub(super) fn infer_custom_provider_endpoint(
    raw_url: &str,
) -> anyhow::Result<InferredCustomProviderEndpoint> {
    let normalized = normalize_base_url(raw_url.to_owned())?;
    let mut url = reqwest::Url::parse(&normalized)?;
    anyhow::ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "custom provider URL must not contain a query or fragment"
    );
    let path = url.path().trim_end_matches('/').to_owned();
    let candidates = [
        (
            "/v1/chat/completions",
            ProviderProtocol::OpenAiChat,
            "/v1/chat/completions",
            "/v1/models",
        ),
        (
            "/chat/completions",
            ProviderProtocol::OpenAiChat,
            "/chat/completions",
            "/models",
        ),
        (
            "/v1/messages",
            ProviderProtocol::AnthropicMessages,
            "/v1/messages",
            "/v1/models",
        ),
        (
            "/messages",
            ProviderProtocol::AnthropicMessages,
            "/messages",
            "/models",
        ),
        (
            "/v1/responses",
            ProviderProtocol::OpenAiResponses,
            "/v1/responses",
            "/v1/models",
        ),
        (
            "/responses",
            ProviderProtocol::OpenAiResponses,
            "/responses",
            "/models",
        ),
    ];
    let matched = candidates
        .iter()
        .find_map(|(suffix, protocol, api_path, models_path)| {
            path.strip_suffix(suffix).map(|base_path| {
                (
                    base_path.to_owned(),
                    *protocol,
                    (*api_path).to_owned(),
                    (*models_path).to_owned(),
                    true,
                )
            })
        });
    let (base_path, protocol, api_path, models_path, path_explicit) =
        matched.unwrap_or_else(|| {
            let base_path = path.to_owned();
            (
                base_path,
                // Prefer the standard Responses API until the /v1/models gate
                // confirms that protocol probing is safe to continue.
                ProviderProtocol::OpenAiResponses,
                "/v1/responses".to_owned(),
                "/v1/models".to_owned(),
                false,
            )
        });
    url.set_path(if base_path.is_empty() {
        "/"
    } else {
        &base_path
    });
    let base_url = url.to_string().trim_end_matches('/').to_owned();
    Ok(InferredCustomProviderEndpoint {
        base_url,
        protocol,
        api_path,
        models_path,
        path_explicit,
    })
}

pub(super) async fn detect_custom_provider_protocol(
    provider: &codex_mixin::provider::ProviderDefinition,
) -> anyhow::Result<Option<InferredCustomProviderEndpoint>> {
    let registry = ProviderRegistry::new(vec![provider.clone()])?;
    let runtime = registry
        .provider(&provider.id)
        .expect("newly constructed provider registry contains the custom provider");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let mut last_models_error = None;
    for (models_path, versioned) in [("/v1/models", true), ("/models", false)] {
        let models_url = endpoint_join(&provider.base_url, models_path)?;
        let models_valid = match probe_custom_models_endpoint(&client, runtime, models_url).await {
            Ok(valid) => valid,
            Err(error) => {
                last_models_error = Some(error);
                false
            }
        };
        if !models_valid {
            continue;
        }
        for (protocol, api_path, body) in custom_protocol_probe_candidates(versioned) {
            let url = match endpoint_join(&provider.base_url, api_path) {
                Ok(url) => url,
                Err(_) => continue,
            };
            if protocol_endpoint_available(&client, runtime, protocol, url, &body).await {
                return Ok(Some(InferredCustomProviderEndpoint {
                    base_url: provider.base_url.clone(),
                    protocol,
                    api_path: api_path.to_owned(),
                    models_path: models_path.to_owned(),
                    path_explicit: false,
                }));
            }
        }
    }
    if let Some(error) = last_models_error {
        return Err(
            error.context("custom provider automatic discovery failed for /v1/models and /models")
        );
    }
    anyhow::bail!(
        "custom provider automatic discovery found neither a valid /v1/models nor /models endpoint"
    )
}

fn custom_protocol_probe_candidates(
    versioned: bool,
) -> [(ProviderProtocol, &'static str, serde_json::Value); 3] {
    let (responses, messages, chat) = if versioned {
        ("/v1/responses", "/v1/messages", "/v1/chat/completions")
    } else {
        ("/responses", "/messages", "/chat/completions")
    };
    [
        (
            ProviderProtocol::OpenAiResponses,
            responses,
            protocol_probe_body(ProviderProtocol::OpenAiResponses),
        ),
        (
            ProviderProtocol::AnthropicMessages,
            messages,
            protocol_probe_body(ProviderProtocol::AnthropicMessages),
        ),
        (
            ProviderProtocol::OpenAiChat,
            chat,
            protocol_probe_body(ProviderProtocol::OpenAiChat),
        ),
    ]
}

fn protocol_probe_body(protocol: ProviderProtocol) -> serde_json::Value {
    // Incomplete bodies intentionally avoid paid generation. A real endpoint
    // still answers with 4xx validation or auth errors; missing routes 404.
    match protocol {
        ProviderProtocol::OpenAiResponses => json!({
            "model": "codex-mixin-protocol-probe",
            "stream": false
        }),
        ProviderProtocol::AnthropicMessages => json!({
            "model": "codex-mixin-protocol-probe",
            "max_tokens": 1
        }),
        ProviderProtocol::OpenAiChat => json!({
            "model": "codex-mixin-protocol-probe",
            "stream": false
        }),
    }
}

async fn protocol_endpoint_available(
    client: &reqwest::Client,
    runtime: &codex_mixin::provider::ProviderRuntime,
    protocol: ProviderProtocol,
    url: reqwest::Url,
    body: &serde_json::Value,
) -> bool {
    let request = runtime
        .apply_auth_for_protocol(client.post(url), protocol)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(Duration::from_secs(5))
        .json(body);
    let response = match request.send().await {
        Ok(response) => response,
        Err(_) => return false,
    };
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = match response.text().await {
        Ok(body) => body,
        Err(_) => return false,
    };
    if matches!(status, 403 | 404 | 501 | 502 | 504) {
        return false;
    }
    protocol_probe_body_matches(protocol, &content_type, &body)
}

async fn probe_custom_models_endpoint(
    client: &reqwest::Client,
    runtime: &codex_mixin::provider::ProviderRuntime,
    url: reqwest::Url,
) -> anyhow::Result<bool> {
    let response = runtime
        .apply_auth(client.get(url.clone()))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .with_context(|| format!("requesting custom provider models endpoint {url}"))?;
    let status = response.status();
    let body = response.text().await?;
    if is_json_api_error(&body) {
        anyhow::bail!("custom provider models endpoint returned {status}: {body}");
    }
    if !status.is_success() {
        return Ok(false);
    }
    let models: ModelsResponse = match serde_json::from_str(&body) {
        Ok(models) => models,
        Err(_) => return Ok(false),
    };
    if models.data.iter().any(|model| model.id.trim().is_empty()) {
        return Ok(false);
    }
    Ok(true)
}

pub(super) fn protocol_probe_body_matches(
    protocol: ProviderProtocol,
    content_type: &str,
    body: &str,
) -> bool {
    let trimmed = body.trim_start();
    if content_type
        .split(';')
        .next()
        .is_some_and(|value| value.eq_ignore_ascii_case("text/html"))
        || trimmed.starts_with("<!doctype html")
        || trimmed.starts_with("<html")
    {
        return false;
    }
    let is_event_stream = content_type
        .split(';')
        .next()
        .is_some_and(|value| value.eq_ignore_ascii_case("text/event-stream"))
        || body
            .lines()
            .any(|line| line.trim_start().starts_with("data:"));
    if is_event_stream {
        return body
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("data:").map(str::trim))
            .any(|data| {
                serde_json::from_str::<Value>(data)
                    .ok()
                    .is_some_and(|value| protocol_probe_value_matches(protocol, &value))
            });
    }
    serde_json::from_str::<Value>(body)
        .ok()
        .is_some_and(|value| protocol_probe_value_matches(protocol, &value))
}

fn protocol_probe_value_matches(protocol: ProviderProtocol, value: &Value) -> bool {
    if is_json_api_error_value(value) {
        return true;
    }
    let Some(object) = value.as_object() else {
        return false;
    };
    match protocol {
        ProviderProtocol::OpenAiResponses => {
            let response = object
                .get("response")
                .and_then(Value::as_object)
                .unwrap_or(object);
            response
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty())
                && (response.contains_key("output")
                    || response.get("object").and_then(Value::as_str) == Some("response")
                    || object
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.starts_with("response.")))
        }
        ProviderProtocol::AnthropicMessages => {
            let message = object
                .get("message")
                .and_then(Value::as_object)
                .unwrap_or(object);
            message.get("type").and_then(Value::as_str) == Some("message")
                && message.contains_key("content")
        }
        ProviderProtocol::OpenAiChat => {
            object
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty())
                && object.contains_key("choices")
        }
    }
}

fn is_json_api_error(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_some_and(|value| is_json_api_error_value(&value))
}

fn is_json_api_error_value(value: &Value) -> bool {
    value.get("error").is_some()
        || (value.get("type").and_then(Value::as_str) == Some("error")
            && value.get("message").is_some())
}

pub(super) fn endpoint_join(base_url: &str, path: &str) -> anyhow::Result<reqwest::Url> {
    let mut base_url = reqwest::Url::parse(base_url)?;
    let base_path = base_url.path().trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let endpoint_path = if base_path.is_empty()
        || base_path == "/"
        || path == base_path
        || path.starts_with(&format!("{base_path}/"))
    {
        path
    } else if let Some(base_without_version) = base_path.strip_suffix("/v1")
        && (path == "/v1" || path.starts_with("/v1/"))
    {
        format!("{base_without_version}{path}")
    } else {
        format!("{base_path}{path}")
    };
    base_url.set_path(&endpoint_path);
    Ok(base_url)
}

pub(super) fn apply_inferred_custom_endpoint(
    provider: &mut codex_mixin::provider::ProviderDefinition,
    endpoint: InferredCustomProviderEndpoint,
) {
    provider.base_url = endpoint.base_url;
    provider.protocol = endpoint.protocol;
    provider.api_path = endpoint.api_path;
    provider.model_source = ProviderModelSource::OpenAiCompatible {
        path: endpoint.models_path,
    };
    provider.anthropic_version =
        (endpoint.protocol == ProviderProtocol::AnthropicMessages).then(|| "2023-06-01".to_owned());
}
