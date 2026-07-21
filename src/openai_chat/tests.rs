use super::*;

#[test]
fn maps_responses_json_schema_to_chat_response_format() {
    let converted = responses_to_openai_chat(&json!({
        "model":"deepseek-chat",
        "stream":true,
        "input":"analyze",
        "text":{"format":{
            "type":"json_schema",
            "name":"panel",
            "strict":true,
            "schema":{
                "type":"object",
                "properties":{"findings":{"type":"array","items":{"type":"string"}}},
                "required":["findings"],
                "additionalProperties":false
            }
        }}
    }))
    .unwrap();
    assert_eq!(converted.request["response_format"]["type"], "json_schema");
    assert_eq!(
        converted.request["response_format"]["json_schema"]["name"],
        "panel"
    );
}

#[test]
fn converts_plaintext_agent_message_for_subagents() {
    let converted = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "input": [{
            "type": "agent_message",
            "author": "/root",
            "recipient": "/root/worker",
            "content": [{"type":"input_text","text":"Inspect the repository"}]
        }]
    }))
    .unwrap();

    assert_eq!(converted.request["messages"][0]["role"], "user");
    assert_eq!(
        converted.request["messages"][0]["content"],
        "[Agent message from /root to /root/worker]\nInspect the repository"
    );
}

#[test]
fn maps_codex_fast_service_tier_to_openai_priority() {
    for service_tier in ["priority", "fast"] {
        let converted = responses_to_openai_chat(&json!({
            "model": "deepseek-chat",
            "stream": true,
            "service_tier": service_tier,
            "input": "say hi"
        }))
        .unwrap();

        assert_eq!(converted.request["service_tier"], "priority");
    }
}

#[test]
fn preserves_codex_namespace_tool_name() {
    let converted = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "input": "hi",
        "tools": [{
            "type": "namespace",
            "name": "collaboration",
            "tools": [{
                "type": "function",
                "name": "spawn_agent",
                "strict": true,
                "parameters": {"type": "object"}
            }]
        }]
    }))
    .unwrap();

    assert_eq!(
        converted.request["tools"][0]["function"]["name"],
        "collaboration__spawn_agent"
    );
    assert_eq!(
        converted
            .tool_names
            .to_codex_name("collaboration__spawn_agent"),
        "spawn_agent"
    );
    assert_eq!(
        converted
            .tool_names
            .to_codex_namespace("collaboration__spawn_agent"),
        Some("collaboration")
    );
    assert_eq!(
        converted.request["tools"][0]["function"]["parameters"],
        json!({"type": "object"})
    );
    assert_eq!(converted.request["tools"][0]["function"]["strict"], true);
}

#[test]
fn preserves_namespaced_tool_loop_and_parallel_setting() {
    let converted = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "parallel_tool_calls": true,
        "tools": [{
            "type": "namespace",
            "name": "collaboration",
            "tools": [{
                "type": "function",
                "name": "spawn_agent",
                "parameters": {"type": "object"}
            }]
        }],
        "input": [
            {
                "type": "function_call",
                "call_id": "call_1",
                "namespace": "collaboration",
                "name": "spawn_agent",
                "arguments": {"task_name": "test", "message": "run"}
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "agent started"
            }
        ]
    }))
    .unwrap();

    assert_eq!(converted.request["parallel_tool_calls"], true);
    assert_eq!(
        converted.request["messages"][0]["tool_calls"][0]["function"]["name"],
        "collaboration__spawn_agent"
    );
    assert_eq!(converted.request["messages"][1]["tool_call_id"], "call_1");
}

#[test]
fn groups_parallel_tool_calls_in_one_assistant_message() {
    let converted = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "parallel_tool_calls": true,
        "input": [
            {"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"a\"}"},
            {"type":"function_call","call_id":"call_2","name":"read_file","arguments":"{\"path\":\"b\"}"},
            {"type":"function_call_output","call_id":"call_1","output":"a"},
            {"type":"function_call_output","call_id":"call_2","output":"b"}
        ]
    }))
    .unwrap();
    assert_eq!(converted.request["messages"].as_array().unwrap().len(), 3);
    assert_eq!(
        converted.request["messages"][0]["tool_calls"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(converted.request["messages"][1]["tool_call_id"], "call_1");
    assert_eq!(converted.request["messages"][2]["tool_call_id"], "call_2");
}

#[test]
fn converts_custom_and_tool_search_tools_for_chat_completions() {
    let converted = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "input": "hi",
        "tools": [
            {
                "type":"custom",
                "name":"apply_patch",
                "description":"Apply patch without JSON wrapping",
                "format":{"type":"grammar","syntax":"lark","definition":"start: PATCH"}
            },
            {
                "type":"tool_search",
                "execution":"client",
                "description":"Search deferred tools",
                "parameters":{"type":"object","properties":{"query":{"type":"string"}}}
            }
        ]
    }))
    .unwrap();

    assert_eq!(
        converted.request["tools"][0]["function"]["name"],
        "apply_patch"
    );
    assert!(converted.tool_names.is_custom("apply_patch"));
    assert_eq!(converted.request["tools"][0]["function"]["strict"], true);
    assert!(
        converted.request["tools"][0]["function"]["description"]
            .as_str()
            .unwrap()
            .contains("start: PATCH")
    );
    assert_eq!(
        converted.request["tools"][1]["function"]["name"],
        "tool_search"
    );
    assert_eq!(
        converted.tool_names.tool_search_execution("tool_search"),
        Some("client")
    );
}

#[test]
fn exposes_deferred_tools_to_chat_completions() {
    let converted = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "tools": [{
            "type":"tool_search",
            "execution":"client",
            "description":"Search tools",
            "parameters":{"type":"object","properties":{"query":{"type":"string"}}}
        }],
        "input": [
            {"type":"tool_search_call","call_id":"search_1","execution":"client","arguments":{"query":"calendar"}},
            {"type":"tool_search_output","call_id":"search_1","status":"completed","execution":"client","tools":[
                {"type":"namespace","name":"mcp__calendar","tools":[{"type":"function","name":"create_event","parameters":{"type":"object"}}]}
            ]}
        ]
    }))
    .unwrap();
    assert_eq!(converted.request["tools"].as_array().unwrap().len(), 2);
    assert_eq!(
        converted.request["tools"][1]["function"]["name"],
        "mcp__calendar__create_event"
    );
    assert_eq!(
        converted
            .tool_names
            .to_codex_namespace("mcp__calendar__create_event"),
        Some("mcp__calendar")
    );
}

#[test]
fn rejects_tools_chat_completions_cannot_execute() {
    let error = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "input": "hi",
        "tools": [{"type":"computer_use_preview"}]
    }))
    .unwrap_err();
    assert!(error.to_string().contains("unsupported tool type"));
}

#[test]
fn omits_unavailable_hosted_web_search_tool() {
    let converted = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "input": "hi",
        "tools": [{"type":"web_search","external_web_access":true}]
    }))
    .unwrap();

    assert!(converted.request.get("tools").is_none());
}

#[test]
fn omits_legacy_openai_hosted_image_generation_tool() {
    let converted = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "parallel_tool_calls": true,
        "tool_choice": "auto",
        "input": "hi",
        "tools": [{"type":"image_generation","output_format":"png"}]
    }))
    .unwrap();
    assert!(converted.request.get("tools").is_none());
    assert!(converted.request.get("tool_choice").is_none());
    assert!(converted.request.get("parallel_tool_calls").is_none());
}

#[test]
fn ignores_provider_native_history_when_switching_to_custom_model() {
    let converted = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "input": [
            {"type":"reasoning","encrypted_content":"opaque","summary":[]},
            {"type":"web_search_call","id":"ws_1","status":"completed"},
            {"type":"image_generation_call","id":"ig_1","status":"completed","result":"base64"},
            {"type":"tool_search_call","execution":"server","call_id":null,"arguments":{"paths":["crm"]}},
            {"type":"tool_search_output","execution":"server","call_id":null,"status":"completed","tools":[]},
            {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}
        ]
    }))
    .unwrap();
    assert_eq!(converted.request["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        converted.request["messages"][0]["content"][0]["text"],
        "continue"
    );
}

#[test]
fn rejects_messages_without_content() {
    let error = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "input": [{"type":"message","role":"user"}]
    }))
    .unwrap_err();

    assert!(error.to_string().contains("message missing content"));
}

#[test]
fn preserves_multimodal_tool_outputs_for_chat_completions() {
    let converted = responses_to_openai_chat(&json!({
        "model": "deepseek-chat",
        "stream": true,
        "input": [
            {"type":"function_call","call_id":"call_1","name":"view_image","arguments":"{\"path\":\"/tmp/a.png\"}"},
            {"type":"function_call_output","call_id":"call_1","output":[
                {"type":"input_text","text":"image loaded"},
                {"type":"input_image","image_url":"data:image/png;base64,AAAA","detail":"original"}
            ]}
        ]
    }))
    .unwrap();
    assert_eq!(
        converted.request["messages"][1]["content"],
        json!([
            {"type":"text","text":"image loaded"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}
        ])
    );
}
