use std::time::Duration;

use serde_json::json;

use crate::config::{
    GatewayConfig, ProviderPreset, ThinkingMode, UpstreamAuthHeader, UpstreamKind,
};

use super::*;

fn config() -> GatewayConfig {
    GatewayConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        provider_preset: ProviderPreset::Custom,
        upstream_kind: UpstreamKind::AnthropicMessages,
        upstream_base_url: "http://127.0.0.1".to_owned(),
        upstream_messages_path: "/v1/messages".to_owned(),
        upstream_models_path: "/v1/models".to_owned(),
        upstream_image_generation_path: None,
        upstream_api_key: "test".to_owned(),
        quota_url: None,
        quota_username: None,
        official_responses_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
        codex_auth_path: std::path::PathBuf::from("/tmp/codex-auth.json"),
        upstream_auth_header: UpstreamAuthHeader::AuthorizationBearer,
        anthropic_version: "2023-06-01".to_owned(),
        anthropic_beta: None,
        gateway_api_key: None,
        accept_codex_oauth: true,
        default_max_tokens: 8192,
        default_context_window: 1_000_000,
        request_timeout: Duration::from_secs(30),
        thinking_mode: ThinkingMode::Off,
        enable_web_search_tool: false,
        web_search_tool_type: "web_search_20250305".to_owned(),
        web_search_max_uses: Some(3),
        fusion_profiles: Vec::new(),
    }
}

#[test]
fn maps_responses_json_schema_to_anthropic_output_config() {
    let converted = responses_to_anthropic(
        &json!({
            "model":"Claude Sonnet 5",
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
        }),
        &config(),
    )
    .unwrap();
    assert_eq!(
        converted.request.output_config.unwrap()["format"]["type"],
        "json_schema"
    );
}

#[test]
fn converts_codex_tool_loop_input() {
    let body = json!({
        "model": "DeepSeek-V4-Flash",
        "stream": true,
        "instructions": "base",
        "input": [
            {"type":"message","role":"developer","content":[{"type":"input_text","text":"dev"}]},
            {"type":"message","role":"user","content":[{"type":"input_text","text":"run pwd"}]},
            {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
            {"type":"function_call_output","call_id":"call_1","output":"/tmp"}
        ],
        "tools": [{"type":"function","name":"exec_command","description":"run","parameters":{"type":"object","properties":{"cmd":{"type":"string"}}}}]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(converted.request.model, "DeepSeek-V4-Flash");
    assert_eq!(converted.request.system.as_ref().unwrap().len(), 2);
    assert_eq!(converted.request.messages.len(), 3);
    assert_eq!(
        converted.request.messages[1].content[0],
        ContentBlock::ToolUse {
            id: "call_1".to_owned(),
            name: "exec_command".to_owned(),
            input: json!({"cmd":"pwd"})
        }
    );
    assert_eq!(
        converted.request.messages[2].content[0],
        ContentBlock::ToolResult {
            tool_use_id: "call_1".to_owned(),
            content: json!("/tmp")
        }
    );
    assert_eq!(converted.request.tools[0]["name"], "exec_command");
}

#[test]
fn maps_codex_fast_service_tier_to_anthropic_speed() {
    for service_tier in ["priority", "fast"] {
        let converted = responses_to_anthropic(
            &json!({
                "model": "Claude Sonnet 5",
                "stream": true,
                "service_tier": service_tier,
                "input": "say hi"
            }),
            &config(),
        )
        .unwrap();

        assert_eq!(converted.request.speed.as_deref(), Some("fast"));
    }
}

#[test]
fn uses_mcp_tool_names_for_baidu_fable_bridge_and_history() {
    let mut config = config();
    config.provider_preset = ProviderPreset::BaiduOneApi;
    let body = json!({
        "model": "Fable 5",
        "stream": true,
        "input": [
            {"type":"message","role":"user","content":[{"type":"input_text","text":"run pwd"}]},
            {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"},
            {"type":"function_call_output","call_id":"call_1","output":"/tmp"}
        ],
        "tools": [
            {"type":"function","name":"exec_command","parameters":{"type":"object"}},
            {"type":"namespace","name":"collaboration","tools":[
                {"type":"function","name":"spawn_agent","parameters":{"type":"object"}}
            ]},
            {"type":"namespace","name":"mcp__fff","tools":[
                {"type":"function","name":"find_files","parameters":{"type":"object"}}
            ]},
            {"type":"custom","name":"apply_patch","description":"Apply a patch"},
            {"type":"tool_search","execution":"client","description":"Search tools","parameters":{"type":"object"}}
        ]
    });

    let converted = responses_to_anthropic(&body, &config).unwrap();
    assert_eq!(
        converted.request.messages[1].content[0],
        ContentBlock::ToolUse {
            id: "call_1".to_owned(),
            name: "mcp__codex__exec_command".to_owned(),
            input: json!({"cmd":"pwd"})
        }
    );
    let tool_names = converted
        .request
        .tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names,
        vec![
            "mcp__codex__exec_command",
            "mcp__codex__collaboration__spawn_agent",
            "mcp__fff__find_files",
            "mcp__codex__apply_patch",
            "mcp__codex__tool_search"
        ]
    );
    assert_eq!(
        converted
            .tool_names
            .to_codex_name("mcp__codex__exec_command"),
        "exec_command"
    );
    assert_eq!(
        converted
            .tool_names
            .to_codex_namespace("mcp__codex__collaboration__spawn_agent"),
        Some("collaboration")
    );
    assert!(converted.tool_names.is_custom("mcp__codex__apply_patch"));
    assert_eq!(
        converted
            .tool_names
            .tool_search_execution("mcp__codex__tool_search"),
        Some("client")
    );
}

#[test]
fn limits_mcp_bridge_names_to_baidu_fable_models() {
    for (provider, model, expected_tool_name) in [
        (
            ProviderPreset::BaiduOneApi,
            "Fable 5",
            "mcp__codex__exec_command",
        ),
        (ProviderPreset::Custom, "Fable 5", "exec_command"),
        (
            ProviderPreset::BaiduOneApi,
            "Claude Sonnet 5",
            "exec_command",
        ),
    ] {
        let mut config = config();
        config.provider_preset = provider;
        let converted = responses_to_anthropic(
            &json!({
                "model": model,
                "stream": true,
                "input": "run pwd",
                "tools": [{
                    "type": "function",
                    "name": "exec_command",
                    "parameters": {"type": "object"}
                }]
            }),
            &config,
        )
        .unwrap();

        assert_eq!(converted.request.tools[0]["name"], expected_tool_name);
    }
}

#[test]
fn converts_plaintext_agent_message_for_subagents() {
    let body = json!({
        "model": "DeepSeek-V4-Flash",
        "stream": true,
        "input": [{
            "type": "agent_message",
            "author": "/root",
            "recipient": "/root/run_uname",
            "content": [{"type":"input_text","text":"Run uname -a"}]
        }]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(converted.request.messages[0].role, "user");
    assert_eq!(
        converted.request.messages[0].content[0],
        ContentBlock::Text {
            text: "[Agent message from /root to /root/run_uname]\nRun uname -a".to_owned()
        }
    );
}

#[test]
fn materializes_v2_agent_message_payload_for_custom_upstream() {
    let body = json!({
        "model": "DeepSeek-V4-Flash",
        "stream": true,
        "input": [{
            "type": "agent_message",
            "author": "/root",
            "recipient": "/root/worker",
            "content": [
                {"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"},
                {"type":"encrypted_content","encrypted_content":"Inspect the workspace"}
            ]
        }]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(
        converted.request.messages[0].content[0],
        ContentBlock::Text {
            text: "[Agent message from /root to /root/worker]\nMessage Type: NEW_TASK\nPayload:\n\nInspect the workspace".to_owned()
        }
    );
}

#[test]
fn flattens_namespace_tools_for_anthropic() {
    let body = json!({
        "model": "m",
        "stream": true,
        "input": "hi",
        "tools": [{"type":"namespace","name":"mcp__node_repl","tools":[{"type":"function","name":"js","strict":true,"parameters":{"type":"object"}}]}]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(converted.request.tools[0]["name"], "mcp__node_repl__js");
    assert_eq!(converted.request.tools[0]["strict"], true);
    assert_eq!(
        converted.tool_names.to_codex_name("mcp__node_repl__js"),
        "js"
    );
    assert_eq!(
        converted
            .tool_names
            .to_codex_namespace("mcp__node_repl__js"),
        Some("mcp__node_repl")
    );
}

#[test]
fn flattens_namespaced_function_call_history_for_upstream() {
    let body = json!({
        "model": "m",
        "stream": true,
        "input": [{
            "type":"function_call",
            "call_id":"call_1",
            "namespace":"collaboration",
            "name":"spawn_agent",
            "arguments":"{\"task_name\":\"test\"}"
        }]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(
        converted.request.messages[0].content[0],
        ContentBlock::ToolUse {
            id: "call_1".to_owned(),
            name: "collaboration__spawn_agent".to_owned(),
            input: json!({"task_name":"test"})
        }
    );
}

#[test]
fn converts_custom_and_tool_search_tools_for_anthropic() {
    let body = json!({
        "model": "m",
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
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(converted.request.tools[0]["name"], "apply_patch");
    assert_eq!(
        converted.request.tools[0]["input_schema"]["required"],
        json!(["input"])
    );
    assert_eq!(converted.request.tools[0]["strict"], true);
    assert!(
        converted.request.tools[0]["description"]
            .as_str()
            .unwrap()
            .contains("start: PATCH")
    );
    assert!(converted.tool_names.is_custom("apply_patch"));
    assert_eq!(converted.request.tools[1]["name"], "tool_search");
    assert_eq!(
        converted.tool_names.tool_search_execution("tool_search"),
        Some("client")
    );
}

#[test]
fn converts_custom_and_tool_search_outputs_for_anthropic() {
    let body = json!({
        "model": "m",
        "stream": true,
        "input": [
            {"type":"custom_tool_call","call_id":"custom_1","name":"apply_patch","input":"*** Begin Patch"},
            {"type":"custom_tool_call_output","call_id":"custom_1","output":[{"type":"input_text","text":"Done"}]},
            {"type":"tool_search_call","call_id":"search_1","execution":"client","arguments":{"query":"calendar"}},
            {"type":"tool_search_output","call_id":"search_1","status":"completed","execution":"client","tools":[{"type":"function","name":"create_event"}]}
        ]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(converted.request.messages.len(), 4);
    assert_eq!(
        converted.request.messages[0].content[0],
        ContentBlock::ToolUse {
            id: "custom_1".to_owned(),
            name: "apply_patch".to_owned(),
            input: json!({"input":"*** Begin Patch"})
        }
    );
    assert_eq!(
        converted.request.messages[1].content[0],
        ContentBlock::ToolResult {
            tool_use_id: "custom_1".to_owned(),
            content: json!([{"type":"text","text":"Done"}])
        }
    );
    assert_eq!(
        converted.request.messages[2].content[0],
        ContentBlock::ToolUse {
            id: "search_1".to_owned(),
            name: "tool_search".to_owned(),
            input: json!({"query":"calendar"})
        }
    );
}

#[test]
fn exposes_deferred_and_additional_tools_to_anthropic() {
    let body = json!({
        "model": "m",
        "stream": true,
        "tools": [{
            "type":"tool_search",
            "execution":"client",
            "description":"Search tools",
            "parameters":{"type":"object","properties":{"query":{"type":"string"}}}
        }],
        "input": [
            {"type":"message","role":"user","content":[{"type":"input_text","text":"create event"}]},
            {"type":"tool_search_call","call_id":"search_1","execution":"client","arguments":{"query":"calendar"}},
            {"type":"tool_search_output","call_id":"search_1","status":"completed","execution":"client","tools":[
                {"type":"function","name":"calendar_create","description":"Create event","parameters":{"type":"object"}},
                {"type":"namespace","name":"mcp__calendar","tools":[{"type":"function","name":"delete_event","parameters":{"type":"object"}}]}
            ]},
            {"type":"additional_tools","role":"developer","tools":[
                {"type":"function","name":"calendar_list","description":"List events","parameters":{"type":"object"}}
            ]}
        ]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    let tool_names = converted
        .request
        .tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names,
        vec![
            "tool_search",
            "calendar_create",
            "mcp__calendar__delete_event",
            "calendar_list"
        ]
    );
    assert_eq!(
        converted
            .tool_names
            .to_codex_namespace("mcp__calendar__delete_event"),
        Some("mcp__calendar")
    );
}

#[test]
fn merges_overlapping_namespace_tools_across_search_history() {
    let reset_tool = json!({
        "type": "function",
        "name": "js_reset",
        "description": "Reset the JavaScript kernel",
        "parameters": {"type": "object"}
    });
    let body = json!({
        "model": "m",
        "stream": true,
        "tools": [{
            "type": "tool_search",
            "execution": "client",
            "description": "Search tools",
            "parameters": {"type": "object", "properties": {"query": {"type": "string"}}}
        }],
        "input": [
            {
                "type": "tool_search_call",
                "call_id": "search_1",
                "execution": "client",
                "arguments": {"query": "computer-use control mac app"}
            },
            {
                "type": "tool_search_output",
                "call_id": "search_1",
                "status": "completed",
                "execution": "client",
                "tools": [{
                    "type": "namespace",
                    "name": "mcp__node_repl",
                    "description": "Persistent JavaScript kernel",
                    "tools": [
                        reset_tool.clone(),
                        {
                            "type": "function",
                            "name": "js",
                            "parameters": {"type": "object"}
                        }
                    ]
                }]
            },
            {
                "type": "tool_search_call",
                "call_id": "search_2",
                "execution": "client",
                "arguments": {"query": "computer-use screenshot click keyboard macOS"}
            },
            {
                "type": "tool_search_output",
                "call_id": "search_2",
                "status": "completed",
                "execution": "client",
                "tools": [{
                    "type": "namespace",
                    "name": "mcp__node_repl",
                    "description": "Persistent JavaScript kernel",
                    "tools": [
                        reset_tool,
                        {
                            "type": "function",
                            "name": "js_add_node_module_dir",
                            "parameters": {"type": "object"}
                        }
                    ]
                }]
            }
        ]
    });

    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(converted.request.tools.len(), 4);
    assert_eq!(converted.request.tools[0]["name"], "tool_search");
    assert_eq!(
        converted.request.tools[1]["name"],
        "mcp__node_repl__js_reset"
    );
    assert_eq!(converted.request.tools[2]["name"], "mcp__node_repl__js");
    assert_eq!(
        converted.request.tools[3]["name"],
        "mcp__node_repl__js_add_node_module_dir"
    );
}

#[test]
fn rejects_messages_without_content() {
    let error = responses_to_anthropic(
        &json!({
            "model": "m",
            "stream": true,
            "input": [{"type":"message","role":"user"}]
        }),
        &config(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("message missing content"));
}

#[test]
fn rejects_conflicting_namespace_tools_across_search_history() {
    let body = json!({
        "input": [
            {
                "type": "tool_search_output",
                "execution": "client",
                "tools": [{
                    "type": "namespace",
                    "name": "mcp__calendar",
                    "tools": [{
                        "type": "function",
                        "name": "create_event",
                        "parameters": {"type": "object"}
                    }]
                }]
            },
            {
                "type": "tool_search_output",
                "execution": "client",
                "tools": [{
                    "type": "namespace",
                    "name": "mcp__calendar",
                    "tools": [{
                        "type": "function",
                        "name": "create_event",
                        "parameters": {"type": "object", "required": ["title"]}
                    }]
                }]
            }
        ]
    });

    let error = collect_active_tools(&body).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("conflicting definitions for discovered tool")
    );
}

#[test]
fn ignores_provider_native_history_when_switching_to_custom_model() {
    let body = json!({
        "model": "m",
        "stream": true,
        "input": [
            {"type":"reasoning","encrypted_content":"opaque","summary":[]},
            {"type":"web_search_call","id":"ws_1","status":"completed"},
            {"type":"image_generation_call","id":"ig_1","status":"completed","result":"base64"},
            {"type":"tool_search_call","execution":"server","call_id":null,"arguments":{"paths":["crm"]}},
            {"type":"tool_search_output","execution":"server","call_id":null,"status":"completed","tools":[]},
            {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}
        ]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(converted.request.messages.len(), 1);
    assert_eq!(
        converted.request.messages[0].content[0],
        ContentBlock::Text {
            text: "continue".to_owned()
        }
    );
}

#[test]
fn preserves_multimodal_tool_outputs_for_anthropic() {
    let body = json!({
        "model": "m",
        "stream": true,
        "input": [
            {"type":"function_call","call_id":"call_1","name":"view_image","arguments":"{\"path\":\"/tmp/a.png\"}"},
            {"type":"function_call_output","call_id":"call_1","output":[
                {"type":"input_text","text":"image loaded"},
                {"type":"input_image","image_url":"data:image/png;base64,AAAA","detail":"original"}
            ]}
        ]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(
        converted.request.messages[1].content[0],
        ContentBlock::ToolResult {
            tool_use_id: "call_1".to_owned(),
            content: json!([
                {"type":"text","text":"image loaded"},
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}
            ])
        }
    );
}

#[test]
fn rejects_incomplete_client_tool_history() {
    for item in [
        json!({"type":"function_call","call_id":"call_1","name":"exec_command"}),
        json!({"type":"function_call_output","call_id":"call_1"}),
        json!({"type":"tool_search_call","execution":"client","call_id":"search_1"}),
        json!({"type":"tool_search_output","execution":"client","call_id":"search_1","status":"completed"}),
    ] {
        let error = responses_to_anthropic(
            &json!({"model":"m","stream":true,"input":[item]}),
            &config(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("missing") || error.to_string().contains("must be"));
    }
}

#[test]
fn auto_thinking_maps_exhigh_to_adaptive_max_for_new_claude() {
    let mut config = config();
    config.thinking_mode = ThinkingMode::Auto;
    let body = json!({
        "model": "Claude Sonnet 5",
        "stream": true,
        "max_output_tokens": 64,
        "reasoning": {"effort": "exhigh"},
        "input": "hi"
    });
    let converted = responses_to_anthropic(&body, &config).unwrap();
    assert_eq!(converted.request.thinking.unwrap()["type"], "adaptive");
    assert_eq!(converted.request.output_config.unwrap()["effort"], "max");
}

#[test]
fn adaptive_thinking_maps_common_effort_levels() {
    let mut config = config();
    config.thinking_mode = ThinkingMode::Adaptive;
    for (input, expected) in [
        ("minimal", "low"),
        ("low", "low"),
        ("medium", "medium"),
        ("high", "high"),
        ("xhigh", "max"),
        ("exhigh", "max"),
        ("max", "max"),
    ] {
        let body = json!({
            "model": "Claude Sonnet 5",
            "stream": true,
            "reasoning": {"effort": input},
            "input": "hi"
        });
        let converted = responses_to_anthropic(&body, &config).unwrap();
        assert_eq!(
            converted.request.output_config.unwrap()["effort"],
            expected,
            "input effort {input}"
        );
    }
}

#[test]
fn adaptive_thinking_rejects_misspelled_medium() {
    let mut config = config();
    config.thinking_mode = ThinkingMode::Adaptive;
    let body = json!({
        "model": "Claude Sonnet 5",
        "stream": true,
        "reasoning": {"effort": "meddium"},
        "input": "hi"
    });
    let err = responses_to_anthropic(&body, &config).unwrap_err();
    assert!(err.to_string().contains("unsupported reasoning effort"));
}

#[test]
fn adaptive_thinking_rejects_unknown_effort() {
    let mut config = config();
    config.thinking_mode = ThinkingMode::Adaptive;
    let body = json!({
        "model": "Claude Sonnet 5",
        "stream": true,
        "reasoning": {"effort": "turbo"},
        "input": "hi"
    });
    let err = responses_to_anthropic(&body, &config).unwrap_err();
    assert!(err.to_string().contains("unsupported reasoning effort"));
}

#[test]
fn auto_thinking_leaves_non_claude_models_untouched() {
    let mut config = config();
    config.thinking_mode = ThinkingMode::Auto;
    let body = json!({
        "model": "DeepSeek-V4-Flash",
        "stream": true,
        "reasoning": {"effort": "xhigh"},
        "input": "hi"
    });
    let converted = responses_to_anthropic(&body, &config).unwrap();
    assert!(converted.request.thinking.is_none());
    assert!(converted.request.output_config.is_none());
}

#[test]
fn manual_thinking_caps_budget_below_max_tokens() {
    let mut config = config();
    config.thinking_mode = ThinkingMode::Manual;
    let body = json!({
        "model": "Claude Haiku 4.5",
        "stream": true,
        "max_output_tokens": 2048,
        "reasoning": {"effort": "high"},
        "input": "hi"
    });
    let converted = responses_to_anthropic(&body, &config).unwrap();
    let thinking = converted.request.thinking.unwrap();
    assert_eq!(thinking["type"], "enabled");
    assert_eq!(thinking["budget_tokens"], 2047);
    assert!(converted.request.output_config.is_none());
}

#[test]
fn web_search_becomes_anthropic_server_tool_when_enabled() {
    let mut config = config();
    config.enable_web_search_tool = true;
    config.web_search_max_uses = Some(1);
    let body = json!({
        "model": "Claude Sonnet 5",
        "stream": true,
        "input": "hi",
        "tools": [{
            "type": "web_search",
            "external_web_access": true,
            "filters": {"allowed_domains": ["openai.com", "docs.rs"]},
            "user_location": {
                "type": "approximate",
                "city": "Taipei",
                "country": "TW",
                "timezone": "Asia/Taipei"
            }
        }]
    });
    let converted = responses_to_anthropic(&body, &config).unwrap();
    assert_eq!(converted.request.tools[0]["type"], "web_search_20250305");
    assert_eq!(converted.request.tools[0]["name"], "web_search");
    assert_eq!(converted.request.tools[0]["max_uses"], 1);
    assert_eq!(
        converted.request.tools[0]["allowed_domains"],
        json!(["openai.com", "docs.rs"])
    );
    assert_eq!(
        converted.request.tools[0]["user_location"]["timezone"],
        "Asia/Taipei"
    );
    assert!(converted.request.tools[0].get("input_schema").is_none());
}

#[test]
fn rejects_codex_web_search_modes_without_anthropic_equivalents() {
    let mut config = config();
    config.enable_web_search_tool = true;
    for unsupported in [
        json!({"external_web_access": false}),
        json!({"external_web_access": true, "indexed_web_access": true}),
        json!({"external_web_access": true, "search_context_size": "high"}),
        json!({"external_web_access": true, "search_content_types": ["text", "image"]}),
    ] {
        let mut tool = unsupported;
        tool["type"] = json!("web_search");
        let error = responses_to_anthropic(
            &json!({
                "model": "Claude Sonnet 5",
                "stream": true,
                "input": "hi",
                "tools": [tool]
            }),
            &config,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("cannot preserve")
                || error.to_string().contains("no equivalent")
        );
    }
}

#[test]
fn rejects_unknown_tool_type() {
    let body = json!({
        "model": "m",
        "stream": true,
        "input": "hi",
        "tools": [{"type":"computer_use_preview"}]
    });
    let error = responses_to_anthropic(&body, &config()).unwrap_err();
    assert!(error.to_string().contains("unsupported tool type"));
}

#[test]
fn omits_legacy_openai_hosted_image_generation_tool() {
    let body = json!({
        "model": "m",
        "stream": true,
        "parallel_tool_calls": true,
        "tool_choice": "auto",
        "input": "hi",
        "tools": [{"type":"image_generation","output_format":"png"}]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert!(converted.request.tools.is_empty());
    assert!(converted.request.tool_choice.is_none());
}

#[test]
fn rejects_tool_names_that_collide_after_flattening() {
    let body = json!({
        "model": "m",
        "stream": true,
        "input": "hi",
        "tools": [
            {"type":"function","name":"mcp__read","parameters":{"type":"object"}},
            {"type":"namespace","name":"mcp","tools":[{"type":"function","name":"read","parameters":{"type":"object"}}]}
        ]
    });
    let error = responses_to_anthropic(&body, &config()).unwrap_err();
    assert!(error.to_string().contains("tool names collide"));
}

#[test]
fn disables_parallel_anthropic_tool_use_when_codex_requests_serial_calls() {
    let body = json!({
        "model": "m",
        "stream": true,
        "parallel_tool_calls": false,
        "tool_choice": "auto",
        "input": "hi",
        "tools": [{"type":"function","name":"exec_command","parameters":{"type":"object"}}]
    });
    let converted = responses_to_anthropic(&body, &config()).unwrap();
    assert_eq!(
        converted.request.tool_choice.as_ref().unwrap()["type"],
        "auto"
    );
    assert_eq!(
        converted.request.tool_choice.as_ref().unwrap()["disable_parallel_tool_use"],
        true
    );
}

#[test]
fn codex_function_style_web_search_becomes_server_tool() {
    let mut config = config();
    config.enable_web_search_tool = true;
    config.web_search_max_uses = Some(1);
    let body = json!({
        "model": "Claude Sonnet 5",
        "stream": true,
        "input": "hi",
        "tools": [{
            "type": "function",
            "name": "web_search",
            "description": "Search the web",
            "parameters": {"type": "object", "properties": {"query": {"type": "string"}}}
        }]
    });
    let converted = responses_to_anthropic(&body, &config).unwrap();
    assert_eq!(converted.request.tools[0]["type"], "web_search_20250305");
    assert_eq!(converted.request.tools[0]["name"], "web_search");
    assert!(converted.request.tools[0].get("input_schema").is_none());
    assert_eq!(
        converted.tool_names.to_codex_name("web_search"),
        "web_search"
    );
}
