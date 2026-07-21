use super::*;

pub(super) fn convert_tools(
    tools: Option<&Value>,
) -> Result<(Vec<Value>, ToolNameMap), GatewayError> {
    let Some(Value::Array(tools)) = tools else {
        return Ok((Vec::new(), ToolNameMap::default()));
    };
    let mut result = Vec::new();
    let mut tool_names = ToolNameMap::default();
    for tool in tools {
        match tool.get("type").and_then(Value::as_str) {
            Some("function") => {
                let (converted, codex_name) = convert_function_tool(tool, None)?;
                let sanitized = converted["function"]["name"]
                    .as_str()
                    .expect("converted function tool missing name")
                    .to_owned();
                tool_names.insert(sanitized, codex_name)?;
                result.push(converted);
            }
            Some("namespace") => {
                let namespace = tool.get("name").and_then(Value::as_str).ok_or_else(|| {
                    GatewayError::BadRequest("namespace tool missing name".to_owned())
                })?;
                let nested_tools =
                    tool.get("tools").and_then(Value::as_array).ok_or_else(|| {
                        GatewayError::BadRequest("namespace tool missing tools".to_owned())
                    })?;
                for nested in nested_tools {
                    let (converted, codex_name) = convert_function_tool(nested, Some(namespace))?;
                    let sanitized = converted["function"]["name"]
                        .as_str()
                        .expect("converted namespace tool missing name")
                        .to_owned();
                    tool_names.insert_namespaced(sanitized, namespace.to_owned(), codex_name)?;
                    result.push(converted);
                }
            }
            Some("custom") => {
                let (converted, codex_name) = convert_custom_tool(tool)?;
                let upstream_name = converted["function"]["name"]
                    .as_str()
                    .expect("converted custom tool missing name")
                    .to_owned();
                tool_names.insert_custom(upstream_name, codex_name)?;
                result.push(converted);
            }
            Some("tool_search") => {
                let execution = tool
                    .get("execution")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        GatewayError::BadRequest("tool_search missing execution".to_owned())
                    })?
                    .to_owned();
                if execution != "client" {
                    return Err(GatewayError::BadRequest(format!(
                        "unsupported tool_search execution: {execution}"
                    )));
                }
                let mut function_tool = tool.clone();
                function_tool["name"] = json!("tool_search");
                let (converted, codex_name) = convert_function_tool(&function_tool, None)?;
                let upstream_name = converted["function"]["name"]
                    .as_str()
                    .expect("converted tool_search missing name")
                    .to_owned();
                tool_names.insert_tool_search(upstream_name, codex_name, execution)?;
                result.push(converted);
            }
            Some("web_search" | "web_search_preview") => {
                tracing::debug!("omitting unavailable hosted web_search tool");
            }
            Some("image_generation") => {
                tracing::debug!("omitting legacy OpenAI-hosted image_generation tool");
            }
            Some(other) => {
                return Err(GatewayError::BadRequest(format!(
                    "unsupported tool type for OpenAI Chat Completions upstream: {other}"
                )));
            }
            None => {
                return Err(GatewayError::BadRequest(
                    "tool definition missing type".to_owned(),
                ));
            }
        }
    }
    Ok((result, tool_names))
}

pub(super) fn convert_function_tool(
    tool: &Value,
    namespace: Option<&str>,
) -> Result<(Value, String), GatewayError> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("function tool missing name".to_owned()))?;
    let codex_name = name.to_owned();
    let sanitized_name = namespace.map_or_else(
        || sanitize_tool_name(name),
        |namespace| sanitize_tool_name(&format!("{namespace}__{name}")),
    );
    let description = tool
        .get("description")
        .cloned()
        .unwrap_or_else(|| json!(""));
    let parameters = tool
        .get("parameters")
        .or_else(|| tool.get("input_schema"))
        .cloned()
        .unwrap_or_else(|| json!({"type":"object","properties":{}}));
    let mut converted = json!({
        "type": "function",
        "function": {
            "name": sanitized_name,
            "description": description,
            "parameters": parameters
        }
    });
    if let Some(strict) = tool.get("strict").and_then(Value::as_bool) {
        converted["function"]["strict"] = json!(strict);
    }
    Ok((converted, codex_name))
}

pub(super) fn convert_custom_tool(tool: &Value) -> Result<(Value, String), GatewayError> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("custom tool missing name".to_owned()))?;
    let description = custom_tool_description(tool)?;
    Ok((
        json!({
            "type": "function",
            "function": {
                "name": sanitize_tool_name(name),
                "description": description,
                "strict": true,
                "parameters": {
                    "type": "object",
                    "properties": {"input": {"type": "string"}},
                    "required": ["input"],
                    "additionalProperties": false
                }
            }
        }),
        name.to_owned(),
    ))
}
