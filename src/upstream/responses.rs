use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::Value;

use crate::error::GatewayError;
use crate::sse::{
    SseDecoder, encode_event, encode_raw_event, event_contains_response_metadata,
    response_failed_payload,
};

use super::{CollectedResponse, ResponseStream};

pub(super) fn map_openai_responses_sse<S>(
    upstream: S,
    upstream_model: String,
    downstream_model: String,
) -> ResponseStream
where
    S: futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    async_stream::stream! {
        let mut decoder = SseDecoder::default();
        tokio::pin!(upstream);
        while let Some(chunk) = upstream.next().await {
            match chunk {
                Ok(bytes) => {
                    for event in decoder.push(&bytes) {
                        let event_name = event.event.as_deref().unwrap_or("message");
                        if event.data == "[DONE]" {
                            yield Ok(encode_raw_event(event_name, &event.data));
                            continue;
                        }
                        if !event_contains_response_metadata(event_name) {
                            yield Ok(encode_raw_event(event_name, &event.data));
                            continue;
                        }
                        match serde_json::from_str::<Value>(&event.data) {
                            Ok(mut payload) => {
                                rewrite_response_model_field(
                                    &mut payload,
                                    &upstream_model,
                                    &downstream_model,
                                );
                                yield Ok(encode_event(event_name, &payload)
                                    .expect("rewritten responses event is serializable"));
                            }
                            Err(_) => {
                                for (recovered_name, recovered_data) in
                                    split_malformed_metadata_events(event_name, &event.data)
                                {
                                    match serde_json::from_str::<Value>(&recovered_data) {
                                        Ok(mut payload) => {
                                            rewrite_response_model_field(
                                                &mut payload,
                                                &upstream_model,
                                                &downstream_model,
                                            );
                                            yield Ok(encode_event(&recovered_name, &payload)
                                                .expect("recovered responses event is serializable"));
                                        }
                                        Err(_) => {
                                            yield Ok(encode_raw_event(
                                                &recovered_name,
                                                &recovered_data,
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    yield Ok(encode_event(
                        "response.failed",
                        &response_failed_payload(
                            None,
                            Some(&downstream_model),
                            error.to_string(),
                            "server_error",
                        ),
                    ).expect("responses transport error is serializable"));
                    return;
                }
            }
        }
        if !decoder.remaining().is_empty() {
            yield Ok(Bytes::copy_from_slice(decoder.remaining()));
        }
    }
    .boxed()
}

fn split_malformed_metadata_events(event_name: &str, data: &str) -> Vec<(String, String)> {
    let mut events = Vec::new();
    let mut remaining = data;
    let mut current_event = event_name.to_owned();
    loop {
        if serde_json::from_str::<Value>(remaining).is_ok() {
            events.push((current_event, remaining.to_owned()));
            return events;
        }
        let Some(marker) = remaining.find("event: ") else {
            events.push((current_event, remaining.to_owned()));
            return events;
        };
        let (prefix, suffix) = remaining.split_at(marker);
        if serde_json::from_str::<Value>(prefix).is_err() {
            events.push((current_event, remaining.to_owned()));
            return events;
        }
        let after_marker = &suffix["event: ".len()..];
        let Some(newline) = after_marker.find('\n') else {
            events.push((current_event, remaining.to_owned()));
            return events;
        };
        events.push((current_event, prefix.to_owned()));
        current_event = after_marker[..newline].trim().to_owned();
        let mut next = &after_marker[newline + 1..];
        if let Some(stripped) = next.strip_prefix("data:") {
            next = stripped.trim_start();
        }
        remaining = next;
    }
}

fn rewrite_response_model_field(payload: &mut Value, upstream_model: &str, downstream_model: &str) {
    if let Some(model) = payload
        .get_mut("response")
        .and_then(|response| response.get_mut("model"))
        && model.as_str() == Some(upstream_model)
    {
        *model = Value::String(downstream_model.to_owned());
    }
}

pub(crate) async fn collect_response_stream(
    mut stream: ResponseStream,
) -> Result<CollectedResponse, GatewayError> {
    let mut decoder = SseDecoder::default();
    let mut completed = None;
    let mut terminal_error = None;
    let mut observed_output = Vec::new();
    while let Some(chunk) = stream.next().await {
        let bytes = match chunk {
            Ok(bytes) => bytes,
            Err(never) => match never {},
        };
        for event in decoder.push(&bytes) {
            match event.event.as_deref() {
                Some("response.completed") => {
                    let mut payload: Value = serde_json::from_str(&event.data)?;
                    completed = payload.get_mut("response").map(Value::take);
                }
                Some("response.output_item.done") => {
                    let mut payload: Value = serde_json::from_str(&event.data)?;
                    if let Some(item) = payload.get_mut("item").map(Value::take) {
                        observed_output.push(item);
                    }
                }
                Some("response.incomplete") => {
                    let mut payload: Value = serde_json::from_str(&event.data)?;
                    if payload
                        .pointer("/response/incomplete_details/reason")
                        .and_then(Value::as_str)
                        == Some("max_output_tokens")
                    {
                        completed = payload.get_mut("response").map(Value::take);
                        continue;
                    }
                    terminal_error = Some(
                        payload
                            .pointer("/error/message")
                            .or_else(|| payload.pointer("/response/error/message"))
                            .and_then(Value::as_str)
                            .unwrap_or("upstream response did not complete")
                            .to_owned(),
                    );
                }
                Some("response.failed") => {
                    let payload: Value = serde_json::from_str(&event.data)?;
                    terminal_error = Some(
                        payload
                            .pointer("/error/message")
                            .or_else(|| payload.pointer("/response/error/message"))
                            .and_then(Value::as_str)
                            .unwrap_or("upstream response did not complete")
                            .to_owned(),
                    );
                }
                _ => {}
            }
        }
    }
    if let Some(message) = terminal_error {
        return Err(GatewayError::Upstream(message));
    }
    let mut response = completed.ok_or_else(|| {
        GatewayError::Upstream("upstream ended without response.completed".to_owned())
    })?;
    let mut output = response
        .get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if output.is_empty() && !observed_output.is_empty() {
        output = observed_output;
        response["output"] = Value::Array(output.clone());
    }
    let output_text = collect_output_text(&output);
    let usage = response.get("usage").cloned().unwrap_or(Value::Null);
    Ok(CollectedResponse {
        response,
        output,
        output_text,
        usage,
    })
}

fn collect_output_text(output: &[Value]) -> String {
    output
        .iter()
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("output_text" | "text")
            )
            .then(|| part.get("text").and_then(Value::as_str))
            .flatten()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_rewrite_only_touches_the_response_metadata() {
        let mut payload = serde_json::json!({
            "type": "response.completed",
            "response": {
                "model": "upstream",
                "output": [{"model": "upstream"}]
            },
            "model": "upstream"
        });

        rewrite_response_model_field(&mut payload, "upstream", "catalog");

        assert_eq!(payload["response"]["model"], "catalog");
        assert_eq!(payload["response"]["output"][0]["model"], "upstream");
        assert_eq!(payload["model"], "upstream");
    }

    #[test]
    fn splits_concatenated_upstream_metadata_events() {
        let first = serde_json::json!({
            "type": "response.in_progress",
            "response": {"id": "resp-first"}
        })
        .to_string();
        let second = serde_json::json!({
            "type": "response.created",
            "response": {"id": "resp-second"}
        })
        .to_string();
        let malformed = format!("{first}event: response.created\n{second}");

        let events = split_malformed_metadata_events("response.in_progress", &malformed);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "response.in_progress");
        assert_eq!(events[0].1, first);
        assert_eq!(events[1].0, "response.created");
        assert_eq!(events[1].1, second);
    }

    #[test]
    fn leaves_valid_metadata_events_unchanged() {
        let valid = serde_json::json!({
            "type": "response.completed",
            "response": {"id": "resp-valid"}
        })
        .to_string();

        let events = split_malformed_metadata_events("response.completed", &valid);

        assert_eq!(events, vec![("response.completed".to_owned(), valid)]);
    }

    #[tokio::test]
    async fn collects_token_limited_incomplete_response() {
        let source = Bytes::from_static(
            b"event: response.incomplete\ndata: {\"type\":\"response.incomplete\",\"response\":{\"id\":\"resp_1\",\"status\":\"incomplete\",\"output\":[],\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\"usage\":{\"input_tokens\":3,\"output_tokens\":7}}}\n\n",
        );
        let stream: ResponseStream =
            futures_util::stream::iter(vec![Ok::<Bytes, std::convert::Infallible>(source)]).boxed();

        let collected = collect_response_stream(stream).await.unwrap();

        assert_eq!(collected.response["status"], "incomplete");
        assert_eq!(collected.usage["output_tokens"], 7);
    }
}
