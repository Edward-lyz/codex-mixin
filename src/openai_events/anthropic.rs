use super::state::{MapperState, ToolBlock, ToolBlockKind};
use super::*;

pub fn map_anthropic_sse<S>(
    upstream: S,
    original_request: Value,
    tool_names: ToolNameMap,
) -> impl Stream<Item = Result<Bytes, Infallible>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    map_anthropic_sse_with_image_routes(upstream, original_request, tool_names, None)
}

pub(crate) fn map_anthropic_sse_with_image_routes<S>(
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
                    continue;
                }
                let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
                    yield Ok(encode_raw_event("response.warning", &json!({"type":"response.warning","warning":"invalid upstream SSE JSON"}).to_string()));
                    continue;
                };
                match handle_anthropic_event(&mut state, &data) {
                    Ok(events) => {
                        for mapped in events {
                            yield Ok(mapped);
                        }
                    }
                    Err(err) => {
                        yield Ok(state.failed_event(err));
                        return;
                    }
                }
                if data.get("type").and_then(Value::as_str) == Some("error") {
                    return;
                }
                if data.get("type").and_then(Value::as_str) == Some("message_stop") {
                    for mapped in state.finish_text() {
                        yield Ok(mapped);
                    }
                    match state.finish_tools(image_routes.as_ref()) {
                        Ok(events) => {
                            for mapped in events {
                                yield Ok(mapped);
                            }
                        }
                        Err(err) => {
                            yield Ok(state.failed_event(err));
                            return;
                        }
                    }
                    if let Err(err) = state.ensure_web_searches_finished() {
                        yield Ok(state.failed_event(err));
                        return;
                    }
                    yield Ok(encode_event("response.completed", &json!({"type":"response.completed","response":state.completed_response()})).unwrap());
                    return;
                }
            }
        }
        for mapped in state.finish_text() {
            yield Ok(mapped);
        }
        match state.finish_tools(image_routes.as_ref()) {
            Ok(events) => {
                for mapped in events {
                    yield Ok(mapped);
                }
            }
            Err(err) => {
                yield Ok(state.failed_event(err));
                return;
            }
        }
        if let Err(err) = state.ensure_web_searches_finished() {
            yield Ok(state.failed_event(err));
            return;
        }
        yield Ok(encode_event("response.completed", &json!({"type":"response.completed","response":state.completed_response()})).unwrap());
    }
}

fn handle_anthropic_event(state: &mut MapperState, data: &Value) -> Result<Vec<Bytes>, String> {
    match data.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            if let Some(usage) = data.pointer("/message/usage") {
                state.usage.input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
                state.usage.output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
            }
            Ok(Vec::new())
        }
        Some("content_block_start") => {
            let index = data
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "content_block_start missing index".to_owned())?;
            match data.pointer("/content_block/type").and_then(Value::as_str) {
                Some("text") => Ok(state.start_text()),
                Some("tool_use") => {
                    let id = data
                        .pointer("/content_block/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let name = data
                        .pointer("/content_block/name")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let input_json = data
                        .pointer("/content_block/input")
                        .filter(|value| !value.is_null())
                        .map(Value::to_string)
                        .unwrap_or_default();
                    if state.tools.contains_key(&index) {
                        return Err(format!("duplicate tool call index: {index}"));
                    }
                    state.tools.insert(
                        index,
                        ToolBlock {
                            id,
                            name,
                            start_input_json: input_json,
                            delta_input_json: String::new(),
                            kind: ToolBlockKind::Function,
                        },
                    );
                    Ok(state.finish_text())
                }
                Some("server_tool_use") => {
                    let content_block = data
                        .get("content_block")
                        .ok_or_else(|| "server_tool_use missing content_block".to_owned())?;
                    let mut events = state.finish_text();
                    events.extend(
                        state.start_web_search(
                            index,
                            content_block
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            content_block
                                .get("name")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            content_block
                                .get("input")
                                .filter(|input| !input.is_null())
                                .map(Value::to_string)
                                .unwrap_or_default(),
                        )?,
                    );
                    Ok(events)
                }
                Some("web_search_tool_result") => {
                    let content_block = data
                        .get("content_block")
                        .ok_or_else(|| "web_search result missing content_block".to_owned())?;
                    let mut events = state.finish_text();
                    events.extend(state.finish_web_search_result(index, content_block)?);
                    Ok(events)
                }
                _ => Ok(Vec::new()),
            }
        }
        Some("content_block_delta") => {
            let index = data
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "content_block_delta missing index".to_owned())?;
            match data.pointer("/delta/type").and_then(Value::as_str) {
                Some("text_delta") => {
                    let delta = data
                        .pointer("/delta/text")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    Ok(state.text_delta(delta))
                }
                Some("input_json_delta") => {
                    let partial = data
                        .pointer("/delta/partial_json")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "input_json_delta missing partial_json".to_owned())?;
                    let tool = state.tools.get_mut(&index).ok_or_else(|| {
                        format!("input_json_delta references unknown tool index: {index}")
                    })?;
                    tool.delta_input_json.push_str(partial);
                    Ok(Vec::new())
                }
                Some("thinking_delta" | "signature_delta") => Ok(Vec::new()),
                _ => Ok(Vec::new()),
            }
        }
        Some("content_block_stop") => {
            let index = data
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| "content_block_stop missing index".to_owned())?;
            if state
                .tools
                .get(&index)
                .is_some_and(|tool| matches!(&tool.kind, ToolBlockKind::WebSearch { .. }))
            {
                state.finish_tool(index, None)
            } else if state.tools.contains_key(&index)
                || state.web_search_result_indexes.remove(&index)
            {
                Ok(Vec::new())
            } else {
                Ok(state.finish_text())
            }
        }
        Some("message_delta") => {
            if data.pointer("/delta/stop_reason").and_then(Value::as_str) == Some("pause_turn") {
                return Err(
                    "Anthropic returned pause_turn; automatic server-tool continuation is unsupported"
                        .to_owned(),
                );
            }
            if let Some(output_tokens) =
                data.pointer("/usage/output_tokens").and_then(Value::as_u64)
            {
                state.usage.output_tokens = Some(output_tokens);
            }
            Ok(Vec::new())
        }
        Some("error") => Ok(vec![
            state.failed_event(
                data.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream stream error"),
            ),
        ]),
        _ => Ok(Vec::new()),
    }
}
