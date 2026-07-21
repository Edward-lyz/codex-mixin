use super::*;

#[derive(Debug)]
pub(super) struct TextBlock {
    pub(super) output_index: usize,
    pub(super) item_id: String,
    pub(super) text: String,
}

#[derive(Debug)]
pub(super) struct ToolBlock {
    pub(super) id: Option<String>,
    pub(super) name: Option<String>,
    pub(super) start_input_json: String,
    pub(super) delta_input_json: String,
    pub(super) kind: ToolBlockKind,
}

#[derive(Debug)]
pub(super) enum ToolBlockKind {
    Function,
    WebSearch { output_index: usize },
}

#[derive(Debug)]
pub(super) struct PendingWebSearch {
    pub(super) output_index: usize,
    pub(super) query: String,
}

#[derive(Debug)]
pub(super) struct MapperState {
    pub(super) response_id: String,
    pub(super) created_at: u64,
    pub(super) request: Value,
    pub(super) output: Vec<Value>,
    pub(super) current_text: Option<TextBlock>,
    pub(super) tools: HashMap<u64, ToolBlock>,
    pub(super) pending_web_searches: HashMap<String, PendingWebSearch>,
    pub(super) web_search_result_indexes: HashSet<u64>,
    pub(super) usage: Usage,
    pub(super) tool_names: ToolNameMap,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Usage {
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
}

impl MapperState {
    pub(super) fn new(request: Value, tool_names: ToolNameMap) -> Self {
        Self {
            response_id: format!("resp_{}", Uuid::new_v4().simple()),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            request,
            output: Vec::new(),
            current_text: None,
            tools: HashMap::new(),
            pending_web_searches: HashMap::new(),
            web_search_result_indexes: HashSet::new(),
            usage: Usage::default(),
            tool_names,
        }
    }

    pub(super) fn response_base(&self, status: &str) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "status": status,
            "error": null,
            "incomplete_details": null,
            "instructions": self.request.get("instructions").cloned().unwrap_or(Value::Null),
            "max_output_tokens": self.request.get("max_output_tokens").cloned().unwrap_or(Value::Null),
            "model": self.request.get("model").cloned().unwrap_or(Value::Null),
            "output": self.output.clone(),
            "parallel_tool_calls": self.request.get("parallel_tool_calls").cloned().unwrap_or(json!(true)),
            "previous_response_id": self.request.get("previous_response_id").cloned().unwrap_or(Value::Null),
            "reasoning": self.request.get("reasoning").cloned().unwrap_or(Value::Null),
            "store": self.request.get("store").cloned().unwrap_or(json!(false)),
            "temperature": self.request.get("temperature").cloned().unwrap_or(Value::Null),
            "text": self.request.get("text").cloned().unwrap_or_else(|| json!({"format": {"type": "text"}})),
            "tool_choice": self.request.get("tool_choice").cloned().unwrap_or(json!("auto")),
            "tools": self.request.get("tools").cloned().unwrap_or_else(|| json!([])),
            "top_p": self.request.get("top_p").cloned().unwrap_or(Value::Null),
            "truncation": self.request.get("truncation").cloned().unwrap_or(Value::Null),
            "usage": Value::Null,
            "user": self.request.get("user").cloned().unwrap_or(Value::Null),
            "metadata": self.request.get("metadata").cloned().unwrap_or_else(|| json!({})),
        })
    }

    pub(super) fn completed_response(&self) -> Value {
        let input_tokens = self.usage.input_tokens.unwrap_or(0);
        let output_tokens = self.usage.output_tokens.unwrap_or(0);
        let mut response = self.response_base("completed");
        response["usage"] = json!({
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": input_tokens + output_tokens
        });
        response
    }

    pub(super) fn failed_event(&self, message: impl Into<String>) -> Bytes {
        let message = message.into();
        tracing::warn!(error = %message, "upstream response stream failed");
        let error = json!({"message":message});
        let mut response = self.response_base("failed");
        response["error"] = error.clone();
        encode_event(
            "response.failed",
            &json!({"type":"response.failed","response":response,"error":error}),
        )
        .unwrap()
    }

    pub(super) fn start_text(&mut self) -> Vec<Bytes> {
        if self.current_text.is_some() {
            return Vec::new();
        }
        let output_index = self.output.len();
        let item_id = format!("msg_{}", Uuid::new_v4().simple());
        self.current_text = Some(TextBlock {
            output_index,
            item_id: item_id.clone(),
            text: String::new(),
        });
        vec![
            encode_event(
                "response.output_item.added",
                &json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":item_id,"type":"message","status":"in_progress","role":"assistant","content":[]}}),
            )
            .unwrap(),
            encode_event(
                "response.content_part.added",
                &json!({"type":"response.content_part.added","item_id":item_id,"output_index":output_index,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
            )
            .unwrap(),
        ]
    }

    pub(super) fn text_delta(&mut self, delta: &str) -> Vec<Bytes> {
        let mut events = self.start_text();
        let Some(block) = self.current_text.as_mut() else {
            return events;
        };
        block.text.push_str(delta);
        events.push(
            encode_event(
                "response.output_text.delta",
                &json!({"type":"response.output_text.delta","item_id":block.item_id,"output_index":block.output_index,"content_index":0,"delta":delta}),
            )
            .unwrap(),
        );
        events
    }

    pub(super) fn finish_text(&mut self) -> Vec<Bytes> {
        let Some(block) = self.current_text.take() else {
            return Vec::new();
        };
        let item = json!({
            "id": block.item_id,
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{"type":"output_text","text":block.text,"annotations":[]}]
        });
        self.output.push(item.clone());
        vec![
            encode_event(
                "response.output_text.done",
                &json!({"type":"response.output_text.done","item_id":block.item_id,"output_index":block.output_index,"content_index":0,"text":block.text}),
            )
            .unwrap(),
            encode_event(
                "response.content_part.done",
                &json!({"type":"response.content_part.done","item_id":block.item_id,"output_index":block.output_index,"content_index":0,"part":{"type":"output_text","text":block.text,"annotations":[]}}),
            )
            .unwrap(),
            encode_event(
                "response.output_item.done",
                &json!({"type":"response.output_item.done","output_index":block.output_index,"item":item}),
            )
            .unwrap(),
        ]
    }

    pub(super) fn finish_tool(
        &mut self,
        index: u64,
        image_routes: Option<&ImageRouteRegistry>,
    ) -> Result<Vec<Bytes>, String> {
        let block = self
            .tools
            .remove(&index)
            .ok_or_else(|| format!("tool call at index {index} was not started"))?;
        let id = block
            .id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| format!("tool call at index {index} missing id"))?
            .to_owned();
        let name = block
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| format!("tool call at index {index} missing name"))?
            .to_owned();
        let mut arguments = if block.delta_input_json.trim().is_empty() {
            block.start_input_json.trim().to_owned()
        } else {
            block.delta_input_json.trim().to_owned()
        };
        if let ToolBlockKind::WebSearch { output_index } = block.kind {
            let arguments = serde_json::from_str::<Value>(&arguments).map_err(|err| {
                format!("web_search call {id} arguments are not valid JSON: {err}")
            })?;
            let query = arguments
                .get("query")
                .and_then(Value::as_str)
                .filter(|query| !query.trim().is_empty())
                .ok_or_else(|| {
                    format!("web_search call {id} arguments must contain a non-empty query")
                })?
                .to_owned();
            if self
                .pending_web_searches
                .insert(
                    id.clone(),
                    PendingWebSearch {
                        output_index,
                        query,
                    },
                )
                .is_some()
            {
                return Err(format!("duplicate web_search tool_use id: {id}"));
            }
            return Ok(Vec::new());
        }
        if arguments.is_empty() {
            arguments = "{}".to_owned();
        }
        let codex_name = self.tool_names.to_codex_name(&name).to_owned();
        let codex_namespace = self.tool_names.to_codex_namespace(&name).map(str::to_owned);
        if let Some(image_routes) = image_routes
            && codex_namespace.as_deref() == Some("image_gen")
            && codex_name == "imagegen"
        {
            arguments = image_routes.mark_arguments(&arguments)?;
        }
        let output_index = self.output.len();
        let item_id = format!("fc_{}", Uuid::new_v4().simple());
        let item = if self.tool_names.is_custom(&name) {
            let parsed_arguments = serde_json::from_str::<Value>(&arguments).map_err(|err| {
                format!("custom tool call {id} arguments are not valid JSON: {err}")
            })?;
            let input = parsed_arguments
                .get("input")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("custom tool call {id} arguments must contain a string input field")
                })?;
            json!({
                "type": "custom_tool_call",
                "id": item_id,
                "call_id": id,
                "name": codex_name,
                "input": input,
                "status": "completed"
            })
        } else if let Some(execution) = self.tool_names.tool_search_execution(&name) {
            let arguments = serde_json::from_str::<Value>(&arguments).map_err(|err| {
                format!("tool_search call {id} arguments are not valid JSON: {err}")
            })?;
            json!({
                "type": "tool_search_call",
                "id": item_id,
                "call_id": id,
                "status": "completed",
                "execution": execution,
                "arguments": arguments
            })
        } else {
            let mut item = json!({
                "type": "function_call",
                "id": item_id,
                "call_id": id,
                "name": codex_name,
                "arguments": arguments,
                "status": "completed"
            });
            if let Some(namespace) = codex_namespace {
                item["namespace"] = json!(namespace);
            }
            item
        };
        self.output.push(item.clone());
        Ok(vec![
            encode_event(
                "response.output_item.added",
                &json!({"type":"response.output_item.added","output_index":output_index,"item":item}),
            )
            .unwrap(),
            encode_event(
                "response.output_item.done",
                &json!({"type":"response.output_item.done","output_index":output_index,"item":item}),
            )
            .unwrap(),
        ])
    }

    pub(super) fn finish_tools(
        &mut self,
        image_routes: Option<&ImageRouteRegistry>,
    ) -> Result<Vec<Bytes>, String> {
        let mut indexes = self.tools.keys().copied().collect::<Vec<_>>();
        indexes.sort_unstable();
        let mut events = Vec::new();
        for index in indexes {
            events.extend(self.finish_tool(index, image_routes)?);
        }
        Ok(events)
    }

    pub(super) fn start_web_search(
        &mut self,
        index: u64,
        id: Option<String>,
        name: Option<String>,
        start_input_json: String,
    ) -> Result<Vec<Bytes>, String> {
        let id = id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| format!("server tool call at index {index} missing id"))?;
        if name.as_deref() != Some("web_search") {
            return Err(format!(
                "unsupported Anthropic server tool: {}",
                name.as_deref().unwrap_or("<missing>")
            ));
        }
        if self.tools.contains_key(&index) {
            return Err(format!("duplicate tool call index: {index}"));
        }
        let output_index = self.output.len();
        let item = json!({
            "type":"web_search_call",
            "id":id,
            "status":"in_progress"
        });
        self.output.push(item.clone());
        self.tools.insert(
            index,
            ToolBlock {
                id: Some(id.to_owned()),
                name,
                start_input_json,
                delta_input_json: String::new(),
                kind: ToolBlockKind::WebSearch { output_index },
            },
        );
        Ok(vec![
            encode_event(
                "response.output_item.added",
                &json!({"type":"response.output_item.added","output_index":output_index,"item":item}),
            )
            .unwrap(),
        ])
    }

    pub(super) fn finish_web_search_result(
        &mut self,
        index: u64,
        content_block: &Value,
    ) -> Result<Vec<Bytes>, String> {
        if !self.web_search_result_indexes.insert(index) {
            return Err(format!("duplicate web_search result index: {index}"));
        }
        let tool_use_id = match content_block.get("tool_use_id").and_then(Value::as_str) {
            Some(tool_use_id) => tool_use_id.to_owned(),
            None if self.pending_web_searches.len() == 1 => {
                let tool_use_id = self
                    .pending_web_searches
                    .keys()
                    .next()
                    .expect("one pending web search")
                    .clone();
                tracing::warn!(
                    index,
                    tool_use_id,
                    "inferring omitted web_search tool_use_id from the only pending search"
                );
                tool_use_id
            }
            None => {
                return Err(format!(
                    "web_search result at index {index} missing tool_use_id with {} pending searches",
                    self.pending_web_searches.len()
                ));
            }
        };
        let pending = self
            .pending_web_searches
            .remove(&tool_use_id)
            .ok_or_else(|| {
                format!("web_search result references unknown tool_use_id: {tool_use_id}")
            })?;
        let content = content_block
            .get("content")
            .filter(|content| content.is_array() || content.is_object())
            .ok_or_else(|| format!("web_search result at index {index} missing content"))?;
        let failed = content.get("type").and_then(Value::as_str)
            == Some("web_search_tool_result_error")
            || content.as_array().is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("type").and_then(Value::as_str) == Some("web_search_tool_result_error")
                })
            });
        let item = json!({
            "type":"web_search_call",
            "id":tool_use_id,
            "status":if failed { "failed" } else { "completed" },
            "action":{"type":"search","query":pending.query}
        });
        let slot = self.output.get_mut(pending.output_index).ok_or_else(|| {
            format!(
                "web_search result output index is missing: {}",
                pending.output_index
            )
        })?;
        *slot = item.clone();
        Ok(vec![
            encode_event(
                "response.output_item.done",
                &json!({"type":"response.output_item.done","output_index":pending.output_index,"item":item}),
            )
            .unwrap(),
        ])
    }

    pub(super) fn ensure_web_searches_finished(&self) -> Result<(), String> {
        if self.pending_web_searches.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Anthropic stream ended before {} web_search result block(s)",
                self.pending_web_searches.len()
            ))
        }
    }
}
