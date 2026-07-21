use super::auth::{
    check_gateway_auth, forward_official_headers, normalize_custom_model_alias,
    stable_oneapi_routing,
};
use super::*;

pub(super) async fn responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers)?;
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing model".to_owned()))?
        .to_owned();
    let route = model_route(&requested_model);
    if route == ModelRoute::Official {
        return forward_official_responses(&state, &headers, body).await;
    }
    if route == ModelRoute::Direct {
        normalize_custom_model_alias(&mut body);
    }
    let oneapi_routing = if state.config.provider_preset == ProviderPreset::BaiduOneApi {
        stable_oneapi_routing(&headers, &body)?
    } else {
        None
    };
    let stream = match route {
        ModelRoute::Official => unreachable!("official route returned above"),
        ModelRoute::Direct => {
            stream_response_with_options(&state, body, oneapi_routing.as_ref(), None).await?
        }
        ModelRoute::Fusion { profile_id } => {
            let profile = state
                .config
                .fusion_profiles
                .iter()
                .find(|profile| profile.id == profile_id)
                .ok_or_else(|| {
                    GatewayError::BadRequest(format!("unknown fusion profile: {profile_id}"))
                })?
                .clone();
            if should_fuse_turn(&body) {
                FusionEngine::new(&state, &profile)
                    .with_headers(headers.clone())
                    .stream_with_routing(body, oneapi_routing)
            } else {
                body["stream"] = Value::Bool(true);
                FusionEngine::new(&state, &profile)
                    .with_headers(headers.clone())
                    .stream_final_continuation(body, oneapi_routing.as_ref())
                    .await?
            }
        }
    };
    let stream = Body::from_stream(stream);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(stream)
        .map_err(|err| GatewayError::Other(err.into()))
}

async fn forward_official_responses(
    state: &AppState,
    headers: &HeaderMap,
    body: Value,
) -> Result<Response, GatewayError> {
    let (authorization, account_id) = state.official_auth().await.map_err(GatewayError::Other)?;
    let upstream = forward_official_headers(
        state
            .client
            .post(&state.config.official_responses_url)
            .header(header::AUTHORIZATION, authorization)
            .header("chatgpt-account-id", account_id)
            .header(header::ACCEPT, "text/event-stream"),
        headers,
    )
    .json(&body)
    .send()
    .await?;
    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("text/event-stream")
        .to_owned();
    if !status.is_success() {
        let body = upstream.text().await.unwrap_or_default();
        return Err(GatewayError::Upstream(format!(
            "official responses endpoint returned {status}: {body}"
        )));
    }
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(|err| GatewayError::Other(err.into()))
}

pub(crate) async fn stream_official_response(
    state: &AppState,
    headers: &HeaderMap,
    body: Value,
) -> Result<ResponseStream, GatewayError> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("official")
        .to_owned();
    let (authorization, account_id) = state.official_auth().await.map_err(GatewayError::Other)?;
    let upstream = forward_official_headers(
        state
            .client
            .post(&state.config.official_responses_url)
            .header(header::AUTHORIZATION, authorization)
            .header("chatgpt-account-id", account_id)
            .header(header::ACCEPT, "text/event-stream"),
        headers,
    )
    .json(&body)
    .send()
    .await?;
    let status = upstream.status();
    if !status.is_success() {
        let body = upstream.text().await.unwrap_or_default();
        return Err(GatewayError::Upstream(format!(
            "official responses endpoint returned {status}: {body}"
        )));
    }
    let stream = async_stream::stream! {
        let mut upstream = upstream.bytes_stream();
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(bytes) => yield Ok::<Bytes, Infallible>(bytes),
                Err(error) => {
                    let error = json!({"message":error.to_string(),"type":"server_error"});
                    let event = encode_event(
                        "response.failed",
                        &json!({
                            "type":"response.failed",
                            "response":{
                                "id":format!("resp_{}", Uuid::new_v4().simple()),
                                "object":"response",
                                "status":"failed",
                                "model":model,
                                "error":error,
                                "output":[]
                            },
                            "error":error
                        }),
                    )
                    .expect("official failure event is serializable");
                    yield Ok(event);
                    break;
                }
            }
        }
    };
    Ok(stream.boxed())
}
