use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::Response;
use futures_util::stream;
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::compaction::{self, CompactionSummary};
use crate::error::GatewayError;
use crate::gateway::ResolvedModelRoute;
use crate::upstream::collect_response;

use super::auth::{check_gateway_auth, forward_official_headers};
use super::{AppState, *};

const COMPACTION_INSTRUCTION: &str = r#"
Summarize this conversation for continuation by another coding agent.
Return only a JSON object with exactly these fields:
goal (string), constraints (array of strings), decisions (array of strings),
files (array of strings), tool_results (array of strings), pending_work (array of strings).
Do not call tools. Do not include markdown fences. Preserve concrete commands,
file paths, unresolved errors, and decisions needed to continue the work.
"#;

pub(super) async fn compact(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let body = crate::request_body::parse_json(body).await?;
    validate_compact_request(&body)?;
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("compact request missing model".to_owned()))?;
    match state.resolve_model_route(model).await? {
        ResolvedModelRoute::Official => forward_official_compact(&state, &headers, body).await,
        ResolvedModelRoute::Provider { .. } => compact_custom_provider(&state, body).await,
        ResolvedModelRoute::Fusion { profile_id } => {
            compact_fusion(&state, &headers, body, &profile_id).await
        }
    }
}

async fn compact_fusion(
    state: &AppState,
    headers: &HeaderMap,
    mut body: Value,
    profile_id: &str,
) -> Result<Response, GatewayError> {
    let final_model = state
        .config
        .fusion_profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .map(|profile| profile.final_model.clone())
        .ok_or_else(|| GatewayError::BadRequest(format!("unknown fusion profile: {profile_id}")))?;
    body["model"] = Value::String(final_model.clone());
    match state.resolve_model_route(&final_model).await? {
        ResolvedModelRoute::Official => forward_official_compact(state, headers, body).await,
        ResolvedModelRoute::Provider { .. } => compact_custom_provider(state, body).await,
        ResolvedModelRoute::Fusion { .. } => Err(GatewayError::BadRequest(
            "fusion final model cannot reference another fusion profile".to_owned(),
        )),
    }
}

fn validate_compact_request(body: &Value) -> Result<(), GatewayError> {
    if body
        .get("model")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(GatewayError::BadRequest(
            "compact request missing model".to_owned(),
        ));
    }
    match body.get("input") {
        Some(Value::String(_)) | Some(Value::Array(_)) => Ok(()),
        Some(_) => Err(GatewayError::BadRequest(
            "compact input must be a string or array".to_owned(),
        )),
        None => Err(GatewayError::BadRequest(
            "compact request missing input".to_owned(),
        )),
    }
}

async fn compact_custom_provider(
    state: &AppState,
    mut body: Value,
) -> Result<Response, GatewayError> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("compact request missing model".to_owned()))?
        .to_owned();
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    body["stream"] = Value::Bool(true);
    body["max_output_tokens"] = Value::from(4096);
    body["tools"] = Value::Array(Vec::new());
    body.as_object_mut()
        .ok_or_else(|| GatewayError::BadRequest("compact request must be an object".to_owned()))?
        .remove("tool_choice");
    body.as_object_mut()
        .ok_or_else(|| GatewayError::BadRequest("compact request must be an object".to_owned()))?
        .remove("previous_response_id");
    let instructions = body
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|instructions| !instructions.is_empty())
        .map_or_else(
            || COMPACTION_INSTRUCTION.trim().to_owned(),
            |instructions| format!("{instructions}\n\n{}", COMPACTION_INSTRUCTION.trim()),
        );
    body["instructions"] = Value::String(instructions);

    let response = collect_response(state, body).await?;
    let summary: CompactionSummary = compaction::summary_from_value(
        serde_json::from_str(response.output_text.trim()).map_err(|error| {
            GatewayError::Upstream(format!("compact provider returned invalid JSON: {error}"))
        })?,
    )?;
    let token = compaction::encode(&model, summary)?;
    let response_id = format!("resp_compact_{}", Uuid::new_v4().simple());
    let item_id = format!("cmp_{}", Uuid::new_v4().simple());
    let response = json!({
        "id": response_id,
        "object": "response",
        "created_at": unix_seconds()?,
        "status": "completed",
        "model": model,
        "output": [{
            "type": "compaction",
            "id": item_id,
            "created_by": "codex-mixin",
            "encrypted_content": token
        }]
    });
    if stream {
        let item = response["output"][0].clone();
        let created = json!({
            "type": "response.created",
            "response": {
                "id": response["id"],
                "object": "response",
                "status": "in_progress",
                "model": response["model"]
            }
        });
        let output_done = json!({
            "type": "response.output_item.done",
            "item": item
        });
        let completed = json!({
            "type": "response.completed",
            "response": response
        });
        let events = [created, output_done, completed].into_iter().map(|event| {
            let event_name = event["type"].as_str().unwrap_or("response.completed");
            crate::sse::encode_event(event_name, &event)
                .map_err(|error| std::io::Error::other(error.to_string()))
        });
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(stream::iter(events)))
            .map_err(|error| GatewayError::Other(error.into()));
    }
    Ok(Json(response).into_response())
}

async fn forward_official_compact(
    state: &AppState,
    headers: &HeaderMap,
    body: Value,
) -> Result<Response, GatewayError> {
    let (authorization, account_id) = state.official_auth().await.map_err(GatewayError::Other)?;
    let mut url = Url::parse(&state.config.official_responses_url)
        .map_err(|error| GatewayError::Other(error.into()))?;
    let path = url.path().strip_suffix("/responses").ok_or_else(|| {
        GatewayError::Other(anyhow::anyhow!(
            "official responses URL must end with /responses"
        ))
    })?;
    url.set_path(&format!("{path}/responses/compact"));
    let request = forward_official_headers(
        state
            .client
            .post(url)
            .header(header::AUTHORIZATION, authorization)
            .header("chatgpt-account-id", account_id)
            .header(header::ACCEPT, "application/json, text/event-stream"),
        headers,
    );
    let upstream = crate::request_body::send_json(request, body).await?;
    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_owned();
    if !status.is_success() {
        let body = crate::request_body::read_error_text(upstream).await?;
        return Err(GatewayError::UpstreamStatus {
            status,
            message: format!("official compact endpoint returned {status}: {body}"),
        });
    }
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(|error| GatewayError::Other(error.into()))
}

fn unix_seconds() -> Result<u64, GatewayError> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| GatewayError::Other(error.into()))?
        .as_secs())
}
