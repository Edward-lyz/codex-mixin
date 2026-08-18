use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::convert::responses_to_anthropic_with_model_reasoning_and_thinking_kind;
use crate::error::GatewayError;
use crate::gateway::{CacheShape, observe_upstream_cache_usage, record_provider_prefix};
use crate::model_reasoning::{anthropic_thinking_kind_with_advertised, prepare_upstream_reasoning};
use crate::openai_chat::responses_to_openai_chat_streaming_with_model;
use crate::openai_events::{
    map_anthropic_sse_with_image_routes, map_openai_chat_sse_with_image_routes,
};
use crate::provider::ProviderProtocol;
use crate::server::AppState;

use super::responses::map_openai_responses_sse;
use super::{ResponseStream, UpstreamRouting};

pub(crate) async fn stream_provider_response(
    state: &AppState,
    body: &Value,
    catalog_slug: &str,
    provider_id: &str,
    upstream_model_id: &str,
    routing: Option<&UpstreamRouting>,
    downstream_model: Option<&str>,
) -> Result<ResponseStream, GatewayError> {
    let provider = state
        .providers
        .provider(provider_id)
        .ok_or_else(|| GatewayError::BadRequest(format!("unknown provider: {provider_id}")))?;
    let upstream_model_id = upstream_model_id.to_owned();
    let downstream_model = downstream_model.unwrap_or(catalog_slug).to_owned();
    let downstream_body = response_metadata_request(body, &downstream_model);
    let web_search_enabled = state.web_search_enabled_for_custom_request(body);
    let protocol = provider.protocol_for_model(&upstream_model_id);
    let advertised_thinking = provider.model_supports_thinking(&upstream_model_id);
    // Both managed auth cores mint native headers. Fetch once and inject at the
    // send sites instead of the stored placeholder key.
    let baidu_native = state.baidu_native_headers(provider).await?;
    let stream = match protocol {
        ProviderProtocol::AnthropicMessages => {
            let auto_thinking_kind =
                anthropic_thinking_kind_with_advertised(&upstream_model_id, advertised_thinking);
            let reasoning = upstream_reasoning(body, advertised_thinking);
            let converted = responses_to_anthropic_with_model_reasoning_and_thinking_kind(
                body,
                Some(&upstream_model_id),
                reasoning.as_ref(),
                &state.config,
                web_search_enabled,
                provider.uses_mcp_bridge_names(&upstream_model_id),
                auto_thinking_kind,
            );
            let mut converted = converted?;
            if provider.uses_session_affinity()
                && let Some(routing) = routing
            {
                converted.request.metadata = Some(json!({"session_id": routing.hash_key}));
            }
            let observation = record_provider_prefix(
                &state.cache_shapes,
                provider.id(),
                catalog_slug,
                &upstream_model_id,
                routing,
                CacheShape::from_anthropic(&converted.request),
            );
            let upstream = state
                .anthropic_stream_with_web_search_retry(
                    provider,
                    converted.request,
                    routing.map(|routing| routing.hash_key.as_str()),
                )
                .await?;
            let upstream = observe_upstream_cache_usage(upstream, observation);
            map_anthropic_sse_with_image_routes(
                upstream,
                downstream_body,
                converted.tool_names,
                state.custom_image_routes(provider),
                provider.definition().preset_id.as_deref() == Some("baidu-oneapi"),
            )
            .boxed()
        }
        ProviderProtocol::OpenAiChat => {
            let converted =
                responses_to_openai_chat_streaming_with_model(body, Some(&upstream_model_id))?;
            let observation = record_provider_prefix(
                &state.cache_shapes,
                provider.id(),
                catalog_slug,
                &upstream_model_id,
                routing,
                CacheShape::from_openai_chat(&converted.request),
            );
            let base_request = state
                .client
                .post(provider.api_url_for_model(&upstream_model_id).clone());
            let upstream_request = match &baidu_native {
                Some(native) => base_request.headers(native.clone()),
                None => provider.apply_auth_for_protocol(base_request, protocol),
            };
            let request = provider
                .apply_session_affinity(
                    upstream_request,
                    routing.map(|routing| routing.hash_key.as_str()),
                )
                .header(reqwest::header::ACCEPT, "text/event-stream");
            let upstream = crate::request_body::send_json(request, converted.request.clone())
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
                let body = crate::request_body::read_error_text(upstream).await?;
                return Err(GatewayError::UpstreamStatus {
                    status,
                    message: format!(
                        "provider {} chat completions endpoint returned {status}: {body}",
                        provider.id()
                    ),
                });
            }
            map_openai_chat_sse_with_image_routes(
                observe_upstream_cache_usage(upstream.bytes_stream(), observation),
                downstream_body,
                converted.tool_names,
                state.custom_image_routes(provider),
            )
            .boxed()
        }
        ProviderProtocol::OpenAiResponses => {
            let mut upstream_body = body.clone();
            upstream_body["model"] = Value::String(upstream_model_id.clone());
            prepare_upstream_reasoning(&mut upstream_body, advertised_thinking);
            let observation = record_provider_prefix(
                &state.cache_shapes,
                provider.id(),
                catalog_slug,
                &upstream_model_id,
                routing,
                CacheShape::from_openai_responses(&upstream_body),
            );
            let base_request = state
                .client
                .post(provider.api_url_for_model(&upstream_model_id).clone());
            let upstream_request = match &baidu_native {
                Some(native) => base_request.headers(native.clone()),
                None => provider.apply_auth_for_protocol(base_request, protocol),
            };
            let request = provider
                .apply_session_affinity(
                    upstream_request,
                    routing.map(|routing| routing.hash_key.as_str()),
                )
                .header(reqwest::header::ACCEPT, "text/event-stream");
            let upstream = crate::request_body::send_json(request, upstream_body.clone()).await;
            let upstream = upstream.inspect_err(|error| {
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
                let body = crate::request_body::read_error_text(upstream).await?;
                return Err(GatewayError::UpstreamStatus {
                    status,
                    message: format!(
                        "provider {} responses endpoint returned {status}: {body}",
                        provider.id()
                    ),
                });
            }
            map_openai_responses_sse(
                observe_upstream_cache_usage(upstream.bytes_stream(), observation),
                upstream_model_id,
                downstream_model,
            )
        }
    };
    Ok(stream)
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
}
