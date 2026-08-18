use super::auth::{check_gateway_auth, forward_official_headers};
use super::*;
use crate::provider::ProviderProtocol;

mod routing;
mod transport;

#[cfg(test)]
use routing::official_live_sideband_url;
use routing::{
    RealtimeRoute, official_codex_base_url, provider_realtime_url, resolve_realtime_route,
    rewrite_custom_call_location, set_mapped_query, set_official_call_query,
};

const REALTIME_CALL_BODY_LIMIT: usize = 4 * 1024 * 1024;

pub(super) async fn realtime_ws(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, GatewayError> {
    transport::proxy_realtime_ws(state, uri, headers, ws, None).await
}

pub(super) async fn live_ws(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, GatewayError> {
    transport::proxy_realtime_ws(state, uri, headers, ws, None).await
}

pub(super) async fn live_sideband_ws(
    State(state): State<AppState>,
    Path(call_id): Path<String>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, GatewayError> {
    transport::proxy_realtime_ws(state, uri, headers, ws, Some(call_id)).await
}

pub(super) async fn realtime_call(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| GatewayError::BadRequest("missing Content-Type".to_owned()))?;
    let boundary = multipart_boundary(content_type)?;
    let body = axum::body::to_bytes(body, REALTIME_CALL_BODY_LIMIT)
        .await
        .map_err(|error| {
            GatewayError::BadRequest(format!("invalid realtime call body: {error}"))
        })?;
    let sdp = multipart_text_field(&body, &boundary, "sdp")?;
    let session = multipart_text_field(&body, &boundary, "session")?;
    let mut session = serde_json::from_str::<Value>(&session).map_err(|error| {
        GatewayError::BadRequest(format!("invalid realtime call session JSON: {error}"))
    })?;
    let requested_model = session.get("model").and_then(Value::as_str);
    let route = resolve_realtime_route(&state, requested_model).await?;
    let is_live = uri.path() == "/v1/live";
    let (upstream, provider_id) = match route {
        RealtimeRoute::Official {
            authorization,
            account_id,
        } => {
            let mut url = official_codex_base_url(&state)?;
            url.path_segments_mut()
                .map_err(|_| GatewayError::BadRequest("invalid official Codex URL".to_owned()))?
                .extend(["realtime", "calls"]);
            set_official_call_query(&mut url, uri.query(), is_live);
            let upstream = forward_official_headers(
                state
                    .client
                    .post(url)
                    .header(header::AUTHORIZATION, authorization)
                    .header("chatgpt-account-id", account_id),
                &headers,
            )
            .json(&json!({"sdp": sdp, "session": session}))
            .send()
            .await?;
            (upstream, None)
        }
        RealtimeRoute::Provider {
            provider,
            upstream_model_id,
        } => {
            let upstream_model_id = upstream_model_id.ok_or_else(|| {
                GatewayError::BadRequest("custom realtime call is missing a model".to_owned())
            })?;
            session["model"] = Value::String(upstream_model_id.to_owned());
            let mut url =
                provider_realtime_url(provider, Some(upstream_model_id), is_live, true, None)?;
            set_mapped_query(&mut url, uri.query(), None, None)?;
            let multipart = encode_realtime_multipart(&boundary, &sdp, &session);
            let request = forward_official_headers(
                provider.apply_auth_for_protocol(
                    state.client.post(url),
                    ProviderProtocol::OpenAiResponses,
                ),
                &headers,
            )
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(multipart);
            (request.send().await?, Some(provider.id()))
        }
    };
    let status = upstream.status();
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let location = upstream
        .headers()
        .get(header::LOCATION)
        .map(|location| rewrite_custom_call_location(location, provider_id, is_live))
        .transpose()?;
    let body = upstream.bytes().await?;
    if !status.is_success() {
        return Err(GatewayError::Upstream(format!(
            "realtime call endpoint returned {status}: {}",
            String::from_utf8_lossy(&body)
        )));
    }
    let mut response = Response::builder().status(status);
    if let Some(content_type) = content_type {
        response = response.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(location) = location {
        response = response.header(header::LOCATION, location);
    }
    response
        .body(Body::from(body))
        .map_err(|error| GatewayError::Other(error.into()))
}

fn encode_realtime_multipart(boundary: &str, sdp: &str, session: &Value) -> String {
    format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\nContent-Type: application/sdp\r\n\r\n{sdp}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\nContent-Type: application/json\r\n\r\n{session}\r\n--{boundary}--\r\n"
    )
}

fn multipart_boundary(content_type: &str) -> Result<String, GatewayError> {
    if !content_type
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("multipart/form-data"))
    {
        return Err(GatewayError::BadRequest(
            "realtime call Content-Type must be multipart/form-data".to_owned(),
        ));
    }
    content_type
        .split(';')
        .skip(1)
        .find_map(|parameter| {
            let (name, value) = parameter.trim().split_once('=')?;
            name.eq_ignore_ascii_case("boundary")
                .then(|| value.trim().trim_matches('"').to_owned())
        })
        .filter(|boundary| !boundary.is_empty())
        .ok_or_else(|| GatewayError::BadRequest("missing multipart boundary".to_owned()))
}

fn multipart_text_field(
    body: &[u8],
    boundary: &str,
    field_name: &str,
) -> Result<String, GatewayError> {
    let body = std::str::from_utf8(body).map_err(|error| {
        GatewayError::BadRequest(format!(
            "realtime call multipart body is not UTF-8: {error}"
        ))
    })?;
    let marker = format!("--{boundary}");
    for part in body.split(&marker) {
        let Some((headers, value)) = part.split_once("\r\n\r\n") else {
            continue;
        };
        let expected_name = format!("name=\"{field_name}\"");
        if headers.lines().any(|line| {
            line.to_ascii_lowercase()
                .starts_with("content-disposition:")
                && line.contains(&expected_name)
        }) {
            return Ok(value.strip_suffix("\r\n").unwrap_or(value).to_owned());
        }
    }
    Err(GatewayError::BadRequest(format!(
        "missing realtime call multipart field: {field_name}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_live_sideband_uses_openai_api_url() {
        let url = official_live_sideband_url("call-123").unwrap();

        assert_eq!(url.as_str(), "https://api.openai.com/v1/live/call-123");
    }
}
