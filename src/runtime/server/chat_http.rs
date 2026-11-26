use super::auth::check_gateway_auth;
use super::*;
use crate::protocol::chat_compat;

pub(super) async fn chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let body = super::request_body::parse_json(body).await?;
    let stream_requested = body.get("stream").and_then(Value::as_bool) == Some(true);
    let include_usage = body
        .pointer("/stream_options/include_usage")
        .and_then(Value::as_bool)
        == Some(true);
    let converted = chat_compat::chat_to_responses(&body)?;
    let requested_model = converted["model"]
        .as_str()
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
        .to_owned();
    let route = state.gateway.resolve_model_route(&requested_model).await?;
    let (converted, _) = crate::images::normalize_provider_images_blocking(converted).await?;
    let stream = if route == ResolvedModelRoute::Official {
        let plan = RequestPlan::official(converted, Some(requested_model.clone()))?;
        state.gateway.stream(plan, &headers).await?
    } else {
        super::responses_http::stream_custom_responses(&state, &headers, converted, route).await?
    };
    if !stream_requested {
        let collected = crate::gateway::collect_response_stream(stream).await?;
        return Ok(
            Json(chat_compat::collected_to_chat(collected, &requested_model)).into_response(),
        );
    }
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(chat_compat::responses_to_chat_stream(
            stream,
            requested_model,
            include_usage,
        )))
        .map_err(|error| GatewayError::Other(error.into()))
}
