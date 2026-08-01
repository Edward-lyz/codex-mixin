use super::state::{AssistantMessagePhase, MapperState, ToolBlock, ToolBlockKind};
use super::*;

pub fn map_anthropic_sse<S>(
    upstream: S,
    original_request: Value,
    tool_names: ToolNameMap,
) -> impl Stream<Item = Result<Bytes, Infallible>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    map_anthropic_sse_with_image_routes(upstream, original_request, tool_names, None, false)
}

pub(crate) fn map_anthropic_sse_with_image_routes<S>(
    upstream: S,
    original_request: Value,
    tool_names: ToolNameMap,
    image_routes: Option<ImageRouteRegistry>,
    baidu_oneapi_usage: bool,
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
                    continue;
                }
                let Ok(data) = serde_json::from_str::<Value>(&event.data) else {
                    pending.push(encode_raw_event("response.warning", &json!({"type":"response.warning","warning":"invalid upstream SSE JSON"}).to_string()));
                    continue;
                };
                match handle_anthropic_event(&mut state, &data, baidu_oneapi_usage) {
                    Ok(events) => pending.extend(events),
                    Err(err) => {
                        if let Some(combined) = coalesce_events(&mut pending) {
                            yield Ok(combined);
                        }
                        yield Ok(state.failed_event(err));
                        return;
                    }
                }
                if data.get("type").and_then(Value::as_str) == Some("error") {
                    if let Some(combined) = coalesce_events(&mut pending) {
                        yield Ok(combined);
                    }
                    return;
                }
                if data.get("type").and_then(Value::as_str) == Some("message_stop") {
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
                    if let Err(err) = state.ensure_web_searches_finished() {
                        if let Some(combined) = coalesce_events(&mut pending) {
                            yield Ok(combined);
                        }
                        yield Ok(state.failed_event(err));
                        return;
                    }
                    pending.push(encode_event("response.completed", &json!({"type":"response.completed","response":state.completed_response()})).unwrap());
                    if let Some(combined) = coalesce_events(&mut pending) {
                        yield Ok(combined);
                    }
                    return;
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
        if let Err(err) = state.ensure_web_searches_finished() {
            if let Some(combined) = coalesce_events(&mut pending) {
                yield Ok(combined);
            }
            yield Ok(state.failed_event(err));
            return;
        }
        pending.push(encode_event("response.completed", &json!({"type":"response.completed","response":state.completed_response()})).unwrap());
        if let Some(combined) = coalesce_events(&mut pending) {
            yield Ok(combined);
        }
    }
}

fn handle_anthropic_event(
    state: &mut MapperState,
    data: &Value,
    baidu_oneapi_usage: bool,
) -> Result<Vec<Bytes>, String> {
    match data.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            if let Some(usage) = data.pointer("/message/usage") {
                if baidu_oneapi_usage {
                    update_anthropic_input_usage(state, usage);
                } else {
                    state.usage.input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
                }
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
                    Ok(state.finish_text(AssistantMessagePhase::Commentary))
                }
                Some("server_tool_use") => {
                    let content_block = data
                        .get("content_block")
                        .ok_or_else(|| "server_tool_use missing content_block".to_owned())?;
                    let mut events = state.finish_text(AssistantMessagePhase::Commentary);
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
                    let mut events = state.finish_text(AssistantMessagePhase::Commentary);
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
            if state.ignored_web_search_result_indexes.remove(&index) {
                return Ok(Vec::new());
            }
            if state
                .tools
                .get(&index)
                .is_some_and(|tool| matches!(&tool.kind, ToolBlockKind::WebSearch))
            {
                state.finish_tool(index, None)
            } else {
                Ok(Vec::new())
            }
        }
        Some("message_delta") => {
            let stop_reason = data.pointer("/delta/stop_reason").and_then(Value::as_str);
            if stop_reason == Some("pause_turn") {
                return Err(
                    "Anthropic returned pause_turn; automatic server-tool continuation is unsupported"
                        .to_owned(),
                );
            }
            if baidu_oneapi_usage && let Some(usage) = data.get("usage") {
                update_anthropic_input_usage(state, usage);
            }
            if let Some(output_tokens) =
                data.pointer("/usage/output_tokens").and_then(Value::as_u64)
            {
                state.usage.output_tokens = Some(output_tokens);
            }
            match stop_reason {
                Some("tool_use") => Ok(state.finish_text(AssistantMessagePhase::Commentary)),
                Some(_) => Ok(state.finish_text(AssistantMessagePhase::FinalAnswer)),
                None => Ok(Vec::new()),
            }
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

fn update_anthropic_input_usage(state: &mut MapperState, usage: &Value) {
    let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
    let cache_read_tokens = usage.get("cache_read_input_tokens").and_then(Value::as_u64);
    let cache_creation_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64);
    if input_tokens.is_none() && cache_read_tokens.is_none() && cache_creation_tokens.is_none() {
        return;
    }

    state.usage.input_tokens = Some(
        input_tokens.unwrap_or(0)
            + cache_read_tokens.unwrap_or(0)
            + cache_creation_tokens.unwrap_or(0),
    );
    state.usage.cached_tokens = cache_read_tokens;
}
