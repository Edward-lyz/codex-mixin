use axum::http::header;
use axum::http::{HeaderMap, HeaderValue};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use serde_json::Value;

use super::UpstreamAccess;
use crate::error::GatewayError;
use crate::protocol::convert::is_gateway_thinking;

/// Headers the official backend understands from the Codex client. Kept at
/// the transport boundary; the server forwards them verbatim.
pub(crate) const FORWARDED_OFFICIAL_HEADERS: &[&str] = &[
    "openai-beta",
    "x-codex-installation-id",
    "x-codex-beta-features",
    "originator",
    "x-codex-originator",
    "x-openai-subagent",
    "x-openai-memgen-request",
    "x-codex-turn-state",
    "x-codex-turn-metadata",
    "x-codex-parent-thread-id",
    "x-oai-attestation",
    "x-responsesapi-include-timing-metrics",
    "x-openai-internal-codex-responses-lite",
    "openai-organization",
    "openai-project",
    "user-agent",
    "accept-language",
    "session-id",
    "x-session-id",
    "thread-id",
    "x-client-request-id",
    "x-request-id",
    "x-codex-window-id",
];

pub(crate) use crate::provider::auth::official::CachedOfficialAuth;

pub(crate) fn forward_official_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for &name in FORWARDED_OFFICIAL_HEADERS {
        if let Some(value) = headers.get(name) {
            request = request.header(name, value);
        }
    }
    request
}

impl UpstreamAccess {
    /// Read the official Codex auth pair from disk, cached by file mtime/size.
    pub(crate) async fn official_auth(&self) -> anyhow::Result<(HeaderValue, HeaderValue)> {
        read_codex_official_auth(&self.config.codex_auth_path, &self.official_auth_cache).await
    }

    /// Send an official Responses request with auth and forwarded client
    /// headers. Callers map status and wrap the byte stream.
    ///
    /// Use the same plaintext tool contract as custom Responses providers.
    /// Opaque native messages remain opaque, including on upstream failures.
    pub(crate) async fn send_official_responses(
        &self,
        headers: &HeaderMap,
        mut body: Value,
        prefer_websocket: bool,
    ) -> Result<(reqwest::Response, Value, bool), GatewayError> {
        let saved = crate::protocol::convert::collaboration::prepare_request(&mut body, None)?;
        let plaintext_collaboration = saved.active();
        let body = OfficialResponsesBody::new(body)?;
        let (response, body) = self
            .send_official_body(headers, body, prefer_websocket)
            .await?;
        let mut body = body.0;
        crate::protocol::convert::collaboration::restore_request(&mut body, saved);
        Ok((response, body, plaintext_collaboration))
    }

    async fn send_official_body(
        &self,
        headers: &HeaderMap,
        body: OfficialResponsesBody,
        prefer_websocket: bool,
    ) -> Result<(reqwest::Response, OfficialResponsesBody), GatewayError> {
        let (authorization, account_id) =
            self.official_auth().await.map_err(GatewayError::Other)?;
        let url = reqwest::Url::parse(&self.config.official_responses_url)
            .map_err(|error| GatewayError::Other(error.into()))?;
        if prefer_websocket
            && url.path() == "/backend-api/codex/responses"
            && body.0.get("stream").and_then(Value::as_bool) == Some(true)
            && body.0.get("previous_response_id").is_none()
            && body.0.get("input").is_some_and(Value::is_array)
            && let Some(response) = self
                .try_official_responses_websocket(
                    url.clone(),
                    headers,
                    (&authorization, &account_id),
                    serde_json::to_value(&body)
                        .map_err(|error| GatewayError::Other(error.into()))?,
                )
                .await?
        {
            return Ok((response, body));
        }
        let request = forward_official_headers(
            self.official_request(reqwest::Method::POST, url)
                .await?
                .header(header::AUTHORIZATION, authorization)
                .header("chatgpt-account-id", account_id)
                .header(header::ACCEPT, "text/event-stream"),
            headers,
        );
        self.send_json_retaining(request, body, None).await
    }
}

struct OfficialResponsesBody(Value);

impl OfficialResponsesBody {
    fn new(mut body: Value) -> Result<Self, GatewayError> {
        if let Some(object) = body.as_object_mut() {
            if object.contains_key("top_k") {
                return Err(GatewayError::BadRequest("the official Responses API does not support topK; disable topK or choose a provider that supports it".to_owned()));
            }
            if let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) {
                for item in input {
                    if !is_foreign_reasoning(item) {
                        validate_agent_message_content(item)?;
                        restore_custom_tool_call_id(item);
                    }
                }
            }
        }
        Ok(Self(body))
    }
}

impl Serialize for OfficialResponsesBody {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let Some(object) = self.0.as_object() else {
            return self.0.serialize(serializer);
        };
        let mut output = serializer.serialize_map(None)?;
        for (key, value) in object {
            if matches!(
                key.as_str(),
                "__mixin_gemini_generation_config" | "max_output_tokens"
            ) {
                continue;
            }
            if key == "input"
                && let Some(input) = value.as_array()
            {
                output.serialize_entry(key, &OfficialResponsesInput(input))?;
            } else {
                output.serialize_entry(key, value)?;
            }
        }
        output.end()
    }
}

struct OfficialResponsesInput<'a>(&'a [Value]);

impl Serialize for OfficialResponsesInput<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut output = serializer.serialize_seq(None)?;
        for item in self.0 {
            if !is_foreign_reasoning(item) {
                output.serialize_element(item)?;
            }
        }
        output.end()
    }
}

pub(crate) fn normalize_official_responses_body(mut body: Value) -> Result<Value, GatewayError> {
    if let Some(body) = body.as_object_mut() {
        if body.contains_key("top_k") {
            return Err(GatewayError::BadRequest("the official Responses API does not support topK; disable topK or choose a provider that supports it".to_owned()));
        }
        body.remove("__mixin_gemini_generation_config");
        body.remove("max_output_tokens");
        if let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) {
            input.retain(|item| !is_foreign_reasoning(item));
            for item in input {
                validate_agent_message_content(item)?;
                restore_custom_tool_call_id(item);
            }
        }
    }
    Ok(body)
}

/// Provider routes carry freeform tool calls as function calls, so a replayed
/// `custom_tool_call` can hold the upstream's `fc_` id. The official endpoint
/// requires the `ctc_` prefix for this item type.
fn restore_custom_tool_call_id(item: &mut Value) {
    if item.get("type").and_then(Value::as_str) != Some("custom_tool_call") {
        return;
    }
    let Some(id) = item.get("id").and_then(Value::as_str) else {
        return;
    };
    if id.starts_with("ctc_") {
        return;
    }
    let suffix = id.strip_prefix("fc_").unwrap_or(id);
    item["id"] = Value::String(format!("ctc_{suffix}"));
}

fn validate_agent_message_content(item: &Value) -> Result<(), GatewayError> {
    if item.get("type").and_then(Value::as_str) != Some("agent_message") {
        return Ok(());
    }
    let content = item
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| GatewayError::BadRequest("agent_message missing content".to_owned()))?;
    for part in content {
        // The official endpoint owns this envelope. Validate its shape, but
        // never infer encryption from the payload or turn it into user text.
        if part.get("type").and_then(Value::as_str) != Some("encrypted_content") {
            continue;
        }
        part.get("encrypted_content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                GatewayError::BadRequest(
                    "agent_message encrypted_content missing payload".to_owned(),
                )
            })?;
    }
    Ok(())
}

fn is_foreign_reasoning(item: &Value) -> bool {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return false;
    }
    let has_content = match item.get("content") {
        Some(Value::Array(content)) => !content.is_empty(),
        Some(Value::String(content)) => !content.is_empty(),
        Some(Value::Object(content)) => !content.is_empty(),
        Some(Value::Bool(_) | Value::Number(_)) => true,
        Some(Value::Null) | None => false,
    };
    has_content
        || item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_some_and(is_gateway_thinking)
}

pub(crate) async fn read_codex_official_auth(
    auth_path: &std::path::Path,
    cache: &tokio::sync::Mutex<Option<CachedOfficialAuth>>,
) -> anyhow::Result<(HeaderValue, HeaderValue)> {
    crate::provider::auth::official::headers(auth_path, cache).await
}
