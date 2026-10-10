use super::*;
use crate::clients::claude_desktop::{route_id, route_model};
use crate::gateway_access::GatewayClient;

async fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<(), GatewayError> {
    super::auth::check_gateway_auth(state, headers).await?;
    if state.config.gateway_client_keys.authenticate(headers) != Some(GatewayClient::ClaudeDesktop)
    {
        return Err(GatewayError::Unauthorized);
    }
    Ok(())
}

pub(super) async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    check_auth(&state, &headers).await?;
    let mut body = super::request_body::parse_json(body).await?;
    let route = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("missing Desktop model route".to_owned()))?;
    let model = route_model(route)
        .map_err(|_| GatewayError::BadRequest("invalid Claude Desktop model route".to_owned()))?;
    body["model"] = json!(model);
    super::messages_http::messages_body(state, headers, body).await
}

pub(super) async fn models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    check_auth(&state, &headers).await?;
    let models = model_catalog(&state).await?;
    Ok(Json(json!({"data": models, "has_more": false})).into_response())
}

pub(super) async fn model(
    State(state): State<AppState>,
    Path(requested): Path<String>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    check_auth(&state, &headers).await?;
    let models = model_catalog(&state).await?;
    match models.into_iter().find(|model| model["id"] == requested) {
        Some(model) => Ok(Json(model).into_response()),
        None => Ok((StatusCode::NOT_FOUND, Json(json!({
            "type": "error", "error": {"type":"not_found_error", "message":"unknown Desktop model route"}
        }))).into_response()),
    }
}

async fn model_catalog(state: &AppState) -> Result<Vec<Value>, GatewayError> {
    let mut models = state
        .fetch_models()
        .await?
        .into_iter()
        .map(|model| {
            model_info(
                &model.id,
                model.display_name.as_deref().unwrap_or(&model.id),
            )
        })
        .collect::<Vec<_>>();
    if !state.config.accept_codex_oauth
        || !tokio::fs::try_exists(&state.config.codex_auth_path).await?
    {
        return Ok(models);
    }
    // The CLI profile includes selected official models as well as providers.
    // Discovery must advertise the same official IDs, without a network refresh.
    let official_ids = match &state.config.official_selected_models {
        Some(ids) => ids.clone(),
        None => {
            let path = crate::config::stored_config_path().with_file_name("official-models.json");
            let bytes = match tokio::fs::read(path).await {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(models),
                Err(error) => return Err(error.into()),
            };
            let catalog: Value = serde_json::from_slice(&bytes)?;
            catalog
                .get("models")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    GatewayError::Other(anyhow::anyhow!("official catalog has no models array"))
                })?
                .iter()
                .filter(|model| model["visibility"] != "hide")
                .map(|model| {
                    model["slug"].as_str().map(str::to_owned).ok_or_else(|| {
                        GatewayError::Other(anyhow::anyhow!("official catalog model has no slug"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    for id in official_ids {
        let model = model_info(&id, &id);
        if !models.iter().any(|listed| listed["id"] == model["id"]) {
            models.push(model);
        }
    }
    Ok(models)
}

fn model_info(id: &str, label: &str) -> Value {
    json!({
        "id": route_id(id), "display_name": label,
        "type": "model", "created_at": "2024-01-01T00:00:00Z",
    })
}
