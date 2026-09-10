use std::collections::HashSet;
use std::time::Duration;

use anyhow::Context;
use codex_mixin::anthropic::ModelsResponse;
use codex_mixin::provider::{
    ProviderModelSource, ProviderProtocol, ProviderQuotaParser, ProviderRegistry,
    redact_provider_error,
};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};

use super::super::config_input::normalize_base_url;
use codex_mixin::provider::{QuotaUsageSummary, quota_usage};

const CUSTOM_PROTOCOL_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InferredCustomProviderEndpoint {
    pub(super) base_url: String,
    pub(super) protocol: ProviderProtocol,
    pub(super) api_path: String,
    pub(super) models_path: String,
    pub(super) path_explicit: bool,
}

fn format_probe_failure(path: &str, error: &anyhow::Error) -> String {
    let is_timeout = error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout)
    });
    let is_network_error = error
        .chain()
        .any(|cause| cause.downcast_ref::<reqwest::Error>().is_some());
    if is_network_error {
        let kind = if is_timeout {
            "timeout"
        } else {
            "network error"
        };
        format!("{path}: {kind}: {error:#}")
    } else {
        format!("{path}: {error:#}")
    }
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
    tokio::time::timeout(
        CUSTOM_PROTOCOL_PROBE_TIMEOUT,
        detect_custom_provider_protocol_inner(provider),
    )
    .await
    .map_err(|_| anyhow::anyhow!("custom provider protocol detection timed out after 5 seconds"))?
}

async fn detect_custom_provider_protocol_inner(
    provider: &codex_mixin::provider::ProviderDefinition,
) -> anyhow::Result<Option<InferredCustomProviderEndpoint>> {
    let registry = ProviderRegistry::new(vec![provider.clone()])?;
    let runtime = registry
        .provider(&provider.id)
        .expect("newly constructed provider registry contains the custom provider");
    let client = reqwest::Client::builder().build()?;
    let mut failures = Vec::new();
    let mut found_models_endpoint = false;
    for (models_path, versioned) in [("/v1/models", true), ("/models", false)] {
        let models_url = match endpoint_join(&provider.base_url, models_path) {
            Ok(url) => url,
            Err(error) => {
                failures.push(format_probe_failure(models_path, &error));
                continue;
            }
        };
        let model_id = match probe_custom_models_endpoint(&client, runtime, models_url).await {
            Ok(model_id) => model_id,
            Err(error) => {
                failures.push(format_probe_failure(models_path, &error));
                continue;
            }
        };
        found_models_endpoint = true;
        for (protocol, api_path, body) in
            custom_protocol_probe_candidates(versioned, model_id.as_deref())
        {
            let url = match endpoint_join(&provider.base_url, api_path) {
                Ok(url) => url,
                Err(error) => {
                    failures.push(format_probe_failure(api_path, &error));
                    continue;
                }
            };
            match protocol_endpoint_available(&client, runtime, protocol, url, &body).await {
                Ok(()) => {
                    return Ok(Some(InferredCustomProviderEndpoint {
                        base_url: provider.base_url.clone(),
                        protocol,
                        api_path: api_path.to_owned(),
                        models_path: models_path.to_owned(),
                        path_explicit: false,
                    }));
                }
                Err(error) => failures.push(format_probe_failure(api_path, &error)),
            }
        }
    }

    let heading = if found_models_endpoint {
        "custom provider models endpoint is valid, but automatic protocol detection failed"
    } else {
        "custom provider automatic discovery found neither a valid /v1/models nor /models endpoint"
    };
    let details = failures.join("; ");
    let error = if details.is_empty() {
        heading.to_owned()
    } else {
        format!("{heading}: {details}")
    };
    anyhow::bail!("{}", redact_provider_error(provider, &error));
}

fn custom_protocol_probe_candidates(
    versioned: bool,
    model_id: Option<&str>,
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
            protocol_probe_body(ProviderProtocol::OpenAiResponses, model_id),
        ),
        (
            ProviderProtocol::AnthropicMessages,
            messages,
            protocol_probe_body(ProviderProtocol::AnthropicMessages, model_id),
        ),
        (
            ProviderProtocol::OpenAiChat,
            chat,
            protocol_probe_body(ProviderProtocol::OpenAiChat, model_id),
        ),
    ]
}

fn protocol_probe_body(protocol: ProviderProtocol, model_id: Option<&str>) -> serde_json::Value {
    // Incomplete bodies intentionally avoid paid generation. A real endpoint
    // still answers with 4xx validation or auth errors; missing routes 404.
    let mut body = match protocol {
        ProviderProtocol::OpenAiResponses => json!({"stream": false}),
        ProviderProtocol::AnthropicMessages => json!({"max_tokens": 1}),
        ProviderProtocol::OpenAiChat => json!({"stream": false}),
    };
    if let Some(model_id) = model_id
        && let Some(object) = body.as_object_mut()
    {
        object.insert("model".to_owned(), Value::String(model_id.to_owned()));
    }
    body
}

async fn protocol_endpoint_available(
    client: &reqwest::Client,
    runtime: &codex_mixin::provider::ProviderRuntime,
    protocol: ProviderProtocol,
    url: reqwest::Url,
    body: &serde_json::Value,
) -> anyhow::Result<()> {
    let path = url.path().to_owned();
    let request = runtime
        .apply_auth_for_protocol(client.post(url), protocol)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(CUSTOM_PROTOCOL_PROBE_TIMEOUT)
        .json(body);
    let response = request
        .send()
        .await
        .with_context(|| format!("POST {path} for custom provider protocol probe"))?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let response_body = response
        .text()
        .await
        .with_context(|| format!("reading {path} protocol probe response"))?;
    if !probe_response_matches(protocol, status, &content_type, &response_body) {
        anyhow::bail!(
            "HTTP {status} ({})",
            response_summary(&content_type, &response_body)
        );
    }
    Ok(())
}

async fn probe_custom_models_endpoint(
    client: &reqwest::Client,
    runtime: &codex_mixin::provider::ProviderRuntime,
    url: reqwest::Url,
) -> anyhow::Result<Option<String>> {
    let path = url.path().to_owned();
    let response = runtime
        .apply_auth(client.get(url.clone()))
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .with_context(|| format!("requesting custom provider models endpoint {url}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = response
        .text()
        .await
        .with_context(|| format!("reading {path} models response"))?;
    if !status.is_success() {
        anyhow::bail!(
            "HTTP {} ({})",
            status.as_u16(),
            response_summary(&content_type, &body)
        );
    }
    let value: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(_) => {
            anyhow::bail!("HTTP {} (invalid models JSON response)", status.as_u16());
        }
    };
    if is_json_api_error_value(&value) {
        anyhow::bail!(
            "HTTP {} ({})",
            status.as_u16(),
            response_summary(&content_type, &body)
        );
    }
    let models: ModelsResponse = match serde_json::from_value(value) {
        Ok(models) => models,
        Err(_) => {
            anyhow::bail!("HTTP {} (invalid models JSON response)", status.as_u16());
        }
    };
    if models.data.iter().any(|model| model.id.trim().is_empty()) {
        anyhow::bail!(
            "HTTP {} (models list contains an empty model ID)",
            status.as_u16()
        );
    }
    Ok(models.data.first().map(|model| model.id.clone()))
}

pub(super) fn protocol_probe_body_matches(
    protocol: ProviderProtocol,
    content_type: &str,
    body: &str,
) -> bool {
    if is_html_response(content_type, body) {
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

fn probe_response_matches(
    protocol: ProviderProtocol,
    status: u16,
    content_type: &str,
    body: &str,
) -> bool {
    if is_html_response(content_type, body) {
        return false;
    }
    if matches!(status, 403 | 404 | 501 | 502 | 504) {
        return false;
    }
    if status == 422 {
        return probe_missing_field_matches(protocol, body)
            || protocol_probe_body_matches(protocol, content_type, body);
    }
    protocol_probe_body_matches(protocol, content_type, body)
}

fn probe_missing_field_matches(protocol: ProviderProtocol, body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let Some(details) = value.get("detail").and_then(Value::as_array) else {
        return false;
    };
    let required_fields = match protocol {
        ProviderProtocol::OpenAiResponses => ["input", "model"].as_slice(),
        ProviderProtocol::AnthropicMessages => ["messages", "model", "max_tokens"].as_slice(),
        ProviderProtocol::OpenAiChat => ["messages", "model"].as_slice(),
    };
    details.iter().any(|detail| {
        let missing_type = matches!(
            detail.get("type").and_then(Value::as_str),
            Some("missing" | "value_error.missing")
        );
        let Some(location) = detail.get("loc").and_then(Value::as_array) else {
            return false;
        };
        missing_type
            && location.len() == 2
            && location[0].as_str() == Some("body")
            && location[1]
                .as_str()
                .is_some_and(|field| required_fields.contains(&field))
    })
}

fn response_summary(content_type: &str, body: &str) -> String {
    if is_html_response(content_type, body) {
        return "HTML response".to_owned();
    }
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return if body.trim().is_empty() {
            "empty response".to_owned()
        } else {
            "invalid JSON response".to_owned()
        };
    };
    structured_response_summary(&value).unwrap_or_else(|| {
        if value.is_object() {
            "unrecognized JSON response".to_owned()
        } else {
            "unstructured response".to_owned()
        }
    })
}

fn structured_response_summary(value: &Value) -> Option<String> {
    if let Some(error) = value.get("error").and_then(Value::as_object) {
        let fields = ["type", "code", "message"]
            .into_iter()
            .filter_map(|field| {
                error
                    .get(field)
                    .and_then(scalar_value_text)
                    .map(|value| format!("{field}={value}"))
            })
            .collect::<Vec<_>>();
        if !fields.is_empty() {
            return Some(format!("error {}", fields.join(", ")));
        }
    }
    let details = value.get("detail").and_then(Value::as_array)?;
    let fields = details
        .iter()
        .filter_map(|detail| {
            let detail_type = detail.get("type").and_then(Value::as_str)?;
            let location = detail.get("loc").and_then(Value::as_array)?;
            let location = location
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(".");
            Some(format!("type={detail_type}, loc={location}"))
        })
        .collect::<Vec<_>>();
    if fields.is_empty() {
        None
    } else {
        Some(format!("detail [{}]", fields.join("; ")))
    }
}

fn scalar_value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn is_html_response(content_type: &str, body: &str) -> bool {
    let trimmed = body.trim_start();
    content_type
        .split(';')
        .next()
        .is_some_and(|value| value.eq_ignore_ascii_case("text/html"))
        || trimmed.starts_with("<!doctype html")
        || trimmed.starts_with("<html")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_422_requires_exact_protocol_field() {
        for (protocol, field) in [
            (ProviderProtocol::OpenAiResponses, "input"),
            (ProviderProtocol::OpenAiResponses, "model"),
            (ProviderProtocol::AnthropicMessages, "messages"),
            (ProviderProtocol::AnthropicMessages, "model"),
            (ProviderProtocol::AnthropicMessages, "max_tokens"),
            (ProviderProtocol::OpenAiChat, "messages"),
            (ProviderProtocol::OpenAiChat, "model"),
        ] {
            let body = json!({
                "detail": [{"type": "missing", "loc": ["body", field]}]
            })
            .to_string();
            assert!(probe_response_matches(
                protocol,
                422,
                "application/json",
                &body
            ));
        }
        assert!(probe_response_matches(
            ProviderProtocol::OpenAiResponses,
            422,
            "application/json",
            r#"{"detail":[{"type":"value_error.missing","loc":["body","input"]}]}"#
        ));

        for body in [
            r#"{"detail":"input is required"}"#,
            r#"{"detail":[{"type":"missing","loc":["body","messages"]}]}"#,
            r#"{"detail":[{"type":"missing","loc":["body","input","nested"]}]}"#,
            r#"{"status":"validation failed"}"#,
        ] {
            assert!(!probe_response_matches(
                ProviderProtocol::OpenAiResponses,
                422,
                "application/json",
                body
            ));
        }
        assert!(!probe_response_matches(
            ProviderProtocol::OpenAiResponses,
            422,
            "text/html",
            r#"{"detail":[{"type":"missing","loc":["body","input"]}]}"#
        ));
        assert!(probe_response_matches(
            ProviderProtocol::OpenAiResponses,
            422,
            "application/json",
            r#"{"error":{"message":"missing input"}}"#
        ));
        assert!(probe_response_matches(
            ProviderProtocol::OpenAiResponses,
            500,
            "application/json",
            r#"{"error":{"message":"upstream failure"}}"#
        ));
        for status in [403, 404, 501, 502, 504] {
            assert!(!probe_response_matches(
                ProviderProtocol::OpenAiResponses,
                status,
                "application/json",
                r#"{"error":{"message":"route unavailable"}}"#
            ));
        }
    }
}
