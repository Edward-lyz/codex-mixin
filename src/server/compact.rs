use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::Response;
use futures_util::stream;
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::error::GatewayError;
use crate::gateway::ResolvedModelRoute;
use crate::protocol::compaction::{self, CompactionSummary};

use super::auth::check_gateway_auth;
use super::{AppState, *};
use crate::upstream::forward_official_headers;

const LOCAL_TRANSCRIPT_MAX_JSON_BYTES: usize = 48 * 1024;
const EARLIER_TEXT_OMITTED: &str = "[earlier text omitted]\n";
const EMPTY_LOCAL_TRANSCRIPT: &str =
    "No text messages were retained from the compacted conversation.";

pub(super) async fn compact(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    check_gateway_auth(&state, &headers).await?;
    let body = super::request_body::parse_json(body).await?;
    validate_compact_request(&body)?;
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("compact request missing model".to_owned()))?;
    match state.gateway.resolve_model_route(model).await? {
        ResolvedModelRoute::Official => forward_official_compact(&state, &headers, body).await,
        ResolvedModelRoute::Provider { .. } => compact_custom_provider(body),
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
        .map(|profile| profile.active_model_now().to_owned())
        .ok_or_else(|| GatewayError::BadRequest(format!("unknown fusion profile: {profile_id}")))?;
    body["model"] = Value::String(final_model.clone());
    match state.gateway.resolve_model_route(&final_model).await? {
        ResolvedModelRoute::Official => forward_official_compact(state, headers, body).await,
        ResolvedModelRoute::Provider { .. } => compact_custom_provider(body),
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

fn compact_custom_provider(body: Value) -> Result<Response, GatewayError> {
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("compact request missing model".to_owned()))?
        .to_owned();
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let summary = local_compaction_summary(&body, &model)?;
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
            crate::protocol::sse::encode_event(event_name, &event)
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

fn local_compaction_summary(body: &Value, model: &str) -> Result<CompactionSummary, GatewayError> {
    let input = body
        .get("input")
        .ok_or_else(|| GatewayError::BadRequest("compact request missing input".to_owned()))?;
    let mut fragments = Vec::new();
    match input {
        Value::String(text) => push_fragment(&mut fragments, "user", text),
        Value::Array(items) => {
            for item in items {
                append_local_fragment(item, model, &mut fragments)?;
            }
        }
        _ => {
            return Err(GatewayError::BadRequest(
                "compact input must be a string or array".to_owned(),
            ));
        }
    }
    let transcript = retain_recent_fragments(&fragments, LOCAL_TRANSCRIPT_MAX_JSON_BYTES);
    let transcript = fit_transcript_to_json_budget(transcript, LOCAL_TRANSCRIPT_MAX_JSON_BYTES)?;
    Ok(CompactionSummary {
        local_transcript: Some(transcript),
        goal: String::new(),
        constraints: Vec::new(),
        decisions: Vec::new(),
        files: Vec::new(),
        tool_results: Vec::new(),
        pending_work: Vec::new(),
    })
}

fn append_local_fragment(
    item: &Value,
    model: &str,
    fragments: &mut Vec<String>,
) -> Result<(), GatewayError> {
    let item_type = item.get("type").and_then(Value::as_str);
    if item_type == Some("compaction") {
        if let Some(token) = item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .filter(|token| token.starts_with(compaction::TOKEN_PREFIX))
        {
            let previous = compaction::decode(token, model)?;
            push_fragment(
                fragments,
                "previous compacted context",
                &compaction::summary_text(&previous),
            );
        }
        return Ok(());
    }
    if item_type == Some("agent_message") {
        if let Some(text) = content_text(item.get("content")) {
            push_fragment(fragments, "agent", &text);
        }
        return Ok(());
    }
    if item_type.is_none() || item_type == Some("message") {
        let Some(role @ ("user" | "assistant")) = item.get("role").and_then(Value::as_str) else {
            return Ok(());
        };
        if let Some(text) = content_text(item.get("content")) {
            push_fragment(fragments, role, &text);
        }
    }
    Ok(())
}

fn content_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => (!text.is_empty()).then(|| text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter(|block| {
                    matches!(
                        block.get("type").and_then(Value::as_str),
                        Some("input_text" | "output_text" | "text")
                    )
                })
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn push_fragment(fragments: &mut Vec<String>, role: &str, text: &str) {
    if !text.is_empty() {
        fragments.push(format!("[{role}]\n{text}"));
    }
}

fn retain_recent_fragments(fragments: &[String], max_bytes: usize) -> String {
    let mut remaining = max_bytes;
    let mut retained = Vec::new();
    for fragment in fragments.iter().rev() {
        let separator_bytes = usize::from(!retained.is_empty()) * 2;
        if remaining <= separator_bytes {
            break;
        }
        remaining -= separator_bytes;
        if fragment.len() <= remaining {
            retained.push(fragment.clone());
            remaining -= fragment.len();
            continue;
        }
        retained.push(truncate_tail(fragment, remaining));
        break;
    }
    if retained.is_empty() {
        return EMPTY_LOCAL_TRANSCRIPT.to_owned();
    }
    retained.reverse();
    retained.join("\n\n")
}

fn truncate_tail(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    if max_bytes <= EARLIER_TEXT_OMITTED.len() {
        return utf8_tail(value, max_bytes).to_owned();
    }
    format!(
        "{EARLIER_TEXT_OMITTED}{}",
        utf8_tail(value, max_bytes - EARLIER_TEXT_OMITTED.len())
    )
}

fn fit_transcript_to_json_budget(
    mut transcript: String,
    max_bytes: usize,
) -> Result<String, GatewayError> {
    while serde_json::to_vec(&transcript)?.len() > max_bytes {
        transcript = truncate_tail(&transcript, transcript.len().saturating_mul(3) / 4);
    }
    Ok(transcript)
}

fn utf8_tail(value: &str, max_bytes: usize) -> &str {
    let mut start = value.len().saturating_sub(max_bytes);
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

async fn forward_official_compact(
    state: &AppState,
    headers: &HeaderMap,
    body: Value,
) -> Result<Response, GatewayError> {
    let (authorization, account_id) = state
        .upstream
        .official_auth()
        .await
        .map_err(GatewayError::Other)?;
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
            .upstream
            .request(reqwest::Method::POST, url)
            .header(header::AUTHORIZATION, authorization)
            .header("chatgpt-account-id", account_id)
            .header(header::ACCEPT, "application/json, text/event-stream"),
        headers,
    );
    let upstream = crate::upstream::body::send_json(request, body).await?;
    let status = upstream.status();
    let content_type = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_owned();
    if !status.is_success() {
        return Err(
            crate::upstream::body::response_error(upstream, "official compact endpoint").await?,
        );
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
