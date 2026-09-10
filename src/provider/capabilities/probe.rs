use std::time::Duration;

use anyhow::Context;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url, header::HeaderMap};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::protocol::sse::SseDecoder;
use crate::provider::{ProviderProtocol, ProviderRuntime};

use super::types::{CapabilityStatus, ModelCapabilities, ProtocolCapabilities};

const PROBE_TIMEOUT: Duration = Duration::from_secs(12);
const PROBE_PROMPT: &str = "Reply with OK. Do not perform any action unless a tool is provided.";
const IMAGE_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

pub(super) async fn probe_model(
    client: &Client,
    provider: &ProviderRuntime,
    model: &str,
    probed_at_ms: u64,
    request_limit: &Semaphore,
    native_headers: Option<&HeaderMap>,
) -> ModelCapabilities {
    let protocol = provider.protocol_for_model(model);
    let selected = probe_protocol(
        client,
        provider,
        model,
        protocol,
        provider.api_url_for_model(model).path(),
        request_limit,
        native_headers,
    )
    .await;
    let supported = selected.baseline == CapabilityStatus::Supported;
    ModelCapabilities {
        model: model.to_owned(),
        selected_protocol: supported.then_some(selected.protocol),
        selected_api_path: supported.then(|| selected.api_path.clone()),
        protocols: vec![selected],
        probed_at_ms,
        last_probe_error: None,
    }
}

async fn probe_protocol(
    client: &Client,
    provider: &ProviderRuntime,
    model: &str,
    protocol: ProviderProtocol,
    api_path: &str,
    request_limit: &Semaphore,
    native_headers: Option<&HeaderMap>,
) -> ProtocolCapabilities {
    let url = match endpoint_url(&provider.definition().base_url, api_path) {
        Ok(url) => url,
        Err(error) => {
            return ProtocolCapabilities {
                protocol,
                api_path: api_path.to_owned(),
                baseline: CapabilityStatus::Indeterminate,
                image_input: CapabilityStatus::Indeterminate,
                thinking: CapabilityStatus::Indeterminate,
                function_tools: CapabilityStatus::Indeterminate,
                tool_search: CapabilityStatus::Indeterminate,
                web_search: CapabilityStatus::Indeterminate,
                error: Some(error.to_string()),
            };
        }
    };
    let baseline = send_probe(
        client,
        provider,
        protocol,
        &url,
        probe_body(protocol, model, None),
        request_limit,
        native_headers,
    )
    .await;
    if baseline.status != CapabilityStatus::Supported {
        return ProtocolCapabilities {
            protocol,
            api_path: api_path.to_owned(),
            baseline: baseline.status,
            image_input: CapabilityStatus::Indeterminate,
            thinking: CapabilityStatus::Indeterminate,
            function_tools: CapabilityStatus::Indeterminate,
            tool_search: CapabilityStatus::Indeterminate,
            web_search: CapabilityStatus::Indeterminate,
            error: baseline.error,
        };
    }
    let (image, thinking, function_tools, tool_search, web_search) = tokio::join!(
        send_probe(
            client,
            provider,
            protocol,
            &url,
            probe_body(protocol, model, Some(ProbeFeature::Image)),
            request_limit,
            native_headers,
        ),
        send_probe(
            client,
            provider,
            protocol,
            &url,
            probe_body(protocol, model, Some(ProbeFeature::Thinking)),
            request_limit,
            native_headers,
        ),
        send_probe(
            client,
            provider,
            protocol,
            &url,
            probe_body(protocol, model, Some(ProbeFeature::FunctionTools)),
            request_limit,
            native_headers,
        ),
        send_probe(
            client,
            provider,
            protocol,
            &url,
            probe_body(protocol, model, Some(ProbeFeature::ToolSearch)),
            request_limit,
            native_headers,
        ),
        send_probe(
            client,
            provider,
            protocol,
            &url,
            probe_body(protocol, model, Some(ProbeFeature::WebSearch)),
            request_limit,
            native_headers,
        ),
    );
    let errors = [
        image.error,
        thinking.error,
        function_tools.error,
        tool_search.error,
        web_search.error,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    ProtocolCapabilities {
        protocol,
        api_path: api_path.to_owned(),
        baseline: CapabilityStatus::Supported,
        image_input: image.status,
        thinking: thinking.status,
        function_tools: function_tools.status,
        tool_search: tool_search.status,
        web_search: web_search.status,
        error: (!errors.is_empty()).then(|| errors.join("; ")),
    }
}

#[derive(Clone, Copy)]
enum ProbeFeature {
    Image,
    Thinking,
    FunctionTools,
    ToolSearch,
    WebSearch,
}

fn probe_body(protocol: ProviderProtocol, model: &str, feature: Option<ProbeFeature>) -> Value {
    match protocol {
        ProviderProtocol::OpenAiResponses => responses_body(model, feature),
        ProviderProtocol::OpenAiChat => chat_body(model, feature),
        ProviderProtocol::AnthropicMessages => messages_body(model, feature),
    }
}

fn responses_body(model: &str, feature: Option<ProbeFeature>) -> Value {
    let mut body = json!({
        "model": model,
        "input": [
            {"role": "developer", "content": [{"type": "input_text", "text": "Follow the user request exactly."}]},
            {"role": "user", "content": [{"type": "input_text", "text": PROBE_PROMPT}]}
        ],
        "stream": true,
        "max_output_tokens": 16
    });
    match feature {
        Some(ProbeFeature::Image) => {
            body["input"][1]["content"] = json!([
                {"type": "input_image", "image_url": IMAGE_DATA_URL},
                {"type": "input_text", "text": "Reply with OK."}
            ]);
        }
        Some(ProbeFeature::Thinking) => {
            body["max_output_tokens"] = json!(128);
            body["reasoning"] = json!({"effort": "low"});
        }
        Some(ProbeFeature::FunctionTools) => {
            body["tools"] = json!([function_tool()]);
            body["tool_choice"] = json!("auto");
        }
        Some(ProbeFeature::ToolSearch) => {
            body["tools"] = json!([{"type": "tool_search"}]);
            body["tool_choice"] = json!("auto");
        }
        Some(ProbeFeature::WebSearch) => {
            body["tools"] = json!([{"type": "web_search"}]);
            body["tool_choice"] = json!("auto");
        }
        None => {}
    }
    body
}

fn chat_body(model: &str, feature: Option<ProbeFeature>) -> Value {
    let mut body = json!({
        "model": model,
        "messages": [
            {"role": "developer", "content": "Follow the user request exactly."},
            {"role": "user", "content": PROBE_PROMPT}
        ],
        "stream": true,
        "max_tokens": 16
    });
    match feature {
        Some(ProbeFeature::Image) => {
            body["messages"][1]["content"] = json!([
                {"type": "image_url", "image_url": {"url": IMAGE_DATA_URL}},
                {"type": "text", "text": "Reply with OK."}
            ]);
        }
        Some(ProbeFeature::Thinking) => {
            body["max_tokens"] = json!(128);
            body["reasoning_effort"] = json!("low");
        }
        Some(ProbeFeature::FunctionTools) => {
            body["tools"] = json!([{"type": "function", "function": function_tool()}]);
            body["tool_choice"] = json!("auto");
        }
        Some(ProbeFeature::ToolSearch) => {
            body["tools"] = json!([{"type": "tool_search"}]);
            body["tool_choice"] = json!("auto");
        }
        Some(ProbeFeature::WebSearch) => {
            body["tools"] = json!([{"type": "web_search"}]);
            body["tool_choice"] = json!("auto");
        }
        None => {}
    }
    body
}

fn messages_body(model: &str, feature: Option<ProbeFeature>) -> Value {
    let mut body = json!({
        "model": model,
        "system": "Follow the user request exactly.",
        "messages": [{"role": "user", "content": PROBE_PROMPT}],
        "stream": true,
        "max_tokens": 16
    });
    match feature {
        Some(ProbeFeature::Image) => {
            body["messages"][0]["content"] = json!([
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": IMAGE_DATA_URL.trim_start_matches("data:image/png;base64,")}},
                {"type": "text", "text": "Reply with OK."}
            ]);
        }
        Some(ProbeFeature::Thinking) => {
            body["max_output_tokens"] = json!(1025);
            body["thinking"] = json!({"type": "enabled", "budget_tokens": 1024});
        }
        Some(ProbeFeature::FunctionTools) => {
            body["tools"] = json!([{
                "name": "codex_mixin_probe_noop",
                "description": "Capability probe. Do not call it.",
                "input_schema": {"type": "object", "properties": {}, "additionalProperties": false}
            }]);
        }
        Some(ProbeFeature::ToolSearch) => {
            body["tools"] = json!([{"type": "tool_search"}]);
        }
        Some(ProbeFeature::WebSearch) => {
            body["tools"] =
                json!([{"type": "web_search_20250305", "name": "web_search", "max_uses": 1}]);
        }
        None => {}
    }
    body
}

fn function_tool() -> Value {
    json!({
        "name": "codex_mixin_probe_noop",
        "description": "Capability probe. Do not call it.",
        "parameters": {"type": "object", "properties": {}, "additionalProperties": false}
    })
}

struct ProbeOutcome {
    status: CapabilityStatus,
    error: Option<String>,
}

impl ProbeOutcome {
    fn indeterminate(error: String) -> Self {
        Self {
            status: CapabilityStatus::Indeterminate,
            error: Some(error),
        }
    }
}

async fn send_probe(
    client: &Client,
    provider: &ProviderRuntime,
    protocol: ProviderProtocol,
    url: &Url,
    body: Value,
    request_limit: &Semaphore,
    native_headers: Option<&HeaderMap>,
) -> ProbeOutcome {
    let permit = request_limit
        .acquire()
        .await
        .expect("provider capability semaphore was closed");
    let request = match native_headers {
        Some(headers) => client.post(url.clone()).headers(headers.clone()),
        None => provider.apply_auth_for_protocol(client.post(url.clone()), protocol),
    }
    .json(&body)
    .timeout(PROBE_TIMEOUT);
    let outcome = match request.send().await {
        Ok(response) if response.status().is_success() => validate_probe_stream(response).await,
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let detail = format!(
                "{protocol:?} {} returned {status}: {}",
                url.path(),
                truncate(&body)
            );
            ProbeOutcome {
                status: classify_status(status),
                error: Some(detail),
            }
        }
        Err(error) => ProbeOutcome::indeterminate(format!(
            "{protocol:?} {} request failed: {error}",
            url.path()
        )),
    };
    drop(permit);
    outcome
}

async fn validate_probe_stream(response: reqwest::Response) -> ProbeOutcome {
    let mut decoder = SseDecoder::default();
    let mut stream = response.bytes_stream();
    let mut saw_event = false;
    let mut saw_error = false;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                return ProbeOutcome::indeterminate(format!(
                    "provider capability probe stream failed: {error}"
                ));
            }
        };
        for event in decoder.push(&chunk) {
            saw_event = true;
            let Ok(payload) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            if payload.get("type").and_then(Value::as_str) == Some("error")
                || payload.get("object").and_then(Value::as_str) == Some("error")
            {
                saw_error = true;
            }
        }
    }
    if !decoder.remaining().is_empty() {
        saw_event = true;
        if serde_json::from_slice::<Value>(decoder.remaining()).is_ok_and(|payload| {
            payload.get("type").and_then(Value::as_str) == Some("error")
                || payload.get("object").and_then(Value::as_str) == Some("error")
        }) {
            saw_error = true;
        }
    }
    if saw_error {
        ProbeOutcome::indeterminate("provider capability probe returned an error event".to_owned())
    } else if saw_event {
        ProbeOutcome {
            status: CapabilityStatus::Supported,
            error: None,
        }
    } else {
        ProbeOutcome::indeterminate("provider capability probe returned no SSE events".to_owned())
    }
}

fn classify_status(status: StatusCode) -> CapabilityStatus {
    if matches!(
        status,
        StatusCode::BAD_REQUEST
            | StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::UNPROCESSABLE_ENTITY
    ) {
        CapabilityStatus::Unsupported
    } else {
        CapabilityStatus::Indeterminate
    }
}

fn endpoint_url(base_url: &str, path: &str) -> anyhow::Result<Url> {
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    Url::parse(&format!("{}{path}", base_url.trim_end_matches('/')))
        .context("construct provider capability probe URL")
}

fn truncate(value: &str) -> String {
    value.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_http_failures_are_indeterminate() {
        assert_eq!(
            classify_status(StatusCode::TOO_MANY_REQUESTS),
            CapabilityStatus::Indeterminate
        );
        assert_eq!(
            classify_status(StatusCode::BAD_GATEWAY),
            CapabilityStatus::Indeterminate
        );
        assert_eq!(
            classify_status(StatusCode::UNAUTHORIZED),
            CapabilityStatus::Indeterminate
        );
        assert_eq!(
            classify_status(StatusCode::UNPROCESSABLE_ENTITY),
            CapabilityStatus::Unsupported
        );
    }

    #[test]
    fn thinking_probes_use_each_protocol_reasoning_field() {
        let responses = probe_body(
            ProviderProtocol::OpenAiResponses,
            "model-a",
            Some(ProbeFeature::Thinking),
        );
        let chat = probe_body(
            ProviderProtocol::OpenAiChat,
            "model-a",
            Some(ProbeFeature::Thinking),
        );
        let messages = probe_body(
            ProviderProtocol::AnthropicMessages,
            "model-a",
            Some(ProbeFeature::Thinking),
        );

        assert_eq!(responses["reasoning"], json!({"effort": "low"}));
        assert_eq!(chat["reasoning_effort"], json!("low"));
        assert_eq!(
            messages["thinking"],
            json!({"type": "enabled", "budget_tokens": 1024})
        );
    }
}
