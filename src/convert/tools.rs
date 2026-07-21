use super::tool_map::ToolNameMap;
use super::*;

pub(crate) fn collect_active_tools(body: &Value) -> Result<Value, GatewayError> {
    let mut active_tools = match body.get("tools") {
        Some(Value::Array(tools)) => tools.clone(),
        Some(_) => {
            return Err(GatewayError::BadRequest(
                "tools must be an array".to_owned(),
            ));
        }
        None => Vec::new(),
    };
    let Some(Value::Array(input)) = body.get("input") else {
        return Ok(Value::Array(active_tools));
    };
    for item in input {
        let item_type = item.get("type").and_then(Value::as_str);
        let discovered_tools = match item_type {
            Some("tool_search_output")
                if item.get("execution").and_then(Value::as_str) == Some("client") =>
            {
                item.get("tools").and_then(Value::as_array).ok_or_else(|| {
                    GatewayError::BadRequest("tool_search_output missing tools".to_owned())
                })?
            }
            Some("additional_tools") => {
                item.get("tools").and_then(Value::as_array).ok_or_else(|| {
                    GatewayError::BadRequest("additional_tools missing tools".to_owned())
                })?
            }
            _ => continue,
        };
        // Codex retains every tool search result, and overlapping searches can
        // return the same definition again in later turns.
        for tool in discovered_tools {
            if tool.get("type").and_then(Value::as_str) == Some("namespace") {
                let namespace = tool.get("name").and_then(Value::as_str).ok_or_else(|| {
                    GatewayError::BadRequest("namespace tool missing name".to_owned())
                })?;
                if let Some(existing_index) = active_tools.iter().position(|existing| {
                    existing.get("type").and_then(Value::as_str) == Some("namespace")
                        && existing.get("name").and_then(Value::as_str) == Some(namespace)
                }) {
                    let mut existing_metadata = active_tools[existing_index].clone();
                    existing_metadata
                        .as_object_mut()
                        .expect("namespace tool must be an object")
                        .remove("tools");
                    let mut discovered_metadata = tool.clone();
                    discovered_metadata
                        .as_object_mut()
                        .expect("namespace tool must be an object")
                        .remove("tools");
                    if existing_metadata != discovered_metadata {
                        return Err(GatewayError::BadRequest(format!(
                            "conflicting namespace definitions across tool search history: {namespace}"
                        )));
                    }

                    let discovered_nested =
                        tool.get("tools").and_then(Value::as_array).ok_or_else(|| {
                            GatewayError::BadRequest("namespace tool missing tools".to_owned())
                        })?;
                    let existing_nested = active_tools[existing_index]
                        .get_mut("tools")
                        .and_then(Value::as_array_mut)
                        .ok_or_else(|| {
                            GatewayError::BadRequest("namespace tool missing tools".to_owned())
                        })?;
                    for nested in discovered_nested {
                        let nested_name =
                            nested.get("name").and_then(Value::as_str).ok_or_else(|| {
                                GatewayError::BadRequest(
                                    "namespace function tool missing name".to_owned(),
                                )
                            })?;
                        if let Some(existing) = existing_nested.iter().find(|existing| {
                            existing.get("name").and_then(Value::as_str) == Some(nested_name)
                        }) {
                            if existing != nested {
                                return Err(GatewayError::BadRequest(format!(
                                    "conflicting definitions for discovered tool: {namespace}.{nested_name}"
                                )));
                            }
                        } else {
                            existing_nested.push(nested.clone());
                        }
                    }
                    continue;
                }
            }
            if !active_tools.contains(tool) {
                active_tools.push(tool.clone());
            }
        }
    }
    Ok(Value::Array(active_tools))
}

pub(super) fn convert_tools(
    tools: Option<&Value>,
    config: &GatewayConfig,
    use_mcp_bridge_names: bool,
    web_search_enabled: bool,
) -> Result<(Vec<Tool>, ToolNameMap), GatewayError> {
    let mut result = Vec::new();
    let mut names = ToolNameMap::default();
    let Some(Value::Array(tools)) = tools else {
        return Ok((result, names));
    };
    let mut web_search_added = false;
    for tool in tools {
        match tool.get("type").and_then(Value::as_str) {
            Some("function")
                if is_codex_web_search_function(tool)
                    && web_search_enabled
                    && !web_search_added =>
            {
                result.push(web_search_server_tool(config, tool)?);
                web_search_added = true;
            }
            Some("function") if is_codex_web_search_function(tool) && web_search_enabled => {}
            Some("function") if is_codex_web_search_function(tool) => {
                tracing::debug!("omitting unavailable hosted web_search tool");
            }
            Some("function") => {
                let (converted, codex_name) =
                    convert_function_tool(tool, None, use_mcp_bridge_names)?;
                let anthropic_name = converted
                    .get("name")
                    .and_then(Value::as_str)
                    .expect("converted function tool missing name")
                    .to_owned();
                names.insert(anthropic_name, codex_name)?;
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
                    let (converted, codex_name) =
                        convert_function_tool(nested, Some(namespace), use_mcp_bridge_names)?;
                    let anthropic_name = converted
                        .get("name")
                        .and_then(Value::as_str)
                        .expect("converted namespace tool missing name")
                        .to_owned();
                    names.insert_namespaced(anthropic_name, namespace.to_owned(), codex_name)?;
                    result.push(converted);
                }
            }
            Some("custom") => {
                let (converted, codex_name) = convert_custom_tool(tool, use_mcp_bridge_names)?;
                let upstream_name = converted
                    .get("name")
                    .and_then(Value::as_str)
                    .expect("converted custom tool missing name")
                    .to_owned();
                names.insert_custom(upstream_name, codex_name)?;
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
                let (converted, codex_name) =
                    convert_function_tool(&function_tool, None, use_mcp_bridge_names)?;
                let upstream_name = converted
                    .get("name")
                    .and_then(Value::as_str)
                    .expect("converted tool_search missing name")
                    .to_owned();
                names.insert_tool_search(upstream_name, codex_name, execution)?;
                result.push(converted);
            }
            Some("web_search" | "web_search_preview")
                if web_search_enabled && !web_search_added =>
            {
                result.push(web_search_server_tool(config, tool)?);
                web_search_added = true;
            }
            Some("web_search" | "web_search_preview") if web_search_enabled => {}
            Some("web_search" | "web_search_preview") => {
                tracing::debug!("omitting unavailable hosted web_search tool");
            }
            Some("image_generation") => {
                tracing::debug!("omitting legacy OpenAI-hosted image_generation tool");
            }
            Some(other) => {
                return Err(GatewayError::BadRequest(format!(
                    "unsupported tool type: {other}"
                )));
            }
            None => {
                return Err(GatewayError::BadRequest(
                    "tool definition missing type".to_owned(),
                ));
            }
        }
    }
    Ok((result, names))
}

pub(super) fn is_codex_web_search_function(tool: &Value) -> bool {
    matches!(
        tool.get("name").and_then(Value::as_str),
        Some("web_search" | "web_search_preview")
    )
}

pub(super) fn web_search_server_tool(
    config: &GatewayConfig,
    codex_tool: &Value,
) -> Result<Tool, GatewayError> {
    if codex_tool
        .get("external_web_access")
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Err(GatewayError::BadRequest(
            "Anthropic web_search cannot preserve Codex cached-search semantics".to_owned(),
        ));
    }
    if codex_tool
        .get("indexed_web_access")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Err(GatewayError::BadRequest(
            "Anthropic web_search cannot preserve Codex indexed-search semantics".to_owned(),
        ));
    }
    if codex_tool.get("search_context_size").is_some() {
        return Err(GatewayError::BadRequest(
            "Anthropic web_search has no equivalent for Codex search_context_size".to_owned(),
        ));
    }
    if let Some(content_types) = codex_tool.get("search_content_types") {
        let content_types = content_types.as_array().ok_or_else(|| {
            GatewayError::BadRequest("web_search search_content_types must be an array".to_owned())
        })?;
        if content_types
            .iter()
            .any(|content_type| content_type.as_str() != Some("text"))
        {
            return Err(GatewayError::BadRequest(
                "Anthropic web_search cannot preserve non-text Codex search content types"
                    .to_owned(),
            ));
        }
    }

    let mut web_search_tool = json!({"type": &config.web_search_tool_type, "name": "web_search"});
    if let Some(max_uses) = config.web_search_max_uses {
        web_search_tool["max_uses"] = json!(max_uses);
    }
    if let Some(filters) = codex_tool.get("filters") {
        let filters = filters.as_object().ok_or_else(|| {
            GatewayError::BadRequest("web_search filters must be an object".to_owned())
        })?;
        if let Some(allowed_domains) = filters.get("allowed_domains") {
            let allowed_domains = allowed_domains.as_array().ok_or_else(|| {
                GatewayError::BadRequest(
                    "web_search filters.allowed_domains must be an array".to_owned(),
                )
            })?;
            if allowed_domains.iter().any(|domain| !domain.is_string()) {
                return Err(GatewayError::BadRequest(
                    "web_search filters.allowed_domains must contain only strings".to_owned(),
                ));
            }
            web_search_tool["allowed_domains"] = Value::Array(allowed_domains.clone());
        }
    }
    if let Some(user_location) = codex_tool.get("user_location") {
        if !user_location.is_object() {
            return Err(GatewayError::BadRequest(
                "web_search user_location must be an object".to_owned(),
            ));
        }
        web_search_tool["user_location"] = user_location.clone();
    }
    Ok(web_search_tool)
}

pub(super) fn convert_function_tool(
    tool: &Value,
    namespace: Option<&str>,
    use_mcp_bridge_names: bool,
) -> Result<(Tool, String), GatewayError> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("function tool missing name".to_owned()))?;
    let codex_name = name.to_owned();
    let anthropic_name = namespace.map_or_else(
        || upstream_client_tool_name(name, use_mcp_bridge_names),
        |namespace| {
            upstream_client_tool_name(&format!("{namespace}__{name}"), use_mcp_bridge_names)
        },
    );
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| namespace.map(|namespace| format!("Tool {name} in namespace {namespace}.")));
    let input_schema = tool
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
    let mut converted = match description {
        Some(description) => json!({
            "name": anthropic_name,
            "description": description,
            "input_schema": input_schema
        }),
        None => json!({
            "name": anthropic_name,
            "input_schema": input_schema
        }),
    };
    if let Some(strict) = tool.get("strict").and_then(Value::as_bool) {
        converted["strict"] = json!(strict);
    }
    Ok((converted, codex_name))
}

pub(super) fn convert_custom_tool(
    tool: &Value,
    use_mcp_bridge_names: bool,
) -> Result<(Tool, String), GatewayError> {
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("custom tool missing name".to_owned()))?;
    let upstream_name = upstream_client_tool_name(name, use_mcp_bridge_names);
    let description = custom_tool_description(tool)?;
    Ok((
        json!({
            "name": upstream_name,
            "description": description,
            "strict": true,
            "input_schema": {
                "type": "object",
                "properties": {"input": {"type": "string"}},
                "required": ["input"],
                "additionalProperties": false
            }
        }),
        name.to_owned(),
    ))
}

pub(crate) fn custom_tool_description(tool: &Value) -> Result<String, GatewayError> {
    let mut description = tool
        .get("description")
        .and_then(Value::as_str)
        .ok_or_else(|| GatewayError::BadRequest("custom tool missing description".to_owned()))?
        .to_owned();
    description.push_str(
        "\n\nFor this gateway function call, put the complete freeform payload unchanged in the input string field.",
    );
    if let Some(format) = tool.get("format") {
        let format_type = format.get("type").and_then(Value::as_str).ok_or_else(|| {
            GatewayError::BadRequest("custom tool format missing type".to_owned())
        })?;
        let syntax = format
            .get("syntax")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                GatewayError::BadRequest("custom tool format missing syntax".to_owned())
            })?;
        let definition = format
            .get("definition")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                GatewayError::BadRequest("custom tool format missing definition".to_owned())
            })?;
        description.push_str(&format!(
            "\nThe input must satisfy the {format_type} {syntax} grammar:\n{definition}"
        ));
    }
    Ok(description)
}

pub fn sanitize_tool_name(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() {
        sanitized.push_str("tool");
    }
    if sanitized.len() <= 64 {
        return sanitized;
    }
    let hash = name
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    sanitized.truncate(47);
    format!("{sanitized}_{hash:016x}")
}

pub(super) fn upstream_client_tool_name(name: &str, use_mcp_bridge_names: bool) -> String {
    let sanitized = sanitize_tool_name(name);
    if !use_mcp_bridge_names || sanitized.starts_with("mcp__") {
        return sanitized;
    }
    sanitize_tool_name(&format!("mcp__codex__{sanitized}"))
}

pub(super) fn convert_tool_choice(
    value: Option<&Value>,
    parallel_tool_calls: Option<bool>,
) -> Option<Value> {
    let mut choice = match value {
        Some(Value::String(choice)) if choice == "auto" => Some(json!({"type": "auto"})),
        Some(Value::String(choice)) if choice == "required" || choice == "any" => {
            Some(json!({"type": "any"}))
        }
        Some(Value::String(choice)) if choice == "none" => Some(json!({"type": "none"})),
        _ => None,
    };
    if parallel_tool_calls == Some(false)
        && !matches!(
            choice
                .as_ref()
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str),
            Some("none")
        )
    {
        let choice = choice.get_or_insert_with(|| json!({"type":"auto"}));
        choice["disable_parallel_tool_use"] = json!(true);
    }
    choice
}
