use std::time::Duration;

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

/// Upstreams with session affinity, such as Baidu OneAPI, reject `/v1/messages`
/// probes without a stable client session. Every probe run shares one
/// identifier: the routing hash travels as `metadata.session_id` and as the
/// provider's affinity header, mirroring the gateway forwarding path.
struct ProbeSession {
    hash_key: String,
}

impl ProbeSession {
    fn new() -> Self {
        let session_id = format!("capability-probe-{}", uuid::Uuid::new_v4().simple());
        Self {
            hash_key: uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, session_id.as_bytes())
                .to_string(),
        }
    }
}

/// Everything one probe run needs, built once per protocol so the baseline and
/// feature requests share the same session.
struct ProbeTarget<'a> {
    provider: &'a ProviderRuntime,
    protocol: ProviderProtocol,
    url: &'a Url,
    request_limit: &'a Semaphore,
    native_headers: Option<&'a HeaderMap>,
    session: Option<&'a ProbeSession>,
}

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
        provider.api_url_for_model(model),
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
    url: &Url,
    request_limit: &Semaphore,
    native_headers: Option<&HeaderMap>,
) -> ProtocolCapabilities {
    let api_path = url.path();
    let session = provider.uses_session_affinity().then(ProbeSession::new);
    let target = ProbeTarget {
        provider,
        protocol,
        url,
        request_limit,
        native_headers,
        session: session.as_ref(),
    };
    let baseline = send_probe(client, &target, probe_body(protocol, model, None)).await;
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
            &target,
            probe_body(protocol, model, Some(ProbeFeature::Image)),
        ),
        send_probe(
            client,
            &target,
            probe_body(protocol, model, Some(ProbeFeature::Thinking)),
        ),
        send_probe(
            client,
            &target,
            probe_body(protocol, model, Some(ProbeFeature::FunctionTools)),
        ),
        send_probe(
            client,
            &target,
            probe_body(protocol, model, Some(ProbeFeature::ToolSearch)),
        ),
        send_probe(
            client,
            &target,
            probe_body(protocol, model, Some(ProbeFeature::WebSearch)),
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
            let mut tool = function_tool();
            // The Responses API requires an explicit tool type. Without it the
            // upstream rejects the probe and function tools look unsupported.
            tool["type"] = json!("function");
            body["tools"] = json!([tool]);
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
            // Anthropic requires max_tokens to exceed the thinking budget.
            body["max_tokens"] = json!(1025);
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

async fn send_probe(client: &Client, target: &ProbeTarget<'_>, mut body: Value) -> ProbeOutcome {
    let permit = target
        .request_limit
        .acquire()
        .await
        .expect("provider capability semaphore was closed");
    if let Some(session) = target.session {
        body["metadata"] = json!({"session_id": session.hash_key.as_str()});
    }
    let request = match target.native_headers {
        Some(headers) => client.post(target.url.clone()).headers(headers.clone()),
        None => target
            .provider
            .apply_auth_for_protocol(client.post(target.url.clone()), target.protocol),
    };
    let request = target
        .provider
        .apply_session_affinity(
            request,
            target.session.map(|session| session.hash_key.as_str()),
        )
        .json(&body)
        .timeout(PROBE_TIMEOUT);
    let outcome = match request.send().await {
        Ok(response) if response.status().is_success() => validate_probe_stream(response).await,
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let detail = format!(
                "{:?} {} returned {status}: {}",
                target.protocol,
                target.url.path(),
                truncate(&body)
            );
            ProbeOutcome {
                status: classify_status(status),
                error: Some(detail),
            }
        }
        Err(error) => ProbeOutcome::indeterminate(format!(
            "{:?} {} request failed: {error}",
            target.protocol,
            target.url.path()
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

fn truncate(value: &str) -> String {
    value.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        Router,
        extract::State,
        http::{Uri, header},
        response::IntoResponse,
        routing::any,
    };

    use super::*;
    use crate::provider::{ProviderRegistry, custom_provider};

    /// Affinity header and `metadata.session_id` captured per probe request.
    type CapturedSessions = Arc<Mutex<Vec<(String, String)>>>;

    async fn record_probe_path(
        State(paths): State<Arc<Mutex<Vec<String>>>>,
        uri: Uri,
    ) -> impl IntoResponse {
        paths.lock().unwrap().push(uri.path().to_owned());
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            "data: {\"type\":\"response.completed\"}\n\n",
        )
    }

    async fn record_probe_session(
        State(captured): State<CapturedSessions>,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> impl IntoResponse {
        let hash_key = headers
            .get("x-hash-key")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let session_id = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|body| body["metadata"]["session_id"].as_str().map(str::to_owned))
            .unwrap_or_default();
        captured.lock().unwrap().push((hash_key, session_id));
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            "data: {\"type\":\"response.completed\"}\n\n",
        )
    }

    #[tokio::test]
    async fn capability_probe_does_not_repeat_base_path() {
        let paths = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .fallback(any(record_probe_path))
            .with_state(Arc::clone(&paths));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut definition = custom_provider("custom", "secret");
        definition.base_url = format!("http://{address}/api/coding/v3");
        definition.api_path = "/responses".to_owned();
        definition.protocol = ProviderProtocol::OpenAiResponses;
        let registry = ProviderRegistry::new(vec![definition]).unwrap();
        let provider = registry.provider("custom").unwrap();
        assert_eq!(
            provider.api_url_for_model("glm-5.3-flash").path(),
            "/api/coding/v3/responses"
        );

        let capabilities = probe_model(
            &Client::new(),
            provider,
            "glm-5.3-flash",
            1,
            &Semaphore::new(6),
            None,
        )
        .await;
        server.abort();

        assert_eq!(
            capabilities.selected_protocol,
            Some(ProviderProtocol::OpenAiResponses)
        );
        let paths = paths.lock().unwrap();
        assert_eq!(paths.len(), 6);
        assert!(paths.iter().all(|path| path == "/api/coding/v3/responses"));
    }

    #[tokio::test]
    async fn session_affinity_probes_share_one_session() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .fallback(any(record_probe_session))
            .with_state(Arc::clone(&captured));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut definition = custom_provider("custom", "secret");
        definition.base_url = format!("http://{address}");
        definition.api_path = "/responses".to_owned();
        definition.protocol = ProviderProtocol::OpenAiResponses;
        definition.request_policy.session_affinity_header = Some("x-hash-key".to_owned());
        let registry = ProviderRegistry::new(vec![definition]).unwrap();
        let provider = registry.provider("custom").unwrap();

        let capabilities = probe_model(
            &Client::new(),
            provider,
            "model-a",
            1,
            &Semaphore::new(6),
            None,
        )
        .await;
        server.abort();

        assert_eq!(
            capabilities.selected_protocol,
            Some(ProviderProtocol::OpenAiResponses)
        );
        let captured = captured.lock().unwrap();
        assert_eq!(
            captured.len(),
            6,
            "baseline and feature probes share one session"
        );
        let (hash_key, session_id) = captured.first().unwrap();
        assert!(!hash_key.is_empty());
        assert_eq!(hash_key, session_id);
        assert!(captured.iter().all(|entry| entry.0 == *hash_key));
    }

    #[test]
    fn the_responses_function_tool_probe_declares_its_tool_type() {
        let body = responses_body("model-a", Some(ProbeFeature::FunctionTools));

        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "codex_mixin_probe_noop");
        assert!(body["tools"][0]["parameters"].is_object());
    }

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
        // Anthropic rejects thinking when max_tokens does not exceed the budget.
        assert_eq!(messages["max_tokens"], json!(1025));
        assert!(messages.get("max_output_tokens").is_none());
    }
}
