use super::state::{AssistantMessagePhase, MapperState};
use super::*;

#[derive(Default)]
struct GroundingState {
    queries: Vec<String>,
    sources: Vec<Value>,
}

pub fn map_gemini_sse<S>(
    upstream: S,
    original_request: Value,
    tool_names: ToolNameMap,
) -> impl Stream<Item = Result<Bytes, Infallible>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    map_gemini_sse_with_image_routes(upstream, original_request, tool_names, None)
}

pub(crate) fn map_gemini_sse_with_image_routes<S>(
    upstream: S,
    original_request: Value,
    tool_names: ToolNameMap,
    image_routes: Option<ImageRouteRegistry>,
) -> impl Stream<Item = Result<Bytes, Infallible>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let model_name = original_request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    stream! {
        let mut state = MapperState::new(original_request, tool_names);
        let created = state.response_base_initial("in_progress");
        if let Ok(event) = encode_event(
            "response.created",
            &json!({"type":"response.created","response":created}),
        ) {
            yield Ok(event);
        }
        if let Ok(event) = encode_event(
            "response.in_progress",
            &json!({"type":"response.in_progress","response":state.response_base_initial("in_progress")}),
        ) {
            yield Ok(event);
        }

        let mut decoder = SseDecoder::default();
        tokio::pin!(upstream);
        let mut pending = Vec::new();
        let mut last_thought_signature: Option<String> = None;
        let mut finish_reason: Option<String> = None;
        let mut next_tool_index: u64 = 0;
        let mut emitted_tool_ids = HashSet::new();
        let mut grounding = GroundingState::default();

        let finish_stream = |state: &mut MapperState,
                             pending: &mut Vec<Bytes>,
                             image_routes: Option<&ImageRouteRegistry>,
                             finish_reason: Option<&str>,
                             grounding: &GroundingState|
         -> Result<(), String> {
            pending.extend(state.finish_chat_reasoning());
            match finish_reason {
                Some("MAX_TOKENS") => {
                    pending.extend(state.finish_text(AssistantMessagePhase::FinalAnswer));
                    pending.push(state.incomplete_chat("max_output_tokens"));
                    Ok(())
                }
                Some("SAFETY") => {
                    pending.extend(state.finish_text(AssistantMessagePhase::FinalAnswer));
                    pending.push(state.incomplete_chat("content_filter"));
                    Ok(())
                }
                _ => {
                    let phase = state.fallback_text_phase();
                    pending.extend(state.finish_text(phase));
                    let events = state.finish_chat_tools(image_routes)?;
                    pending.extend(events);
                    for query in &grounding.queries {
                        let index = state.output.len();
                        let item = json!({"type":"web_search_call","id":format!("ws_{}", Uuid::new_v4().simple()),
                            "status":"completed","action":{"type":"search","query":query,"sources":grounding.sources}});
                        let mut added = item.clone();
                        added["status"] = json!("in_progress");
                        pending.push(chat_event("response.output_item.added", json!({"type":"response.output_item.added","output_index":index,"item":added})));
                        pending.push(chat_event("response.web_search_call.completed", json!({"type":"response.web_search_call.completed","output_index":index,"item_id":item["id"]})));
                        pending.push(chat_event("response.output_item.done", json!({"type":"response.output_item.done","output_index":index,"item":item})));
                        state.output.push(item);
                    }
                    let completed = state.completed_response();
                    if let Ok(event) = encode_event(
                        "response.completed",
                        &json!({"type":"response.completed","response":completed}),
                    ) {
                        pending.push(event);
                    }
                    Ok(())
                }
            }
        };

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
                if event.data.is_empty() {
                    continue;
                }
                // Standard Gemini SSE terminates on stream close, but internal Antigravity
                // gateways or proxies may emit an explicit [DONE] event.
                if event.data == "[DONE]" {
                    if let Err(err) = finish_stream(
                        &mut state,
                        &mut pending,
                        image_routes.as_ref(),
                        finish_reason.as_deref(),
                        &grounding,
                    ) {
                        if let Some(combined) = coalesce_events(&mut pending) {
                            yield Ok(combined);
                        }
                        yield Ok(state.failed_event(err));
                        return;
                    }
                    if let Some(combined) = coalesce_events(&mut pending) {
                        yield Ok(combined);
                    }
                    return;
                }
                let data = match serde_json::from_str::<Value>(&event.data) {
                    Ok(data) => data,
                    Err(error) => {
                        if let Some(combined) = coalesce_events(&mut pending) { yield Ok(combined); }
                        yield Ok(state.failed_event(format!("invalid upstream SSE JSON: {error}")));
                        return;
                    }
                };
                let error = data.get("error").or_else(|| data.pointer("/response/error"));
                if let Some(error) = error {
                    if let Some(combined) = coalesce_events(&mut pending) {
                        yield Ok(combined);
                    }
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Gemini upstream returned an error");
                    yield Ok(state.failed_event(message));
                    return;
                }
                let usage = data
                    .get("usageMetadata")
                    .or_else(|| data.pointer("/response/usageMetadata"));
                if let Some(usage) = usage {
                    state.usage.input_tokens = usage.get("promptTokenCount").and_then(Value::as_u64);
                    state.usage.output_tokens = usage.get("candidatesTokenCount").and_then(Value::as_u64);
                    state.usage.cached_tokens = usage.get("cachedContentTokenCount").and_then(Value::as_u64);
                }
                if let Some(sig) = data
                    .get("thoughtSignature")
                    .or_else(|| data.get("thought_signature"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    last_thought_signature = Some(sig.to_owned());
                }
                let candidates = data
                    .get("candidates")
                    .or_else(|| data.pointer("/response/candidates"))
                    .and_then(Value::as_array);
                if let Some(candidate) = candidates.and_then(|c| c.first()) {
                    if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str)
                        && !reason.is_empty()
                        && reason != "FINISH_REASON_UNSPECIFIED"
                    {
                        finish_reason = Some(reason.to_owned());
                    }
                    if let Some(sig) = candidate
                        .get("thoughtSignature")
                        .or_else(|| candidate.get("thought_signature"))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        last_thought_signature = Some(sig.to_owned());
                    }
                    if let Some(parts) = candidate.pointer("/content/parts").and_then(Value::as_array) {
                        for part in parts {
                            if let Some(sig) = part
                                .get("thoughtSignature")
                                .or_else(|| part.get("thought_signature"))
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                            {
                                last_thought_signature = Some(sig.to_owned());
                            }
                            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                                if let Some(thought) = part.get("text").and_then(Value::as_str)
                                    && !thought.is_empty()
                                {
                                    pending.extend(state.chat_reasoning_delta(thought));
                                }
                            } else if let Some(text) = part.get("text").and_then(Value::as_str)
                                && !text.is_empty()
                            {
                                pending.extend(state.finish_chat_reasoning());
                                pending.extend(state.text_delta(text));
                            }
                        }

                        if let Some(metadata) = candidate.get("groundingMetadata")
                            && let Err(error) = add_grounding(&mut state, &mut grounding, &mut pending, metadata)
                        {
                            if let Some(combined) = coalesce_events(&mut pending) { yield Ok(combined); }
                            yield Ok(state.failed_event(error));
                            return;
                        }

                        let chunk_func_calls: Vec<(&Value, &str, String)> = parts
                            .iter()
                            .filter_map(|p| {
                                let func_call = p.get("functionCall")?;
                                let name = func_call.get("name").and_then(Value::as_str).unwrap_or("");
                                let args = func_call.get("args").cloned().unwrap_or(json!({}));
                                let args_str = serde_json::to_string(&args).unwrap_or_default();
                                Some((p, name, args_str))
                            })
                            .collect();

                        if !chunk_func_calls.is_empty() {
                            pending.extend(state.finish_chat_reasoning());
                            for (part, name, args_str) in chunk_func_calls {
                                let func_call = &part["functionCall"];
                                let upstream_id = func_call
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .filter(|id| !id.trim().is_empty());
                                // Equal arguments do not identify a call. Without an upstream
                                // ID, each part is a separate invocation.
                                if let Some(id) = upstream_id
                                    && !emitted_tool_ids.insert(id.to_owned())
                                {
                                    continue;
                                }
                                let tool_index = next_tool_index;
                                next_tool_index += 1;

                                let sig = part
                                    .get("thoughtSignature")
                                    .or_else(|| part.get("thought_signature"))
                                    .and_then(Value::as_str)
                                    .or(last_thought_signature.as_deref());
                                let base_id = upstream_id
                                    .map(ToOwned::to_owned)
                                    .unwrap_or_else(|| {
                                        if tool_index == 0 {
                                            format!("call_{name}")
                                        } else {
                                            format!("call_{name}_{tool_index}")
                                        }
                                    });
                                let call_id = match sig {
                                    Some(sig) => crate::protocol::gemini::encode_signature_in_call_id(&base_id, sig, &model_name),
                                    None => base_id,
                                };
                                let tool_call = json!({
                                    "index": tool_index,
                                    "id": call_id,
                                    "function": {
                                        "name": name,
                                        "arguments": args_str.clone(),
                                    }
                                });
                                match state.chat_tool_delta(&tool_call, image_routes.as_ref()) {
                                    Ok(events) => pending.extend(events),
                                    Err(err) => {
                                        if let Some(combined) = coalesce_events(&mut pending) { yield Ok(combined); }
                                        yield Ok(state.failed_event(err));
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(metadata) = candidates.and_then(|c| c.first()).and_then(|candidate|
                    candidate.get("groundingMetadata").filter(|_| candidate.pointer("/content/parts").is_none()))
                    && let Err(error) = add_grounding(&mut state, &mut grounding, &mut pending, metadata)
                {
                    if let Some(combined) = coalesce_events(&mut pending) { yield Ok(combined); }
                    yield Ok(state.failed_event(error));
                    return;
                }
                if let Some(combined) = coalesce_events(&mut pending) {
                    yield Ok(combined);
                }
            }
        }
        if !decoder.remaining().is_empty() || finish_reason.is_none() {
            if let Some(combined) = coalesce_events(&mut pending) {
                yield Ok(combined);
            }
            yield Ok(state.failed_event("Gemini stream ended unexpectedly"));
            return;
        }
        if let Err(err) = finish_stream(
            &mut state,
            &mut pending,
            image_routes.as_ref(),
            finish_reason.as_deref(),
            &grounding,
        ) {
            if let Some(combined) = coalesce_events(&mut pending) {
                yield Ok(combined);
            }
            yield Ok(state.failed_event(err));
            return;
        }
        if let Some(combined) = coalesce_events(&mut pending) {
            yield Ok(combined);
        }
    }
}

fn add_grounding(
    state: &mut MapperState,
    grounding: &mut GroundingState,
    pending: &mut Vec<Bytes>,
    metadata: &Value,
) -> Result<(), String> {
    if let Some(queries) = metadata.get("webSearchQueries").and_then(Value::as_array) {
        for query in queries {
            let query = query
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .ok_or("Gemini returned an invalid search query")?;
            if !grounding.queries.iter().any(|existing| existing == query) {
                if grounding.queries.len() >= 64 {
                    return Err("Gemini search query limit exceeded".to_owned());
                }
                grounding.queries.push(query.to_owned());
            }
        }
    }
    let chunks = metadata.get("groundingChunks").and_then(Value::as_array);
    for chunk in chunks.into_iter().flatten() {
        if let Some(web) = chunk.get("web") {
            let uri = web
                .get("uri")
                .and_then(Value::as_str)
                .ok_or("Gemini citation has no URL")?;
            let url = url::Url::parse(uri).map_err(|_| "Gemini citation URL is invalid")?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err("Gemini citation URL must use HTTP or HTTPS".to_owned());
            }
            let source = json!({"type":"url","url":uri});
            if !grounding.sources.contains(&source) {
                if grounding.sources.len() >= 256 {
                    return Err("Gemini citation limit exceeded".to_owned());
                }
                grounding.sources.push(source);
            }
        }
    }
    let Some(supports) = metadata.get("groundingSupports").and_then(Value::as_array) else {
        return Ok(());
    };
    let block = state
        .current_text
        .as_mut()
        .ok_or("Gemini citations have no text item")?;
    for support in supports {
        let start = support
            .pointer("/segment/startIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let end = support
            .pointer("/segment/endIndex")
            .and_then(Value::as_u64)
            .ok_or("Gemini citation has no end offset")?;
        let start = usize::try_from(start).map_err(|_| "Gemini citation offset is too large")?;
        let end = usize::try_from(end).map_err(|_| "Gemini citation offset is too large")?;
        if start > end || block.text.get(start..end).is_none() {
            return Err("Gemini citation offsets are outside the text".to_owned());
        }
        let indices = support
            .get("groundingChunkIndices")
            .and_then(Value::as_array)
            .ok_or("Gemini citation has no source indices")?;
        for index in indices {
            let index = index
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .ok_or("Gemini citation has an invalid source index")?;
            let web = chunks
                .and_then(|chunks| chunks.get(index))
                .and_then(|chunk| chunk.get("web"))
                .ok_or("Gemini citation references an unknown web source")?;
            let uri = web["uri"].as_str().ok_or("Gemini citation has no URL")?;
            let annotation = json!({"type":"url_citation","url":uri,"title":web.get("title").and_then(Value::as_str).unwrap_or(uri),
                "start_index":block.text[..start].chars().count(), "end_index":block.text[..end].chars().count()});
            if block.annotations.contains(&annotation) {
                continue;
            }
            if block.annotations.len() >= 256 {
                return Err("Gemini annotation limit exceeded".to_owned());
            }
            pending.push(chat_event(
                "response.output_text.annotation.added",
                json!({"type":"response.output_text.annotation.added",
                "item_id":block.item_id,"output_index":block.output_index,"content_index":0,
                "annotation_index":block.annotations.len(),"annotation":annotation}),
            ));
            block.annotations.push(annotation);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::sse::drain_events;

    #[tokio::test]
    async fn map_gemini_sse_translates_text_and_usage() {
        let request = json!({
            "model": "gemini-3.8-flash-low",
            "stream": true,
            "input": "say hi"
        });
        let stream = futures_util::stream::iter(vec![
            Ok::<_, reqwest::Error>(Bytes::from_static(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello \"}]}}]}\n\n",
            )),
            Ok::<_, reqwest::Error>(Bytes::from_static(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"from Gemini!\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":12,\"candidatesTokenCount\":5,\"totalTokenCount\":17}}\n\n",
            )),
        ]);
        let mapped = map_gemini_sse(stream, request, ToolNameMap::default());
        tokio::pin!(mapped);
        let mut text_output = String::new();
        let mut completed_usage: Option<Value> = None;
        while let Some(chunk) = mapped.next().await {
            let bytes = chunk.unwrap();
            let mut buf = bytes.to_vec();
            for event in drain_events(&mut buf) {
                if event.event.as_deref() == Some("response.output_text.delta") {
                    let val: Value = serde_json::from_str(&event.data).unwrap();
                    if let Some(delta) = val.get("delta").and_then(Value::as_str) {
                        text_output.push_str(delta);
                    }
                }
                if event.event.as_deref() == Some("response.completed") {
                    let val: Value = serde_json::from_str(&event.data).unwrap();
                    completed_usage = val.pointer("/response/usage").cloned();
                }
            }
        }
        assert_eq!(text_output, "Hello from Gemini!");
        let usage = completed_usage.expect("completed usage");
        assert_eq!(usage["input_tokens"], 12);
        assert_eq!(usage["output_tokens"], 5);
    }

    #[tokio::test]
    async fn map_gemini_sse_translates_thought_delta() {
        let request = json!({
            "model": "gemini-3.8-flash-high",
            "stream": true,
            "input": "think"
        });
        let stream = futures_util::stream::iter(vec![
            Ok::<_, reqwest::Error>(Bytes::from_static(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Analyzing question...\",\"thought\":true}]}}]}\n\n",
            )),
            Ok::<_, reqwest::Error>(Bytes::from_static(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"42\"}]}}]}\n\n",
            )),
        ]);
        let mapped = map_gemini_sse(stream, request, ToolNameMap::default());
        tokio::pin!(mapped);
        let mut thought_output = String::new();
        let mut text_output = String::new();
        while let Some(chunk) = mapped.next().await {
            let bytes = chunk.unwrap();
            let mut buf = bytes.to_vec();
            for event in drain_events(&mut buf) {
                if event.event.as_deref() == Some("response.reasoning_text.delta") {
                    let val: Value = serde_json::from_str(&event.data).unwrap();
                    if let Some(delta) = val.get("delta").and_then(Value::as_str) {
                        thought_output.push_str(delta);
                    }
                }
                if event.event.as_deref() == Some("response.output_text.delta") {
                    let val: Value = serde_json::from_str(&event.data).unwrap();
                    if let Some(delta) = val.get("delta").and_then(Value::as_str) {
                        text_output.push_str(delta);
                    }
                }
            }
        }
        assert_eq!(thought_output, "Analyzing question...");
        assert_eq!(text_output, "42");
    }

    #[tokio::test]
    async fn map_gemini_sse_translates_function_call() {
        let request = json!({
            "model": "gemini-3.8-flash-low",
            "stream": true,
            "input": "get weather"
        });
        let stream = futures_util::stream::iter(vec![
            Ok::<_, reqwest::Error>(Bytes::from_static(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"Berlin\"}}}]}}]}\n\n",
            )),
        ]);
        let mapped = map_gemini_sse(stream, request, ToolNameMap::default());
        tokio::pin!(mapped);
        let mut found_call = false;
        while let Some(chunk) = mapped.next().await {
            let bytes = chunk.unwrap();
            let mut buf = bytes.to_vec();
            for event in drain_events(&mut buf) {
                if event.event.as_deref() == Some("response.output_item.added") {
                    let val: Value = serde_json::from_str(&event.data).unwrap();
                    if val["item"]["type"] == "function_call" {
                        assert_eq!(val["item"]["name"], "get_weather");
                        found_call = true;
                    }
                }
            }
        }
        assert!(found_call, "expected function_call event");
    }

    #[tokio::test]
    async fn map_gemini_sse_encodes_thought_signature_into_call_id() {
        let request = json!({
            "model": "gemini-3.8-flash-high",
            "stream": true,
            "input": "run pwd"
        });
        let raw_sig = "AQJyZWFsLXNpZ25hdHVyZS1ieXRlcwM=";
        let sse_json = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "exec_command",
                            "args": {"cmd": "pwd"}
                        },
                        "thoughtSignature": raw_sig
                    }]
                }
            }]
        });
        let sse = format!("data: {sse_json}\n\n");
        let stream = futures_util::stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(sse))]);
        let mapped = map_gemini_sse(stream, request, ToolNameMap::default());
        tokio::pin!(mapped);
        let mut call_id = String::new();
        while let Some(chunk) = mapped.next().await {
            let bytes = chunk.unwrap();
            let mut buf = bytes.to_vec();
            for event in drain_events(&mut buf) {
                if event.event.as_deref() == Some("response.output_item.added") {
                    let val: Value = serde_json::from_str(&event.data).unwrap();
                    if val["item"]["type"] == "function_call" {
                        call_id = val["item"]["call_id"].as_str().unwrap().to_owned();
                    }
                }
            }
        }
        assert!(call_id.starts_with("call_exec_command_ts_"));
        let decoded =
            crate::protocol::gemini::extract_thought_signature(&json!({"call_id": call_id}), true);
        assert_eq!(decoded, raw_sig);
    }

    #[tokio::test]
    async fn map_gemini_sse_captures_preceding_thought_signature() {
        let request = json!({
            "model": "gemini-3.8-flash-high",
            "stream": true,
            "input": "run pwd"
        });
        let raw_sig = "Evq83vArLw==";
        let sse_thought = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": "thinking...",
                        "thought": true,
                        "thought_signature": raw_sig
                    }]
                }
            }]
        });
        let sse_func = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "exec_command",
                            "args": {"cmd": "pwd"}
                        }
                    }]
                }
            }]
        });
        let stream = futures_util::stream::iter(vec![
            Ok::<_, reqwest::Error>(Bytes::from(format!("data: {sse_thought}\n\n"))),
            Ok::<_, reqwest::Error>(Bytes::from(format!("data: {sse_func}\n\n"))),
        ]);
        let mapped = map_gemini_sse(stream, request, ToolNameMap::default());
        tokio::pin!(mapped);
        let mut call_id = String::new();
        while let Some(chunk) = mapped.next().await {
            let bytes = chunk.unwrap();
            let mut buf = bytes.to_vec();
            for event in drain_events(&mut buf) {
                if event.event.as_deref() == Some("response.output_item.added") {
                    let val: Value = serde_json::from_str(&event.data).unwrap();
                    if val["item"]["type"] == "function_call" {
                        call_id = val["item"]["call_id"].as_str().unwrap().to_owned();
                    }
                }
            }
        }
        assert!(call_id.starts_with("call_exec_command_ts_"));
        let decoded =
            crate::protocol::gemini::extract_thought_signature(&json!({"call_id": call_id}), true);
        assert_eq!(decoded, raw_sig);
    }

    #[tokio::test]
    async fn deduplicates_repeated_call_ids() {
        let request = json!({
            "model": "gemini-3.8-flash-high",
            "stream": true,
            "input": "run pwd"
        });
        let sse_chunk1 = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "id": "call_pwd",
                            "name": "exec_command",
                            "args": {"cmd": "pwd"}
                        }
                    }]
                }
            }]
        });
        let sse_chunk2 = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "id": "call_pwd",
                            "name": "exec_command",
                            "args": {"cmd": "pwd"}
                        }
                    }]
                },
                "finishReason": "STOP"
            }]
        });
        let stream = futures_util::stream::iter(vec![
            Ok::<_, reqwest::Error>(Bytes::from(format!("data: {sse_chunk1}\n\n"))),
            Ok::<_, reqwest::Error>(Bytes::from(format!("data: {sse_chunk2}\n\n"))),
            Ok::<_, reqwest::Error>(Bytes::from_static(b"data: [DONE]\n\n")),
        ]);
        let mapped = map_gemini_sse(stream, request, ToolNameMap::default());
        tokio::pin!(mapped);
        let mut failed = false;
        let mut completed = false;
        let mut tool_count = 0;
        while let Some(chunk) = mapped.next().await {
            let bytes = chunk.unwrap();
            let mut buf = bytes.to_vec();
            for event in drain_events(&mut buf) {
                if event.event.as_deref() == Some("response.output_item.added") {
                    let value: Value = serde_json::from_str(&event.data).unwrap();
                    if value["item"]["type"] == "function_call" {
                        tool_count += 1;
                    }
                }
                if event.event.as_deref() == Some("response.failed") {
                    failed = true;
                }
                if event.event.as_deref() == Some("response.completed") {
                    completed = true;
                }
            }
        }
        assert!(!failed, "stream should not fail with trailing characters");
        assert!(completed, "stream should complete successfully");
        assert_eq!(tool_count, 1);
    }

    #[tokio::test]
    async fn map_gemini_sse_handles_max_tokens_finish_reason() {
        let request = json!({
            "model": "gemini-3.8-flash-low",
            "stream": true,
            "input": "say hi"
        });
        let stream = futures_util::stream::iter(vec![
            Ok::<_, reqwest::Error>(Bytes::from_static(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello cut short\"}],\"role\":\"model\"},\"finishReason\":\"MAX_TOKENS\"}]}\n\n",
            )),
        ]);
        let mapped = map_gemini_sse(stream, request, ToolNameMap::default());
        tokio::pin!(mapped);
        let mut incomplete_reason: Option<String> = None;
        while let Some(chunk) = mapped.next().await {
            let bytes = chunk.unwrap();
            let mut buf = bytes.to_vec();
            for event in drain_events(&mut buf) {
                if event.event.as_deref() == Some("response.incomplete") {
                    let val: Value = serde_json::from_str(&event.data).unwrap();
                    incomplete_reason = val
                        .pointer("/response/incomplete_details/reason")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                }
            }
        }
        assert_eq!(incomplete_reason.as_deref(), Some("max_output_tokens"));
    }

    #[tokio::test]
    async fn map_gemini_sse_handles_safety_finish_reason() {
        let request = json!({
            "model": "gemini-3.8-flash-low",
            "stream": true,
            "input": "say hi"
        });
        let stream = futures_util::stream::iter(vec![
            Ok::<_, reqwest::Error>(Bytes::from_static(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"blocked\"}],\"role\":\"model\"},\"finishReason\":\"SAFETY\"}]}\n\n",
            )),
        ]);
        let mapped = map_gemini_sse(stream, request, ToolNameMap::default());
        tokio::pin!(mapped);
        let mut incomplete_reason: Option<String> = None;
        while let Some(chunk) = mapped.next().await {
            let bytes = chunk.unwrap();
            let mut buf = bytes.to_vec();
            for event in drain_events(&mut buf) {
                if event.event.as_deref() == Some("response.incomplete") {
                    let val: Value = serde_json::from_str(&event.data).unwrap();
                    incomplete_reason = val
                        .pointer("/response/incomplete_details/reason")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                }
            }
        }
        assert_eq!(incomplete_reason.as_deref(), Some("content_filter"));
    }

    #[tokio::test]
    async fn map_gemini_sse_handles_multiple_tool_calls_across_chunks() {
        let request = json!({
            "model": "gemini-3.8-flash-high",
            "stream": true,
            "input": "check weather and date"
        });
        let sse_chunk1 = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "get_weather",
                            "args": {"city": "Tokyo"}
                        }
                    }]
                }
            }]
        });
        let sse_chunk2 = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "get_date",
                            "args": {}
                        }
                    }]
                },
                "finishReason": "STOP"
            }]
        });
        let stream = futures_util::stream::iter(vec![
            Ok::<_, reqwest::Error>(Bytes::from(format!("data: {sse_chunk1}\n\n"))),
            Ok::<_, reqwest::Error>(Bytes::from(format!("data: {sse_chunk2}\n\n"))),
        ]);
        let mapped = map_gemini_sse(stream, request, ToolNameMap::default());
        tokio::pin!(mapped);
        let mut tool_calls = Vec::new();
        let mut failed = false;
        while let Some(chunk) = mapped.next().await {
            let bytes = chunk.unwrap();
            let mut buf = bytes.to_vec();
            for event in drain_events(&mut buf) {
                if event.event.as_deref() == Some("response.failed") {
                    failed = true;
                }
                if event.event.as_deref() == Some("response.output_item.added") {
                    let val: Value = serde_json::from_str(&event.data).unwrap();
                    if val["item"]["type"] == "function_call" {
                        tool_calls.push(val["item"].clone());
                    }
                }
            }
        }
        assert!(
            !failed,
            "should not fail on multiple tool calls across chunks"
        );
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0]["name"], "get_weather");
        assert_eq!(tool_calls[0]["call_id"], "call_get_weather");
        assert_eq!(tool_calls[1]["name"], "get_date");
        assert_eq!(tool_calls[1]["call_id"], "call_get_date_1");
    }
}
