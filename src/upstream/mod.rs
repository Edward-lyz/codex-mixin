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

pub(crate) mod body;
mod ducx;
#[cfg(test)]
mod ech_tests;
mod official;

#[cfg(test)]
pub(crate) use official::read_codex_official_auth;
pub(crate) use official::{
    CachedOfficialAuth, FORWARDED_OFFICIAL_HEADERS, forward_official_headers,
    materialize_official_responses_body, normalize_official_responses_body,
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
    official_ech: Arc<crate::ech::OfficialEch>,
    official_direct_client: Arc<tokio::sync::OnceCell<Client>>,
    official_auth_cache: Arc<tokio::sync::Mutex<Option<CachedOfficialAuth>>>,
    ducx_runtimes: Arc<tokio::sync::Mutex<HashMap<String, Arc<DucxRuntime>>>>,
}

impl UpstreamAccess {
    pub(crate) fn new(config: Arc<GatewayConfig>, client: Client) -> Self {
        Self {
            config,
            client,
            official_ech: Arc::new(crate::ech::OfficialEch::default()),
            official_direct_client: Arc::new(tokio::sync::OnceCell::new()),
            official_auth_cache: Arc::new(tokio::sync::Mutex::new(None)),
            ducx_runtimes: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    pub(crate) async fn send_provider_json<T>(
        &self,
        provider: &ProviderRuntime,
        protocol: ProviderProtocol,
        upstream_model_id: &str,
        hash_key: Option<&str>,
        native_headers: Option<&reqwest::header::HeaderMap>,
        body: T,
    ) -> Result<reqwest::Response, GatewayError>
    where
        T: serde::Serialize + Send + 'static,
    {
        let base = self
            .request(
                reqwest::Method::POST,
                provider.api_url_for_model(upstream_model_id).clone(),
            )
            .await?;
        let authenticated = match native_headers {
            Some(headers) => base.headers(headers.clone()),
            None => provider.apply_auth_for_protocol(base, protocol),
        };
        let request = provider
            .apply_session_affinity(authenticated, hash_key)
            .header(reqwest::header::ACCEPT, "text/event-stream");
        let request = body::prepare_json(request, body).await?;
        self.send_official(request).await
    }

    pub(crate) async fn request(
        &self,
        method: reqwest::Method,
        url: reqwest::Url,
    ) -> Result<reqwest::RequestBuilder, GatewayError> {
        if crate::ech::is_official_url(&url) {
            return self.official_request(method, url).await;
        }
        Ok(self.client.request(method, url))
    }

    pub(crate) fn official_ech_active(&self) -> bool {
        self.config.official_ech_proxy && !self.official_ech.disabled()
    }

    pub(crate) fn official_is_direct(&self) -> bool {
        self.official_ech.disabled() || self.config.official_ech_fallback_reason.is_some()
    }

    async fn official_default_client(&self) -> Result<Client, GatewayError> {
        if !self.official_is_direct() {
            return Ok(self.client.clone());
        }
        let client = self
            .official_direct_client
            .get_or_try_init(|| async {
                Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .timeout(self.config.request_timeout)
                    .build()
            })
            .await?;
        Ok(client.clone())
    }

    pub(crate) async fn official_request(
        &self,
        method: reqwest::Method,
        url: reqwest::Url,
    ) -> Result<reqwest::RequestBuilder, GatewayError> {
        if self.official_ech_active() {
            let attempt = tokio::time::timeout(
                std::time::Duration::from_secs(20),
                self.official_ech
                    .connection(&url, self.config.request_timeout),
            )
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result);
            match attempt {
                Ok(connection) => return Ok(connection.client.request(method, url)),
                Err(error) => self
                    .official_ech
                    .disable(error)
                    .await
                    .map_err(GatewayError::Other)?,
            }
        }
        Ok(self.official_default_client().await?.request(method, url))
    }

    pub(crate) async fn send_official(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, GatewayError> {
        let (client, request) = request.build_split();
        let request = request?;
        let official = crate::ech::is_official_url(request.url());
        let retry = request.try_clone();
        let result = client.execute(request).await;
        match result {
            Err(error) if official && self.official_ech_active() && error.is_connect() => {
                self.official_ech
                    .disable(error.without_url().into())
                    .await
                    .map_err(GatewayError::Other)?;
                if let Some(retry) = retry {
                    return self
                        .official_default_client()
                        .await?
                        .execute(retry)
                        .await
                        .map_err(GatewayError::Http);
                }
                Err(GatewayError::Other(anyhow::anyhow!(
                    "ECH connection failed and was disabled; streaming request was not replayed, retry using direct access"
                )))
            }
            other => other.map_err(GatewayError::Http),
        }
    }

    pub(crate) async fn official_tls(
        &self,
        url: &reqwest::Url,
    ) -> anyhow::Result<Option<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>> {
        if !self.official_ech_active() {
            return Ok(None);
        }
        let connect = async {
            let connection = self
                .official_ech
                .connection(url, self.config.request_timeout)
                .await?;
            let host = crate::ech::official_host(url)?;
            let (stream, _) = crate::ech::connect_tls(host, &connection, true).await?;
            Ok::<_, anyhow::Error>(stream)
        };
        match tokio::time::timeout(std::time::Duration::from_secs(20), connect)
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
        {
            Ok(stream) => Ok(Some(stream)),
            Err(error) => {
                self.official_ech.disable(error).await?;
                Ok(None)
            }
        }
    }

    /// Send a provider Anthropic Messages request and return its SSE byte
    /// stream. Refreshes DUCX headers once when the upstream rejects cached
    /// headers with 401.
    pub(crate) async fn send_anthropic_request(
        &self,
        provider: &ProviderRuntime,
        request: &crate::anthropic::MessageRequest,
        hash_key: Option<&str>,
        client_beta: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let provider_beta = if request.speed.as_deref() == Some("fast") {
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
        let beta = merge_anthropic_beta(provider_beta.as_deref(), client_beta);
        let mut refreshed_ducx_auth = false;
        loop {
            // DUCX acts as a header generator. Merge its native headers instead
            // of the stored key.
            let native = self.baidu_native_headers(provider).await?;
            // Use the model-level endpoint like the OpenAI protocols so a
            // per-model api_path is honored for Anthropic Messages too.
            let base_request = self
                .request(
                    reqwest::Method::POST,
                    provider.api_url_for_model(&request.model).clone(),
                )
                .await?;
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
                let prepared = body::prepare_signed_json(request.clone()).await?;
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
                let prepared = body::prepare_json(upstream_request, request.clone()).await?;
                self.send_official(prepared).await
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
                return Err(body::response_error(
                    response,
                    format!("provider {} messages endpoint", provider.id()),
                )
                .await?);
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
        client_beta: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let has_hosted_web_search = request.tools.iter().any(|tool| {
            tool.get("name").and_then(Value::as_str) == Some("web_search")
                && tool
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|tool_type| tool_type.starts_with("web_search_"))
        });
        let upstream = self
            .send_anthropic_request(provider, &request, hash_key, client_beta)
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
                    metadata.insert("user_id".to_owned(), json!(retry_hash_key));
                }
                let retry = self
                    .send_anthropic_request(
                        provider,
                        &request,
                        retry_hash_key.as_deref().or(hash_key),
                        client_beta,
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

/// Merge provider-configured Anthropic betas with the ones the client sent.
///
/// Values keep configuration order, drop blanks and repeats, and stay absent
/// when neither side asked for a beta.
fn merge_anthropic_beta(configured: Option<&str>, client: Option<&str>) -> Option<String> {
    let mut merged: Vec<&str> = Vec::new();
    for value in [configured, client].into_iter().flatten() {
        for item in value.split(',') {
            let item = item.trim();
            if !item.is_empty() && !merged.contains(&item) {
                merged.push(item);
            }
        }
    }
    (!merged.is_empty()).then(|| merged.join(","))
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
