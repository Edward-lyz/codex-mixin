use axum::http::header;
use axum::http::{HeaderMap, HeaderValue};
use serde_json::{Value, json};

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
    /// Native `agent_message` envelopes stay as the client wrote them. When the
    /// endpoint rejects the request, retry once with locally stored plaintext
    /// payloads materialized as `input_text`, which is how a third-party
    /// subagent's message reaches an official parent.
    pub(crate) async fn send_official_responses(
        &self,
        headers: &HeaderMap,
        body: Value,
    ) -> Result<reqwest::Response, GatewayError> {
        let body = normalize_official_responses_body(body)?;
        let response = self.send_official_body(headers, body.clone()).await?;
        if response.status().is_success() {
            return Ok(response);
        }
        let Some(fallback) = materialize_official_responses_body(&body)? else {
            return Ok(response);
        };
        tracing::warn!(
            status = %response.status(),
            "retrying official responses request with materialized agent messages"
        );
        self.send_official_body(headers, fallback).await
    }

    async fn send_official_body(
        &self,
        headers: &HeaderMap,
        body: Value,
    ) -> Result<reqwest::Response, GatewayError> {
        let (authorization, account_id) =
            self.official_auth().await.map_err(GatewayError::Other)?;
        let request = forward_official_headers(
            self.official_request(
                reqwest::Method::POST,
                reqwest::Url::parse(&self.config.official_responses_url)
                    .map_err(|error| GatewayError::Other(error.into()))?,
            )
            .await?
            .header(header::AUTHORIZATION, authorization)
            .header("chatgpt-account-id", account_id)
            .header(header::ACCEPT, "text/event-stream"),
            headers,
        );
        self.send_json(request, body, None).await
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
            }
        }
    }
    Ok(body)
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

/// Rebuild locally stored plaintext `agent_message` payloads as `input_text`.
///
/// Returns `None` when the body carries no such payload, so callers only retry
/// when materialization would actually change the request.
pub(crate) fn materialize_official_responses_body(
    body: &Value,
) -> Result<Option<Value>, GatewayError> {
    let mut materialized = body.clone();
    let Some(input) = materialized.get_mut("input").and_then(Value::as_array_mut) else {
        return Ok(None);
    };
    let mut changed = false;
    for item in input {
        changed |= materialize_agent_message_content(item)?;
    }
    Ok(changed.then_some(materialized))
}

fn materialize_agent_message_content(item: &mut Value) -> Result<bool, GatewayError> {
    if item.get("type").and_then(Value::as_str) != Some("agent_message") {
        return Ok(false);
    }
    let Some(content) = item.get_mut("content").and_then(Value::as_array_mut) else {
        return Ok(false);
    };
    let mut changed = false;
    for part in content {
        if part.get("type").and_then(Value::as_str) != Some("encrypted_content") {
            continue;
        }
        // Codex collaboration stores local plaintext in this agent-only field
        // when the subagent ran on a non-official provider. The official
        // endpoint cannot decode it, so present it as user text instead.
        let text = part
            .get("encrypted_content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                GatewayError::BadRequest(
                    "agent_message encrypted_content missing payload".to_owned(),
                )
            })?
            .to_owned();
        *part = json!({"type": "input_text", "text": text});
        changed = true;
    }
    Ok(changed)
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
