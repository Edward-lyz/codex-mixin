use super::state::{AssistantMessagePhase, MapperState};
use super::*;

#[derive(Debug)]
pub(super) struct StreamingTool {
    output_index: usize,
    item_id: String,
    sent_bytes: usize,
}

impl MapperState {
    pub(super) fn chat_tool_delta(
        &mut self,
        tool: &Value,
        image_routes: Option<&ImageRouteRegistry>,
    ) -> Result<Vec<Bytes>, String> {
        let index = tool.get("index").and_then(Value::as_u64).unwrap_or(0);
        let id = tool
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty());
        let entry = self.openai_tool_entry(index, id);
        if let Some(function) = tool.get("function") {
            if let Some(name) = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
            {
                if entry
                    .name
                    .as_deref()
                    .is_some_and(|existing| existing != name)
                {
                    return Err(format!("tool call at index {index} changed its name"));
                }
                entry.name = Some(name.to_owned());
            }
            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                entry.delta_input_json.push_str(arguments);
            }
        }
        let key = self.openai_tool_keys_by_index[&index];
        self.stream_chat_tool(key, image_routes)
    }

    fn stream_chat_tool(
        &mut self,
        key: u64,
        image_routes: Option<&ImageRouteRegistry>,
    ) -> Result<Vec<Bytes>, String> {
        let mut events = Vec::new();
        let block = &self.tools[&key];
        let (Some(id), Some(name)) = (block.id.as_deref(), block.name.as_deref()) else {
            // Some providers send arguments before the call's identity.
            return Ok(events);
        };
        if !self.chat_tools.contains_key(&key) {
            let id = id.to_owned();
            let name = name.to_owned();
            events.extend(self.finish_text(AssistantMessagePhase::Commentary));
            let output_index = self.output.len();
            let item_id = format!("fc_{}", Uuid::new_v4().simple());
            let mut item = json!({"id":item_id,"call_id":id,"status":"in_progress"});
            if self.tool_names.is_custom(&name) {
                item["type"] = json!("custom_tool_call");
                item["name"] = json!(self.tool_names.to_codex_name(&name));
                item["input"] = json!("");
            } else if let Some(execution) = self.tool_names.tool_search_execution(&name) {
                item["type"] = json!("tool_search_call");
                item["execution"] = json!(execution);
                item["arguments"] = json!({});
            } else {
                item["type"] = json!("function_call");
                item["name"] = json!(self.tool_names.to_codex_name(&name));
                item["arguments"] = json!("");
                if let Some(namespace) = self.tool_names.to_codex_namespace(&name) {
                    item["namespace"] = json!(namespace);
                }
            }
            self.output.push(item.clone());
            self.chat_tools.insert(
                key,
                StreamingTool {
                    output_index,
                    item_id,
                    sent_bytes: 0,
                },
            );
            events.push(chat_event(
                "response.output_item.added",
                json!({
                    "type":"response.output_item.added","output_index":output_index,"item":item
                }),
            ));
        }
        let block = &self.tools[&key];
        let name = block
            .name
            .as_deref()
            .ok_or_else(|| "tool name is missing".to_owned())?;
        // Custom input is wrapped JSON, and image arguments are rewritten.
        // Emit their decoded/rewritten payload only after final validation.
        let deferred = self.tool_names.is_custom(name)
            || self.tool_names.tool_search_execution(name).is_some()
            || image_routes.is_some()
                && self.tool_names.to_codex_namespace(name) == Some("image_gen")
                && self.tool_names.to_codex_name(name) == "imagegen";
        if !deferred {
            let stream = self
                .chat_tools
                .get_mut(&key)
                .ok_or_else(|| "tool stream is missing".to_owned())?;
            let delta = &block.delta_input_json[stream.sent_bytes..];
            if !delta.is_empty() {
                events.push(chat_event(
                    "response.function_call_arguments.delta",
                    json!({
                        "type":"response.function_call_arguments.delta","item_id":stream.item_id,
                        "output_index":stream.output_index,"delta":delta
                    }),
                ));
                stream.sent_bytes = block.delta_input_json.len();
            }
        }
        Ok(events)
    }

    pub(super) fn finish_chat_tools(
        &mut self,
        image_routes: Option<&ImageRouteRegistry>,
    ) -> Result<Vec<Bytes>, String> {
        let mut keys = self.tools.keys().copied().collect::<Vec<_>>();
        keys.sort_unstable();
        let mut completed = Vec::new();
        let mut call_ids = self
            .output
            .iter()
            .filter(|item| item["status"] == "completed")
            .filter_map(|item| item["call_id"].as_str())
            .collect::<HashSet<_>>();
        // Validate the whole tool batch before any executable done item.
        for key in keys {
            let block = &self.tools[&key];
            let id = block
                .id
                .as_deref()
                .ok_or_else(|| format!("tool call at index {key} missing id"))?;
            let name = block
                .name
                .as_deref()
                .ok_or_else(|| format!("tool call at index {key} missing name"))?;
            if !call_ids.insert(id) {
                return Err(format!("duplicate tool call id: {id}"));
            }
            let stream = self
                .chat_tools
                .get(&key)
                .ok_or_else(|| format!("tool call at index {key} was not started"))?;
            let mut arguments = if block.delta_input_json.is_empty() {
                "{}".to_owned()
            } else {
                block.delta_input_json.clone()
            };
            if let Some(image_routes) = image_routes
                && self.tool_names.to_codex_namespace(name) == Some("image_gen")
                && self.tool_names.to_codex_name(name) == "imagegen"
            {
                arguments = image_routes.mark_arguments(&arguments)?;
            }
            let item = self.tool_item(id, name, &stream.item_id, &arguments)?;
            completed.push((key, item));
        }
        let mut events = Vec::new();
        for (key, item) in completed {
            let stream = self
                .chat_tools
                .remove(&key)
                .ok_or_else(|| "tool stream is missing".to_owned())?;
            let (field, delta_kind, done_kind) = if item["type"] == "custom_tool_call" {
                (
                    "input",
                    "response.custom_tool_call_input.delta",
                    "response.custom_tool_call_input.done",
                )
            } else {
                (
                    "arguments",
                    "response.function_call_arguments.delta",
                    "response.function_call_arguments.done",
                )
            };
            if let Some(payload) = item[field].as_str() {
                let delta = &payload[stream.sent_bytes..];
                if !delta.is_empty() {
                    events.push(chat_event(
                        delta_kind,
                        json!({"type":delta_kind,
                        "item_id":stream.item_id,"output_index":stream.output_index,"delta":delta}),
                    ));
                }
                let mut done = json!({"type":done_kind,"item_id":stream.item_id,
                    "output_index":stream.output_index,"name":item["name"]});
                done[field] = json!(payload);
                events.push(chat_event(done_kind, done));
            }
            self.output[stream.output_index] = item.clone();
            self.tools.remove(&key);
            events.push(chat_event("response.output_item.done", json!({
                "type":"response.output_item.done","output_index":stream.output_index,"item":item
            })));
        }
        Ok(events)
    }

    pub(super) fn incomplete_chat(&mut self, reason: &str) -> Bytes {
        for (key, stream) in &self.chat_tools {
            if self.output[stream.output_index]["type"] == "function_call"
                && let Some(block) = self.tools.get(key)
            {
                self.output[stream.output_index]["arguments"] = json!(block.delta_input_json);
            }
        }
        for item in &mut self.output {
            if item["status"] == "in_progress" {
                item["status"] = json!("incomplete");
            }
        }
        let mut response = self.completed_response();
        response["status"] = json!("incomplete");
        response["incomplete_details"] = json!({"reason":reason});
        chat_event(
            "response.incomplete",
            json!({"type":"response.incomplete","response":response}),
        )
    }
}
