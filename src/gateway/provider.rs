use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::config::ensure_compaction_secret;
use crate::error::GatewayError;
use crate::gateway::{
    CacheShape, GatewayExecutor, observe_upstream_cache_usage, record_provider_prefix,
};
use crate::protocol::convert::{
    WebSearchTool, responses_to_anthropic_with_model_reasoning_and_thinking_kind,
};
use crate::protocol::gemini::responses_to_gemini;
use crate::protocol::model_reasoning::{
    anthropic_thinking_kind_with_advertised, prepare_upstream_reasoning,
};
use crate::protocol::openai_chat::responses_to_openai_chat_streaming_with_model;
use crate::protocol::openai_events::{
    map_anthropic_sse_with_image_routes, map_gemini_sse_with_image_routes,
    map_openai_chat_sse_with_image_routes,
};
use crate::provider::{ProviderProtocol, ProviderRuntime};

use super::responses::map_openai_responses_sse;
use super::{ResponseStream, UpstreamRouting};

// Read lazily: only requests replaying a compaction item need the secret.
fn compaction_secret() -> Result<String, GatewayError> {
    ensure_compaction_secret().map_err(GatewayError::Other)
}

pub(crate) struct ProviderResponseRequest<'a> {
    pub(crate) body: &'a Value,
    pub(crate) catalog_slug: &'a str,
    pub(crate) provider_id: &'a str,
    pub(crate) upstream_model_id: &'a str,
    pub(crate) routing: Option<&'a UpstreamRouting>,
    pub(crate) downstream_model: Option<&'a str>,
    pub(crate) headers: &'a axum::http::HeaderMap,
    pub(crate) native_messages: Option<&'a (crate::anthropic::MessageRequest, Value)>,
}

struct ProviderRequestContext<'a> {
    executor: &'a GatewayExecutor,
    provider: &'a ProviderRuntime,
    upstream_model_id: &'a str,
    catalog_slug: &'a str,
    downstream_model: &'a str,
    routing: Option<&'a UpstreamRouting>,
    body: &'a Value,
    advertised_thinking: Option<bool>,
    web_search_enabled: bool,
    native_messages: Option<&'a (crate::anthropic::MessageRequest, Value)>,
    headers: &'a axum::http::HeaderMap,
}

pub(crate) async fn stream_provider_response(
    executor: &GatewayExecutor,
    request: ProviderResponseRequest<'_>,
) -> Result<ResponseStream, GatewayError> {
    let ProviderResponseRequest {
        body,
        catalog_slug,
        provider_id,
        upstream_model_id,
        routing,
        downstream_model,
        headers,
        native_messages,
    } = request;
    let provider = executor
        .providers
        .provider(provider_id)
        .ok_or_else(|| GatewayError::BadRequest(format!("unknown provider: {provider_id}")))?;
    let downstream_model = downstream_model.unwrap_or(catalog_slug);
    let web_search_enabled = executor.web_search_enabled_for_custom_request(body);
    let protocol = provider.protocol_for_model(upstream_model_id);
    let advertised_thinking = provider.model_supports_thinking(upstream_model_id);
    executor.require_ducx_client(provider, headers).await?;
    let ctx = ProviderRequestContext {
        executor,
        provider,
        upstream_model_id,
        catalog_slug,
        downstream_model,
        routing,
        body,
        advertised_thinking,
        web_search_enabled,
        native_messages,
        headers,
    };
    match protocol {
        ProviderProtocol::AnthropicMessages => stream_anthropic_messages(ctx).await,
        ProviderProtocol::OpenAiChat => stream_openai_chat(ctx).await,
        ProviderProtocol::OpenAiResponses => stream_openai_responses(ctx).await,
        ProviderProtocol::GoogleGemini => stream_gemini(ctx).await,
    }
}

async fn stream_anthropic_messages(
    ctx: ProviderRequestContext<'_>,
) -> Result<ResponseStream, GatewayError> {
    let ProviderRequestContext {
        executor,
        provider,
        upstream_model_id,
        catalog_slug,
        downstream_model,
        routing,
        body,
        advertised_thinking,
        web_search_enabled,
        native_messages,
        headers,
    } = ctx;
    let mut downstream_body = response_metadata_request(body, downstream_model);
    if native_messages.is_some() {
        downstream_body["__mixin_native_messages"] = Value::Bool(true);
    }
    let auto_thinking_kind =
        anthropic_thinking_kind_with_advertised(upstream_model_id, advertised_thinking);
    let reasoning = upstream_reasoning(body, advertised_thinking);
    let convert = |default_max_tokens| {
        let mut converted = responses_to_anthropic_with_model_reasoning_and_thinking_kind(
            body,
            Some(upstream_model_id),
            reasoning.as_ref(),
            if native_messages.is_some() {
                crate::config::ThinkingMode::Off
            } else {
                executor.config.thinking_mode
            },
            default_max_tokens,
            web_search_enabled.then_some(WebSearchTool {
                tool_type: &executor.config.web_search_tool_type,
                max_uses: executor.config.web_search_max_uses,
            }),
            provider.uses_mcp_bridge_names(upstream_model_id),
            auto_thinking_kind,
            &compaction_secret,
        )?;
        if let Some((native, _)) = native_messages {
            converted.request.thinking = native.thinking.clone();
            converted.request.output_config = native.output_config.clone();
            converted.request.speed = native.speed.clone();
            converted.request.metadata = native.metadata.clone();
            converted.request.tool_choice = native.tool_choice.clone();
            let mut tools = native.tools.clone();
            tools.retain(|tool| {
                !tool
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(crate::context::types::is_context_tool)
            });
            tools.extend(converted.request.tools);
            converted.request.tools = tools;
        }
        if native_messages.is_none()
            && provider.uses_session_affinity()
            && let Some(routing) = routing
        {
            // OneAPI's Anthropic-to-OpenAI conversion accepts only an
            // optional string `metadata.user_id`. `session_id` is a 400
            // ("Anthropic feature at \"metadata\" ... is not supported")
            // on models behind that conversion, such as GLM-5.3.
            // Session affinity still travels in `x-hash-key`.
            converted.request.metadata = Some(json!({"user_id": routing.hash_key}));
        }
        Ok::<_, GatewayError>(converted)
    };
    let send = |converted: crate::protocol::convert::ConvertedRequest| async move {
        let shape = CacheShape::from_anthropic(&converted.request);
        let native_body = native_messages
            .map(|(_, original)| {
                crate::protocol::convert::native_messages_body(
                    body,
                    &converted.request,
                    original,
                    &compaction_secret,
                )
            })
            .transpose()?;
        let upstream = executor
            .upstream
            .anthropic_stream_with_web_search_retry(
                provider,
                converted.request,
                routing.map(|routing| routing.hash_key.as_str()),
                native_messages
                    .and_then(|_| {
                        let values = headers
                            .get_all("anthropic-beta")
                            .iter()
                            .filter_map(|v| v.to_str().ok())
                            .map(str::trim)
                            .filter(|v| !v.is_empty())
                            .collect::<Vec<_>>();
                        (!values.is_empty()).then(|| values.join(","))
                    })
                    .as_deref(),
                native_body,
            )
            .await?;
        Ok::<_, GatewayError>((upstream, shape, converted.tool_names))
    };
    let declared_max_tokens = provider
        .model_max_output_tokens(upstream_model_id)
        .unwrap_or(executor.config.default_max_tokens);
    let default_max_tokens =
        executor
            .output_limits
            .default_for(provider.id(), upstream_model_id, declared_max_tokens);
    let converted = convert(default_max_tokens)?;
    let requested = converted.request.max_tokens;
    let defaulted = converted.max_tokens_defaulted;
    let (upstream, shape, tool_names) = match send(converted).await {
        Err(GatewayError::UpstreamStatus {
            status,
            body: error_body,
            content_type,
            context,
        }) if defaulted && status.is_client_error() => {
            if let Some(limit) =
                super::output_limit::output_limit_from_error(&error_body, requested)
            {
                tracing::warn!(
                    provider_id = provider.id(),
                    upstream_model_id = %upstream_model_id,
                    requested,
                    limit,
                    "provider rejected the default output budget; retrying with its limit"
                );
                let sent = send(convert(limit)?).await?;
                executor
                    .output_limits
                    .record(provider.id(), upstream_model_id, limit);
                sent
            } else {
                return Err(GatewayError::UpstreamStatus {
                    status,
                    body: error_body,
                    content_type,
                    context,
                });
            }
        }
        result => result?,
    };
    let observation = record_provider_prefix(
        &executor.cache_shapes,
        provider.id(),
        catalog_slug,
        upstream_model_id,
        routing,
        shape,
    );
    let upstream = observe_upstream_cache_usage(upstream, observation);
    Ok(map_anthropic_sse_with_image_routes(
        upstream,
        downstream_body,
        tool_names,
        executor.custom_image_routes(provider),
        provider.definition().preset_id.as_deref() == Some("baidu-oneapi"),
    )
    .boxed())
}

async fn stream_openai_chat(
    ctx: ProviderRequestContext<'_>,
) -> Result<ResponseStream, GatewayError> {
    let ProviderRequestContext {
        executor,
        provider,
        upstream_model_id,
        catalog_slug,
        downstream_model,
        routing,
        body,
        advertised_thinking,
        ..
    } = ctx;
    let baidu_native = executor.upstream.baidu_native_headers(provider).await?;
    let downstream_body = response_metadata_request(body, downstream_model);
    let mut upstream_body = body.clone();
    prepare_upstream_reasoning(&mut upstream_body, advertised_thinking);
    let converted = responses_to_openai_chat_streaming_with_model(
        &upstream_body,
        Some(upstream_model_id),
        &compaction_secret,
    )?;
    let observation = record_provider_prefix(
        &executor.cache_shapes,
        provider.id(),
        catalog_slug,
        upstream_model_id,
        routing,
        CacheShape::from_openai_chat(&converted.request),
    );
    let upstream = executor
        .upstream
        .send_provider_json(
            provider,
            ProviderProtocol::OpenAiChat,
            upstream_model_id,
            routing.map(|routing| routing.hash_key.as_str()),
            baidu_native.as_ref(),
            converted.request.clone(),
        )
        .await
        .inspect_err(|error| {
            tracing::error!(
                provider_id = provider.id(),
                catalog_slug = %catalog_slug,
                upstream_model_id = %upstream_model_id,
                error = %crate::error::format_error_chain(error),
                "provider chat completions request failed before receiving a response"
            );
        })?;
    let status = upstream.status();
    if !status.is_success() {
        return Err(crate::upstream::body::response_error(
            upstream,
            format!("provider {} chat completions endpoint", provider.id()),
        )
        .await?);
    }
    Ok(map_openai_chat_sse_with_image_routes(
        observe_upstream_cache_usage(upstream.bytes_stream(), observation),
        downstream_body,
        converted.tool_names,
        executor.custom_image_routes(provider),
    )
    .boxed())
}

async fn stream_openai_responses(
    ctx: ProviderRequestContext<'_>,
) -> Result<ResponseStream, GatewayError> {
    let ProviderRequestContext {
        executor,
        provider,
        upstream_model_id,
        catalog_slug,
        downstream_model,
        routing,
        body,
        advertised_thinking,
        ..
    } = ctx;
    let baidu_native = executor.upstream.baidu_native_headers(provider).await?;
    let mut upstream_body = body.clone();
    upstream_body["model"] = Value::String(upstream_model_id.to_owned());
    prepare_upstream_reasoning(&mut upstream_body, advertised_thinking);
    let observation = record_provider_prefix(
        &executor.cache_shapes,
        provider.id(),
        catalog_slug,
        upstream_model_id,
        routing,
        CacheShape::from_openai_responses(&upstream_body),
    );
    let upstream = executor
        .upstream
        .send_provider_json(
            provider,
            ProviderProtocol::OpenAiResponses,
            upstream_model_id,
            routing.map(|routing| routing.hash_key.as_str()),
            baidu_native.as_ref(),
            upstream_body.clone(),
        )
        .await
        .inspect_err(|error| {
            tracing::error!(
                provider_id = provider.id(),
                catalog_slug = %catalog_slug,
                upstream_model_id = %upstream_model_id,
                error = %crate::error::format_error_chain(error),
                "provider responses request failed before receiving a response"
            );
        })?;
    let status = upstream.status();
    if !status.is_success() {
        return Err(crate::upstream::body::response_error(
            upstream,
            format!("provider {} responses endpoint", provider.id()),
        )
        .await?);
    }
    Ok(map_openai_responses_sse(
        observe_upstream_cache_usage(upstream.bytes_stream(), observation),
        upstream_model_id.to_owned(),
        downstream_model.to_owned(),
    ))
}

fn is_thought_signature_error(error_text: &str) -> bool {
    error_text.contains("thought signature") || error_text.contains("thought_signature")
}

fn build_gemini_payload(
    provider: &ProviderRuntime,
    upstream_model_id: &str,
    mut request: Value,
) -> Value {
    let api_url = provider.api_url_for_model(upstream_model_id);
    if api_url.path().contains("/v1internal:") {
        // AGY's private endpoint rejects mixed tools even with the public API flag.
        // Preserve client functions; native Google Search requires a separate request.
        if provider.definition().preset_id.as_deref() == Some("agy")
            && let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut)
            && tools
                .iter()
                .any(|tool| tool.get("functionDeclarations").is_some())
            && tools.iter().any(|tool| tool.get("googleSearch").is_some())
        {
            tools.retain(|tool| tool.get("googleSearch").is_none());
            if let Some(request) = request.as_object_mut() {
                request.remove("toolConfig");
            }
            tracing::warn!(
                provider_id = provider.id(),
                model = upstream_model_id,
                "AGY native Google Search omitted from mixed-tool request; function tools preserved"
            );
        }
        json!({
            "project": "aicode-consumers",
            "model": upstream_model_id,
            "requestType": "agent",
            "userAgent": "antigravity",
            "requestId": format!("agent/{}", uuid::Uuid::new_v4()),
            "request": request
        })
    } else {
        request
    }
}

async fn handle_gemini_bad_request(
    executor: &GatewayExecutor,
    provider: &ProviderRuntime,
    upstream_model_id: &str,
    routing: Option<&UpstreamRouting>,
    payload: &Value,
    upstream: reqwest::Response,
) -> Result<reqwest::Response, GatewayError> {
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .cloned();
    let error_text = crate::upstream::body::read_error_text(upstream).await?;
    if is_thought_signature_error(&error_text) {
        tracing::warn!(
            provider_id = provider.id(),
            upstream_model_id = %upstream_model_id,
            "upstream rejected thought signature, retrying with sentinel bypass"
        );
        let mut retry_payload = payload.clone();
        crate::protocol::gemini::sanitize_thought_signatures_in_gemini_payload(&mut retry_payload);
        let retry_headers = executor.upstream.agy_native_headers(provider).await?;
        executor
            .upstream
            .send_provider_json(
                provider,
                ProviderProtocol::GoogleGemini,
                upstream_model_id,
                routing.map(|routing| routing.hash_key.as_str()),
                retry_headers.as_ref(),
                retry_payload,
            )
            .await
    } else {
        Err(crate::error::GatewayError::UpstreamStatus {
            status: reqwest::StatusCode::BAD_REQUEST,
            body: error_text,
            content_type,
            context: format!("provider {} gemini endpoint", provider.id()),
        })
    }
}

async fn stream_gemini(ctx: ProviderRequestContext<'_>) -> Result<ResponseStream, GatewayError> {
    let ProviderRequestContext {
        executor,
        provider,
        upstream_model_id,
        catalog_slug,
        downstream_model,
        routing,
        body,
        advertised_thinking,
        ..
    } = ctx;
    let agy_native = executor.upstream.agy_native_headers(provider).await?;
    let mut upstream_body = body.clone();
    if upstream_body.get("max_output_tokens").is_none() {
        upstream_body["max_output_tokens"] = json!(
            provider
                .model_max_output_tokens(upstream_model_id)
                .unwrap_or(executor.config.default_max_tokens)
        );
    }
    prepare_upstream_reasoning(&mut upstream_body, advertised_thinking);
    let converted = responses_to_gemini(&upstream_body, Some(upstream_model_id))?;
    let mut payload = build_gemini_payload(provider, upstream_model_id, converted.request);
    let observation = record_provider_prefix(
        &executor.cache_shapes,
        provider.id(),
        catalog_slug,
        upstream_model_id,
        routing,
        CacheShape::from_gemini(payload.get("request").unwrap_or(&payload)),
    );
    if let Some(headers) = agy_native.as_ref() {
        payload["project"] = json!(
            executor
                .upstream
                .agy_native_project(provider, headers)
                .await?
        );
    }
    let mut upstream = executor
        .upstream
        .send_provider_json(
            provider,
            ProviderProtocol::GoogleGemini,
            upstream_model_id,
            routing.map(|routing| routing.hash_key.as_str()),
            agy_native.as_ref(),
            payload.clone(),
        )
        .await
        .inspect_err(|error| {
            tracing::error!(
                provider_id = provider.id(),
                catalog_slug = %catalog_slug,
                upstream_model_id = %upstream_model_id,
                error = %crate::error::format_error_chain(error),
                "provider gemini generate content request failed before receiving a response"
            );
        })?;
    if upstream.status() == reqwest::StatusCode::UNAUTHORIZED && agy_native.is_some() {
        tracing::warn!(
            provider_id = provider.id(),
            "refreshing agy OAuth token after upstream rejected credentials"
        );
        crate::provider::auth::agy::invalidate_agy_cache().await;
        let refreshed_headers = executor.upstream.agy_native_headers(provider).await?;
        upstream = executor
            .upstream
            .send_provider_json(
                provider,
                ProviderProtocol::GoogleGemini,
                upstream_model_id,
                routing.map(|routing| routing.hash_key.as_str()),
                refreshed_headers.as_ref(),
                payload.clone(),
            )
            .await?;
    }
    if upstream.status() == reqwest::StatusCode::BAD_REQUEST {
        upstream = handle_gemini_bad_request(
            executor,
            provider,
            upstream_model_id,
            routing,
            &payload,
            upstream,
        )
        .await?;
    }
    if !upstream.status().is_success() {
        return Err(crate::upstream::body::response_error(
            upstream,
            format!("provider {} gemini endpoint", provider.id()),
        )
        .await?);
    }
    // Gemini SSE mapper constructs response lifecycle events using downstream_body.
    let downstream_body = response_metadata_request(body, downstream_model);
    Ok(map_gemini_sse_with_image_routes(
        observe_upstream_cache_usage(upstream.bytes_stream(), observation),
        downstream_body,
        converted.tool_names,
        executor.custom_image_routes(provider),
    )
    .boxed())
}

fn response_metadata_request(body: &Value, downstream_model: &str) -> Value {
    const RESPONSE_FIELDS: &[&str] = &[
        "instructions",
        "max_output_tokens",
        "parallel_tool_calls",
        "previous_response_id",
        "reasoning",
        "store",
        "temperature",
        "text",
        "tool_choice",
        "tools",
        "top_p",
        "truncation",
        "user",
        "metadata",
    ];
    let mut request = serde_json::Map::with_capacity(RESPONSE_FIELDS.len() + 1);
    request.insert(
        "model".to_owned(),
        Value::String(downstream_model.to_owned()),
    );
    for &field in RESPONSE_FIELDS {
        if let Some(value) = body.get(field) {
            request.insert(field.to_owned(), value.clone());
        }
    }
    Value::Object(request)
}

fn upstream_reasoning(body: &Value, advertised_thinking: Option<bool>) -> Option<Value> {
    if advertised_thinking == Some(false) {
        return None;
    }
    let mut reasoning = body.get("reasoning")?.clone();
    if reasoning.get("effort").and_then(Value::as_str) == Some("ultra") {
        reasoning["effort"] = Value::String("max".to_owned());
    }
    Some(reasoning)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_metadata_does_not_copy_input_history() {
        let body = json!({
            "model": "upstream",
            "instructions": "system",
            "input": [{"role": "user", "content": "large history"}],
            "tools": [{"type": "function", "name": "lookup"}],
            "stream": true
        });

        let metadata = response_metadata_request(&body, "catalog");

        assert_eq!(metadata["model"], "catalog");
        assert_eq!(metadata["instructions"], "system");
        assert_eq!(metadata["tools"], body["tools"]);
        assert!(metadata.get("input").is_none());
        assert!(metadata.get("stream").is_none());
    }

    #[test]
    fn upstream_reasoning_drops_unsupported_and_normalizes_ultra() {
        let supported = json!({"reasoning":{"effort":"ultra","summary":"auto"}});
        assert_eq!(
            upstream_reasoning(&supported, Some(true)).unwrap()["effort"],
            "max"
        );
        assert_eq!(
            upstream_reasoning(&supported, Some(true)).unwrap()["summary"],
            "auto"
        );

        let unsupported = json!({"reasoning":{"effort":"ultra"}});
        assert!(upstream_reasoning(&unsupported, Some(false)).is_none());
    }
    #[test]
    fn agy_tool_policy_is_scoped() {
        for (preset, path, native_agy) in [
            ("agy", "/v1internal:streamGenerateContent", true),
            ("custom", "/v1internal:streamGenerateContent", false),
            ("agy", "/v1beta/models/{model}:streamGenerateContent", false),
        ] {
            let mut definition = crate::provider::agy_provider("fixture", "key");
            definition.preset_id = Some(preset.to_owned());
            definition.api_path = path.to_owned();
            let registry = crate::provider::ProviderRegistry::new(vec![definition]).unwrap();
            let provider = registry.provider("fixture").unwrap();
            for (functions, search) in [(true, true), (true, false), (false, true)] {
                let mut tools = Vec::new();
                if functions {
                    tools.push(json!({"type":"function", "name":"read_file", "parameters":{"type":"object"}}));
                }
                if search {
                    tools.push(json!({"type":"web_search"}));
                }
                let converted = responses_to_gemini(
                    &json!({"model":"gemini-3.8-flash-high", "input":"hello", "tools":tools}),
                    None,
                )
                .unwrap();
                let payload =
                    build_gemini_payload(provider, "gemini-3.8-flash-high", converted.request);
                let request = payload.get("request").unwrap_or(&payload);
                let tools = request["tools"].as_array().unwrap();
                assert_eq!(
                    tools
                        .iter()
                        .any(|tool| tool.get("functionDeclarations").is_some()),
                    functions
                );
                assert_eq!(
                    tools.iter().any(|tool| tool.get("googleSearch").is_some()),
                    search && !(native_agy && functions)
                );
                assert_eq!(
                    request.get("toolConfig").is_some(),
                    functions && search && !native_agy
                );
            }
        }
    }
}
