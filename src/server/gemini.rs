//! Gemini Developer API HTTP adapter. OAuth Code Assist / CloudCode is not this API.
use super::auth::check_gateway_auth;
use super::*;
use crate::protocol::gemini::inbound;
use axum::extract::Query;
use std::collections::HashMap;

#[cfg(test)]
mod tests;

// Normalize the Gemini header only inside this adapter; never forward the client key.
fn auth_headers(mut headers: HeaderMap) -> Result<HeaderMap, GatewayError> {
    if let Some(key) = headers.remove("x-goog-api-key") {
        if headers.contains_key(header::AUTHORIZATION) || headers.contains_key("x-api-key") {
            return Err(GatewayError::BadRequest(
                "use only one gateway authentication header".to_owned(),
            ));
        }
        headers.insert("x-api-key", key);
    }
    Ok(headers)
}

pub(super) async fn models(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, GatewayError> {
    let headers = auth_headers(headers)?;
    check_gateway_auth(&state, &headers).await?;
    if !query.is_empty() {
        return Err(GatewayError::BadRequest(
            "model list query parameters are not supported".to_owned(),
        ));
    }
    let models = state.fetch_models().await?;
    let models: Vec<_> = models.iter().map(|model| model.id.as_str()).map(|id| json!({
        "name":format!("models/{id}"),"displayName":id,"supportedGenerationMethods":["generateContent","streamGenerateContent"]
    })).collect();
    Ok(Json(json!({"models":models})).into_response())
}

pub(super) async fn generate(
    State(state): State<AppState>,
    Path(model_action): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    let headers = auth_headers(headers)?;
    check_gateway_auth(&state, &headers).await?;
    let (model, action) = model_action
        .rsplit_once(':')
        .ok_or_else(|| GatewayError::BadRequest("missing Gemini model action".to_owned()))?;
    let streaming = match action {
        "generateContent" => false,
        "streamGenerateContent" => true,
        _ => {
            return Err(GatewayError::BadRequest(
                "unsupported Gemini action (including countTokens/CloudCode)".to_owned(),
            ));
        }
    };
    if query.keys().any(|key| key != "alt")
        || query
            .get("alt")
            .is_some_and(|alt| !streaming || alt != "sse")
    {
        return Err(GatewayError::BadRequest(
            "only alt=sse is supported; use x-goog-api-key for authentication".to_owned(),
        ));
    }
    if streaming && !query.contains_key("alt") {
        return Err(GatewayError::BadRequest(
            "streamGenerateContent requires alt=sse".to_owned(),
        ));
    }
    let body = super::request_body::parse_json(body).await?;
    let converted = inbound::to_responses(&body, model)?;
    let route = state.gateway.resolve_model_route(model).await?;
    let stream = if route == ResolvedModelRoute::Official {
        state
            .gateway
            .stream(
                RequestPlan::official(converted, Some(model.to_owned()))?,
                &headers,
            )
            .await?
    } else {
        super::responses_http::stream_custom_responses(&state, &headers, converted, route).await?
    };
    if !streaming {
        let collected = crate::gateway::collect_response_stream(stream).await?;
        return Ok(Json(inbound::from_collected(collected, model)?).into_response());
    }
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(inbound::to_gemini_stream(
            stream,
            model.to_owned(),
        )))
        .map_err(|error| GatewayError::Other(error.into()))
}
