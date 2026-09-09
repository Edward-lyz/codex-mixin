//! Upstream transport: request preparation, provider/official sends, auth
//! runtimes, and protocol-related retries.
//!
//! This module owns no inbound server state and no routing. Callers pass the
//! exact providers and headers they need; `server` and `gateway` both call
//! into it. It never depends on `server`, `gateway`, `fusion`, or `cli`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::header;
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream};
use reqwest::Client;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::provider::auth::ducx::DucxRuntime;
use crate::provider::{ProviderProtocol, ProviderRuntime};

mod ducx;
mod official;

#[cfg(test)]
pub(crate) use official::read_codex_official_auth;
pub(crate) use official::{
    CachedOfficialAuth, FORWARDED_OFFICIAL_HEADERS, forward_official_headers,
    normalize_official_responses_body,
};

pub type AnthropicByteStream = BoxStream<'static, Result<Bytes, reqwest::Error>>;

const ANTHROPIC_FAST_BETA: &str = "fast-mode-2026-02-01";

enum AnthropicStreamDisposition {
    Ready(AnthropicByteStream),
    RetryHostedWebSearch,
}

/// Transport + auth runtime shared by every upstream send path.
#[derive(Clone)]
pub(crate) struct UpstreamAccess {
    config: Arc<GatewayConfig>,
    client: Client,
    official_auth_cache: Arc<tokio::sync::Mutex<Option<CachedOfficialAuth>>>,
    ducx_runtimes: Arc<tokio::sync::Mutex<HashMap<String, Arc<DucxRuntime>>>>,
}

impl UpstreamAccess {
    pub(crate) fn new(config: Arc<GatewayConfig>, client: Client) -> Self {
        Self {
            config,
            client,
            official_auth_cache: Arc::new(tokio::sync::Mutex::new(None)),
            ducx_runtimes: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn client(&self) -> &Client {
        &self.client
    }

    /// Send a provider Anthropic Messages request and return its SSE byte
    /// stream. Refreshes DUCX headers once when the upstream rejects cached
    /// headers with 401.
    pub(crate) async fn send_anthropic_request(
        &self,
        provider: &ProviderRuntime,
        request: &crate::anthropic::MessageRequest,
        hash_key: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let beta = if request.speed.as_deref() == Some("fast") {
            Some(match provider.definition().anthropic_beta.as_deref() {
                Some(configured)
                    if configured
                        .split(',')
                        .any(|item| item.trim() == ANTHROPIC_FAST_BETA) =>
                {
                    configured.to_owned()
                }
                Some(configured) if !configured.trim().is_empty() => {
                    format!("{configured},{ANTHROPIC_FAST_BETA}")
                }
                _ => ANTHROPIC_FAST_BETA.to_owned(),
            })
        } else {
            provider.definition().anthropic_beta.clone()
        };
        let mut refreshed_ducx_auth = false;
        loop {
            // DUCX acts as a header generator. Merge its native headers instead
            // of the stored key.
            let native = self.baidu_native_headers(provider).await?;
            let base_request = self.client.post(provider.api_url().clone());
            let mut upstream_request = match &native {
                Some(native) => base_request.headers(native.clone()),
                None if provider.aws_sigv4().is_some() => provider
                    .apply_protocol_headers(base_request, ProviderProtocol::AnthropicMessages),
                None => provider.apply_auth(base_request),
            };
            upstream_request = provider.apply_anthropic_beta(upstream_request, beta.as_deref());
            let upstream_request = provider
                .apply_session_affinity(upstream_request, hash_key)
                .header(header::ACCEPT, "text/event-stream");
            let response = if let Some(aws) = provider.aws_sigv4() {
                let prepared =
                    crate::protocol::request_body::prepare_signed_json(request.clone()).await?;
                let content_length = header::HeaderValue::from_str(&prepared.length.to_string())
                    .map_err(|error| GatewayError::Other(error.into()))?;
                let mut request = upstream_request
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::CONTENT_LENGTH, content_length)
                    .body(prepared.file)
                    .build()
                    .map_err(GatewayError::Http)?;
                crate::provider::sign_aws_request(
                    &mut request,
                    aws,
                    prepared.sha256,
                    std::time::SystemTime::now(),
                )?;
                self.client
                    .execute(request)
                    .await
                    .map_err(GatewayError::Http)
            } else {
                crate::protocol::request_body::send_json(upstream_request, request.clone()).await
            }
            .inspect_err(|error| {
                tracing::error!(
                    provider_id = provider.id(),
                    upstream_model_id = %request.model,
                    error = %crate::error::format_error_chain(error),
                    "provider messages request failed before receiving a response"
                );
            })?;
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED
                && provider.uses_ducx_loopback()
                && !refreshed_ducx_auth
            {
                tracing::warn!(
                    provider_id = provider.id(),
                    upstream_model_id = %request.model,
                    "refreshing DUCX authentication after upstream rejected cached headers"
                );
                self.invalidate_ducx_headers(provider).await?;
                refreshed_ducx_auth = true;
                continue;
            }
            if !status.is_success() {
                let body = crate::protocol::request_body::read_error_text(response).await?;
                return Err(GatewayError::UpstreamStatus {
                    status,
                    message: format!(
                        "provider {} messages endpoint returned {status}: {body}",
                        provider.id()
                    ),
                });
            }
            return Ok(response.bytes_stream().boxed());
        }
    }

    /// Send an Anthropic request, retrying a client-style `web_search` tool
    /// call once as a hosted Anthropic server tool.
    pub(crate) async fn anthropic_stream_with_web_search_retry(
        &self,
        provider: &ProviderRuntime,
        mut request: crate::anthropic::MessageRequest,
        hash_key: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let has_hosted_web_search = request.tools.iter().any(|tool| {
            tool.get("name").and_then(Value::as_str) == Some("web_search")
                && tool
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|tool_type| tool_type.starts_with("web_search_"))
        });
        let upstream = self
            .send_anthropic_request(provider, &request, hash_key)
            .await?;
        if !has_hosted_web_search {
            return Ok(upstream);
        }
        match inspect_anthropic_stream(upstream).await? {
            AnthropicStreamDisposition::Ready(upstream) => Ok(upstream),
            AnthropicStreamDisposition::RetryHostedWebSearch => {
                tracing::warn!(
                    model = %request.model,
                    "retrying client-style web_search call as an Anthropic server tool"
                );
                request.tool_choice = Some(json!({"type":"tool","name":"web_search"}));
                let retry_hash_key = hash_key.map(|_| Uuid::new_v4().to_string());
                if let Some(retry_hash_key) = retry_hash_key.as_ref()
                    && let Some(metadata) = request.metadata.as_mut().and_then(Value::as_object_mut)
                {
                    metadata.insert("session_id".to_owned(), json!(retry_hash_key));
                }
                let retry = self
                    .send_anthropic_request(
                        provider,
                        &request,
                        retry_hash_key.as_deref().or(hash_key),
                    )
                    .await?;
                match inspect_anthropic_stream(retry).await? {
                    AnthropicStreamDisposition::Ready(retry) => Ok(retry),
                    AnthropicStreamDisposition::RetryHostedWebSearch => {
                        Err(GatewayError::Upstream(format!(
                            "model {} returned a client-style web_search call after a forced hosted-tool retry",
                            request.model
                        )))
                    }
                }
            }
        }
    }
}

async fn inspect_anthropic_stream(
    mut upstream: AnthropicByteStream,
) -> Result<AnthropicStreamDisposition, GatewayError> {
    let mut buffered_chunks = Vec::new();
    let mut decoder = crate::protocol::sse::SseDecoder::default();
    while let Some(chunk) = upstream.next().await {
        let chunk = chunk?;
        let events = decoder.push(&chunk);
        buffered_chunks.push(chunk);
        let mut retry_hosted_web_search = None;
        for event in events {
            if event.data == "[DONE]" {
                retry_hosted_web_search = Some(false);
                break;
            }
            let Ok(payload) = serde_json::from_str::<Value>(&event.data) else {
                continue;
            };
            match payload.get("type").and_then(Value::as_str) {
                Some("content_block_start") => {
                    let block = payload.get("content_block").unwrap_or(&Value::Null);
                    match block.get("type").and_then(Value::as_str) {
                        Some("tool_use") => {
                            retry_hosted_web_search = Some(
                                block.get("name").and_then(Value::as_str) == Some("web_search"),
                            );
                        }
                        Some("server_tool_use") => retry_hosted_web_search = Some(false),
                        _ => {}
                    }
                }
                Some("content_block_delta") => {
                    let delta = payload.get("delta").unwrap_or(&Value::Null);
                    if delta.get("type").and_then(Value::as_str) == Some("text_delta")
                        && delta
                            .get("text")
                            .and_then(Value::as_str)
                            .is_some_and(|text| !text.is_empty())
                    {
                        retry_hosted_web_search = Some(false);
                    }
                }
                Some("message_stop" | "error") => retry_hosted_web_search = Some(false),
                _ => {}
            }
            if retry_hosted_web_search.is_some() {
                break;
            }
        }
        if let Some(retry_hosted_web_search) = retry_hosted_web_search {
            if retry_hosted_web_search {
                return Ok(AnthropicStreamDisposition::RetryHostedWebSearch);
            }
            let prefix = stream::iter(buffered_chunks.into_iter().map(Ok));
            return Ok(AnthropicStreamDisposition::Ready(
                prefix.chain(upstream).boxed(),
            ));
        }
    }
    Ok(AnthropicStreamDisposition::Ready(
        stream::iter(buffered_chunks.into_iter().map(Ok)).boxed(),
    ))
}
