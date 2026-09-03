use super::state::{AssistantMessagePhase, MapperState};
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
        let created = state.response_base_initial("in_progress");
        yield Ok(encode_event("response.created", &json!({"type":"response.created","response":created})).unwrap());
        yield Ok(encode_event("response.in_progress", &json!({"type":"response.in_progress","response":state.response_base_initial("in_progress")})).unwrap());

        let mut decoder = SseDecoder::default();
        tokio::pin!(upstream);
        let mut pending = Vec::new();
        while let Some(chunk) = upstream.next().await {
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(err) => {
                    if let Some(combined) = coalesce_events(&mut pending) {
                        yield Ok(combined);
                    }
                    yield Ok(state.failed_event(err.to_string()));
                    return;
                }
            };
            for event in decoder.push(&bytes) {
                if event.data == "[DONE]" {
                    let phase = state.fallback_text_phase();
                    pending.extend(state.finish_text(phase));
                    match state.finish_tools(image_routes.as_ref()) {
                        Ok(events) => pending.extend(events),
                        Err(err) => {
                            if let Some(combined) = coalesce_events(&mut pending) {
                                yield Ok(combined);
                            }
                            yield Ok(state.failed_event(err));
                            return;
                        }
                    }
                    let completed = state.completed_response();
                    pending.push(encode_event("response.completed", &json!({"type":"response.completed","response":completed})).unwrap());
                    if let Some(combined) = coalesce_events(&mut pending) {
                        yield Ok(combined);
                    }
                    return;
                }
                let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
                    pending.push(encode_raw_event("response.warning", &json!({"type":"response.warning","warning":"invalid upstream SSE JSON"}).to_string()));
                    continue;
                };
                if let Some(error) = data.get("error") {
                    if let Some(combined) = coalesce_events(&mut pending) {
                        yield Ok(combined);
                    }
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("OpenAI Chat upstream returned an error");
                    yield Ok(state.failed_event(message));
                    return;
                }
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
                    pending.extend(state.text_delta(text));
                }
                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tool_call in tool_calls {
                        let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0);
                        let id = tool_call
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.trim().is_empty());
                        let entry = state.openai_tool_entry(index, id);
                        if let Some(function) = tool_call.get("function") {
                            if let Some(name) = function
                                .get("name")
                                .and_then(Value::as_str)
                                .filter(|name| !name.trim().is_empty())
                            {
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
                        pending.extend(state.finish_text(AssistantMessagePhase::Commentary));
                        match state.finish_tools(image_routes.as_ref()) {
                            Ok(events) => pending.extend(events),
                            Err(err) => {
                                if let Some(combined) = coalesce_events(&mut pending) {
                                    yield Ok(combined);
                                }
                                yield Ok(state.failed_event(err));
                                return;
                            }
                        }
                    }
                    Some("stop") | Some("length") | Some("content_filter") => {
                        pending.extend(state.finish_text(AssistantMessagePhase::FinalAnswer));
                    }
                    _ => {}
                }
            }
            if let Some(combined) = coalesce_events(&mut pending) {
                yield Ok(combined);
            }
        }
        let phase = state.fallback_text_phase();
        pending.extend(state.finish_text(phase));
        match state.finish_tools(image_routes.as_ref()) {
            Ok(events) => pending.extend(events),
            Err(err) => {
                if let Some(combined) = coalesce_events(&mut pending) {
                    yield Ok(combined);
                }
                yield Ok(state.failed_event(err));
                return;
            }
        }
        let completed = state.completed_response();
        pending.push(encode_event("response.completed", &json!({"type":"response.completed","response":completed})).unwrap());
        if let Some(combined) = coalesce_events(&mut pending) {
            yield Ok(combined);
        }
    }
}
