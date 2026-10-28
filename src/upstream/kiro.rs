use anyhow::Context;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::{AnthropicByteStream, UpstreamAccess};
use crate::anthropic::MessageRequest;
use crate::error::GatewayError;
use crate::protocol::kiro::{EventDecoder, KiroSse};
use crate::provider::ProviderRuntime;

impl UpstreamAccess {
    pub(super) async fn send_kiro(
        &self,
        provider: &ProviderRuntime,
        request: &MessageRequest,
        hash_key: Option<&str>,
    ) -> Result<AnthropicByteStream, GatewayError> {
        let conversation = hash_key
            .map(|key| uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, key.as_bytes()))
            .unwrap_or_else(uuid::Uuid::new_v4)
            .to_string();
        let mut response = None;
        for force in [false, true] {
            let session = crate::provider::kiro::credentials(provider, force).await?;
            let body = crate::protocol::kiro::request_payload(
                request,
                session.profile_arn.as_deref(),
                &conversation,
            )?;
            let mut call = self
                .client
                .post(format!("https://runtime.{}.kiro.dev/", session.region))
                .bearer_auth(&session.access_token)
                .header(
                    "x-amz-target",
                    "KiroRuntimeService.GenerateAssistantResponse",
                )
                .header(reqwest::header::CONTENT_TYPE, "application/x-amz-json-1.0")
                .header(
                    reqwest::header::ACCEPT,
                    "application/vnd.amazon.eventstream",
                )
                .header("x-amzn-codewhisperer-optout", "true")
                .json(&body);
            if let Some(kind) = session.token_type {
                call = call.header("tokentype", kind);
            }
            let reply = call.send().await?;
            let unauthorized = reply.status() == reqwest::StatusCode::UNAUTHORIZED;
            response = Some(reply);
            if !unauthorized || force || !provider.definition().auth.api_key.is_empty() {
                break;
            }
        }
        let response = response
            .context("Kiro request was not executed")?
            .error_for_status()?;
        let model = request.model.clone();
        Ok(Box::pin(async_stream::stream! {
            let mut source = response.bytes_stream();
            let mut decoder = EventDecoder::default();
            let mut adapter = KiroSse::default();
            match KiroSse::start(&model) {
                Ok(event) => yield Ok(event),
                Err(error) => { yield Ok(error_event(&error)); return; },
            }
            while let Some(chunk) = source.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => { yield Err(error); return; },
                };
                if let Err(error) = decoder.push(&chunk) { yield Ok(error_event(&error)); return; }
                loop {
                    let (kind, payload) = match decoder.next_event() {
                        Ok(Some(event)) => event,
                        Ok(None) => break,
                        Err(error) => { yield Ok(error_event(&error)); return; },
                    };
                    match adapter.event(&kind, &payload) {
                        Ok(events) => for event in events { yield Ok(event); },
                        Err(error) => { yield Ok(error_event(&error)); return; },
                    }
                }
            }
            let final_events = decoder.finish().and_then(|()| adapter.finish());
            match final_events {
                Ok(events) => for event in events { yield Ok(event); },
                Err(error) => yield Ok(error_event(&error)),
            }
        }))
    }
}

fn error_event(error: &anyhow::Error) -> Bytes {
    let payload: Value = json!({"type":"error","error":{"type":"api_error","message":format!("Kiro stream failed: {error:#}")}});
    crate::protocol::sse::encode_raw_event("error", &payload.to_string())
}
