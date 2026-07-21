use super::auth::{check_gateway_auth, forward_official_headers};
use super::*;

pub(super) async fn image_generations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    let routed_prompt = body
        .get("prompt")
        .and_then(Value::as_str)
        .map(|prompt| state.image_routes.resolve_prompt(prompt))
        .transpose()
        .map_err(GatewayError::BadRequest)?
        .flatten();
    if let Some(prompt) = routed_prompt {
        body["prompt"] = Value::String(prompt);
        let url = state
            .config
            .upstream_image_generation_url()
            .ok_or_else(|| {
                GatewayError::Other(anyhow::anyhow!(
                    "routed image request has no configured upstream image generation endpoint"
                ))
            })?;
        let request = state
            .client
            .post(url)
            .header(header::ACCEPT, "application/json");
        let request = match state.config.upstream_auth_header {
            UpstreamAuthHeader::AuthorizationBearer => {
                request.bearer_auth(&state.config.upstream_api_key)
            }
            UpstreamAuthHeader::XApiKey => {
                request.header("x-api-key", &state.config.upstream_api_key)
            }
        };
        let upstream = request.json(&body).send().await?;
        return proxy_image_response(upstream, "upstream").await;
    }
    let url = state
        .config
        .official_image_generation_url()
        .map_err(GatewayError::Other)?;
    forward_official_image_request(&state, &headers, &body, url).await
}

pub(super) async fn image_edits(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    if body
        .get("prompt")
        .and_then(Value::as_str)
        .map(|prompt| state.image_routes.resolve_prompt(prompt))
        .transpose()
        .map_err(GatewayError::BadRequest)?
        .flatten()
        .is_some()
    {
        return Err(GatewayError::BadRequest(
            "custom upstream image editing is not supported".to_owned(),
        ));
    }
    let url = state
        .config
        .official_image_edit_url()
        .map_err(GatewayError::Other)?;
    forward_official_image_request(&state, &headers, &body, url).await
}

async fn forward_official_image_request(
    state: &AppState,
    headers: &HeaderMap,
    body: &Value,
    url: String,
) -> Result<Response, GatewayError> {
    let (authorization, account_id) = state.official_auth().await.map_err(GatewayError::Other)?;
    let request = forward_official_headers(
        state
            .client
            .post(url)
            .header(header::AUTHORIZATION, authorization)
            .header("chatgpt-account-id", account_id)
            .header(header::ACCEPT, "application/json"),
        headers,
    );
    let upstream = request.json(body).send().await?;
    proxy_image_response(upstream, "official").await
}

async fn proxy_image_response(
    upstream: reqwest::Response,
    endpoint: &str,
) -> Result<Response, GatewayError> {
    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_owned();
    if !status.is_success() {
        let body = upstream.text().await?;
        return Err(GatewayError::Upstream(format!(
            "{endpoint} image endpoint returned {status}: {body}"
        )));
    }
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(|err| GatewayError::Other(err.into()))
}
