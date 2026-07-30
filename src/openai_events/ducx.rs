use super::state::{AssistantMessagePhase, MapperState};
use super::*;

pub fn map_ducx_events<S>(
    upstream: S,
    original_request: Value,
) -> impl Stream<Item = Result<Bytes, Infallible>>
where
    S: Stream<Item = anyhow::Result<Value>> + Send + 'static,
{
    stream! {
        let mut state = MapperState::new(original_request, ToolNameMap::default());
        let created = state.response_base("in_progress");
        yield Ok(encode_event(
            "response.created",
            &json!({"type":"response.created","response":created}),
        )
        .unwrap());
        yield Ok(encode_event(
            "response.in_progress",
            &json!({"type":"response.in_progress","response":state.response_base("in_progress")}),
        )
        .unwrap());

        let mut last_error = None;
        tokio::pin!(upstream);
        while let Some(message) = upstream.next().await {
            let message = match message {
                Ok(message) => message,
                Err(error) => {
                    yield Ok(state.failed_event(error.to_string()));
                    return;
                }
            };
            match message.get("method").and_then(Value::as_str) {
                Some("item/agentMessage/delta") => {
                    if let Some(delta) = message
                        .pointer("/params/delta")
                        .and_then(Value::as_str)
                        .filter(|delta| !delta.is_empty())
                    {
                        for event in state.text_delta(delta) {
                            yield Ok(event);
                        }
                    }
                }
                Some("error" | "warning") => {
                    if let Some(error) = message
                        .pointer("/params/message")
                        .and_then(Value::as_str)
                        .filter(|error| !error.trim().is_empty())
                    {
                        last_error = Some(error.to_owned());
                    }
                }
                Some("turn/completed") => {
                    let status = message
                        .pointer("/params/turn/status")
                        .and_then(Value::as_str)
                        .unwrap_or("failed");
                    if status != "completed" {
                        yield Ok(state.failed_event(
                            last_error.unwrap_or_else(|| {
                                format!("DUCX turn finished with status {status}")
                            }),
                        ));
                        return;
                    }
                    for event in state.finish_text(AssistantMessagePhase::FinalAnswer) {
                        yield Ok(event);
                    }
                    let completed = state.completed_response();
                    yield Ok(encode_event(
                        "response.completed",
                        &json!({"type":"response.completed","response":completed}),
                    )
                    .unwrap());
                    return;
                }
                _ => {}
            }
        }
        yield Ok(state.failed_event("DUCX turn event stream ended before completion"));
    }
}

#[cfg(test)]
mod tests {
    use futures_util::stream;

    use super::*;

    #[tokio::test]
    async fn maps_text_delta_and_completion_to_responses_sse() {
        let upstream = stream::iter([
            Ok(json!({
                "method": "item/agentMessage/delta",
                "params": {"delta": "hello"}
            })),
            Ok(json!({
                "method": "turn/completed",
                "params": {"turn": {"status": "completed"}}
            })),
        ]);
        let chunks = map_ducx_events(upstream, json!({"model":"gpt-5.6-luna"}))
            .map(|chunk| chunk.unwrap())
            .collect::<Vec<_>>()
            .await;
        let output = String::from_utf8(
            chunks
                .into_iter()
                .flat_map(|chunk| chunk.to_vec())
                .collect(),
        )
        .unwrap();

        assert!(output.contains("event: response.created"));
        assert!(output.contains("event: response.output_text.delta"));
        assert!(output.contains("\"delta\":\"hello\""));
        assert!(output.contains("event: response.output_text.done"));
        assert!(output.contains("event: response.completed"));
    }

    #[tokio::test]
    async fn maps_failed_turn_to_response_failed() {
        let upstream = stream::iter([
            Ok(json!({
                "method": "error",
                "params": {"message": "upstream disconnected"}
            })),
            Ok(json!({
                "method": "turn/completed",
                "params": {"turn": {"status": "failed"}}
            })),
        ]);
        let chunks = map_ducx_events(upstream, json!({"model":"gpt-5.6-luna"}))
            .map(|chunk| chunk.unwrap())
            .collect::<Vec<_>>()
            .await;
        let output = String::from_utf8(
            chunks
                .into_iter()
                .flat_map(|chunk| chunk.to_vec())
                .collect(),
        )
        .unwrap();

        assert!(output.contains("event: response.failed"));
        assert!(output.contains("upstream disconnected"));
        assert!(!output.contains("event: response.completed"));
    }
}
