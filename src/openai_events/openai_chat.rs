use super::state::{AssistantMessagePhase, MapperState, ToolBlock, ToolBlockKind};
use super::*;

pub fn map_openai_chat_sse<S>(
    upstream: S,
    original_request: Value,
    tool_names: ToolNameMap,
) -> impl Stream<Item = Result<Bytes, Infallible>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    map_openai_chat_sse_with_image_routes(upstream, original_request, tool_names, None)
}

pub(crate) fn map_openai_chat_sse_with_image_routes<S>(
    upstream: S,
    original_request: Value,
    tool_names: ToolNameMap,
    image_routes: Option<ImageRouteRegistry>,
) -> impl Stream<Item = Result<Bytes, Infallible>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    stream! {
        let mut state = MapperState::new(original_request, tool_names);
        let created = state.response_base("in_progress");
        yield Ok(encode_event("response.created", &json!({"type":"response.created","response":created})).unwrap());
        yield Ok(encode_event("response.in_progress", &json!({"type":"response.in_progress","response":state.response_base("in_progress")})).unwrap());

        let mut decoder = SseDecoder::default();
        tokio::pin!(upstream);
        while let Some(chunk) = upstream.next().await {
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(err) => {
                    yield Ok(state.failed_event(err.to_string()));
                    return;
                }
            };
            for event in decoder.push(&bytes) {
                if event.data == "[DONE]" {
                    let phase = state.fallback_text_phase();
                    for bytes in state.finish_text(phase) {
                        yield Ok(bytes);
                    }
                    match state.finish_tools(image_routes.as_ref()) {
                        Ok(events) => {
                            for bytes in events {
                                yield Ok(bytes);
                            }
                        }
                        Err(err) => {
                            yield Ok(state.failed_event(err));
                            return;
                        }
                    }
                    let completed = state.completed_response();
                    yield Ok(encode_event("response.completed", &json!({"type":"response.completed","response":completed})).unwrap());
                    return;
                }
                let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
                    yield Ok(encode_raw_event("response.warning", &json!({"type":"response.warning","warning":"invalid upstream SSE JSON"}).to_string()));
                    continue;
                };
                if let Some(usage) = data.get("usage") {
                    state.usage.input_tokens = usage.get("prompt_tokens").and_then(Value::as_u64);
                    state.usage.output_tokens = usage.get("completion_tokens").and_then(Value::as_u64);
                }
                let Some(choice) = data.get("choices").and_then(Value::as_array).and_then(|choices| choices.first()) else {
                    continue;
                };
                let delta = choice.get("delta").unwrap_or(&Value::Null);
                if let Some(text) = delta.get("content").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    for bytes in state.text_delta(text) {
                        yield Ok(bytes);
                    }
                }
                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tool_call in tool_calls {
                        let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0);
                        let entry = state.tools.entry(index).or_insert_with(|| ToolBlock {
                            id: tool_call
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            name: None,
                            start_input_json: String::new(),
                            delta_input_json: String::new(),
                            kind: ToolBlockKind::Function,
                        });
                        if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                            entry.id = Some(id.to_owned());
                        }
                        if let Some(function) = tool_call.get("function") {
                            if let Some(name) = function.get("name").and_then(Value::as_str) {
                                entry.name = Some(name.to_owned());
                            }
                            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                                entry.delta_input_json.push_str(arguments);
                            }
                        }
                    }
                }
                match choice.get("finish_reason").and_then(Value::as_str) {
                    Some("tool_calls") => {
                        for bytes in state.finish_text(AssistantMessagePhase::Commentary) {
                            yield Ok(bytes);
                        }
                        match state.finish_tools(image_routes.as_ref()) {
                            Ok(events) => {
                                for bytes in events {
                                    yield Ok(bytes);
                                }
                            }
                            Err(err) => {
                                yield Ok(state.failed_event(err));
                                return;
                            }
                        }
                    }
                    Some("stop") | Some("length") | Some("content_filter") => {
                        for bytes in state.finish_text(AssistantMessagePhase::FinalAnswer) {
                            yield Ok(bytes);
                        }
                    }
                    _ => {}
                }
            }
        }
        let phase = state.fallback_text_phase();
        for bytes in state.finish_text(phase) {
            yield Ok(bytes);
        }
        match state.finish_tools(image_routes.as_ref()) {
            Ok(events) => {
                for bytes in events {
                    yield Ok(bytes);
                }
            }
            Err(err) => {
                yield Ok(state.failed_event(err));
                return;
            }
        }
        let completed = state.completed_response();
        yield Ok(encode_event("response.completed", &json!({"type":"response.completed","response":completed})).unwrap());
    }
}
