use super::*;

#[test]
fn responses_to_gemini_basic_user_turn() {
    let body = json!({
        "model": "gemini-3.8-flash-low",
        "instructions": "Be brief.",
        "input": "hello world",
        "max_output_tokens": 100
    });
    let converted = responses_to_gemini(&body, None).unwrap();
    let req = &converted.request;
    assert_eq!(req["systemInstruction"]["parts"][0]["text"], "Be brief.");
    assert_eq!(req["contents"][0]["role"], "user");
    assert_eq!(req["contents"][0]["parts"][0]["text"], "hello world");
    assert_eq!(req["generationConfig"]["maxOutputTokens"], 100);
    assert_eq!(
        req["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        2048
    );
}

#[test]
fn responses_to_gemini_multi_turn_with_function_call() {
    let body = json!({
        "model": "gemini-2.5-pro",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": "what is the weather?"
            },
            {
                "type": "function_call",
                "call_id": "call_123",
                "name": "get_weather",
                "arguments": "{\"location\":\"Paris\"}"
            },
            {
                "type": "function_call_output",
                "call_id": "get_weather",
                "output": "{\"temp\": 20}"
            }
        ]
    });
    let converted = responses_to_gemini(&body, None).unwrap();
    let req = &converted.request;
    assert_eq!(req["contents"].as_array().unwrap().len(), 3);
    assert_eq!(req["contents"][0]["role"], "user");
    assert_eq!(
        req["contents"][0]["parts"][0]["text"],
        "what is the weather?"
    );
    assert_eq!(req["contents"][1]["role"], "model");
    assert_eq!(
        req["contents"][1]["parts"][0]["functionCall"]["name"],
        "get_weather"
    );
    assert_eq!(
        req["contents"][1]["parts"][0]["functionCall"]["args"]["location"],
        "Paris"
    );
    assert_eq!(req["contents"][2]["role"], "user");
    assert_eq!(
        req["contents"][2]["parts"][0]["functionResponse"]["name"],
        "get_weather"
    );
}

#[test]
fn responses_to_gemini_thinking_budget_mapping() {
    let high = responses_to_gemini(
        &json!({
            "model": "gemini-3.8-flash-high",
            "input": "think hard"
        }),
        None,
    )
    .unwrap();
    assert_eq!(
        high.request["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        16384
    );

    let medium = responses_to_gemini(
        &json!({
            "model": "gemini-3.8-flash-medium",
            "input": "normal thought"
        }),
        None,
    )
    .unwrap();
    assert_eq!(
        medium.request["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        8192
    );

    let low = responses_to_gemini(
        &json!({
            "model": "gemini-3.8-flash-low",
            "input": "quick thought"
        }),
        None,
    )
    .unwrap();
    assert_eq!(
        low.request["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        2048
    );
}

#[test]
fn sanitize_gemini_schema_strips_boolean_enum_and_unknown_fields() {
    let mut schema = json!({
        "type": "boolean",
        "enum": [true],
        "title": "Some Flag",
        "encrypted": true,
        "properties": {
            "sub": {
                "type": ["string", "null"],
                "const": "test",
                "enum": ["a", "b"]
            }
        }
    });
    sanitize_gemini_schema(&mut schema);
    assert_eq!(schema.get("enum"), None);
    assert_eq!(schema.get("title"), None);
    assert_eq!(schema.get("encrypted"), None);
    assert_eq!(schema["type"], "boolean");
    let sub = &schema["properties"]["sub"];
    assert_eq!(sub["type"], "string");
    assert_eq!(sub["nullable"], true);
    assert_eq!(sub["enum"], json!(["a", "b"]));
}

#[test]
fn test_thought_signature_roundtrip_and_extraction() {
    let raw_sig = "Evq83vArLw==";
    let encoded = encode_signature_in_call_id("call_exec", raw_sig, "gemini-3.8-flash-high");
    assert!(!encoded.contains('+'));
    assert!(!encoded.contains('/'));
    assert!(!encoded.contains('='));
    assert!(encoded.starts_with("call_exec_ts_gemini_"));

    let item = json!({"call_id": encoded});
    let decoded = extract_thought_signature(&item, true);
    assert_eq!(decoded, raw_sig);

    // Cross-model switch: Claude-tagged signature or toolu_ id evaluated for Gemini falls back to sentinel
    let claude_call = encode_signature_in_call_id("call_exec", raw_sig, "claude-opus-4-6-thinking");
    assert!(claude_call.starts_with("call_exec_ts_claude_"));
    assert_eq!(
        extract_thought_signature(&json!({"call_id": claude_call}), true),
        SKIP_THOUGHT_SIGNATURE_VALIDATOR
    );
    let toolu_call = json!({"call_id": "toolu_011Cg3APu7LYEY856bxqYpWG"});
    assert_eq!(
        extract_thought_signature(&toolu_call, true),
        SKIP_THOUGHT_SIGNATURE_VALIDATOR
    );

    // Explicit field wins over call_id
    let item_explicit = json!({
        "call_id": "call_exec_ts_ignored",
        "thoughtSignature": "explicit_sig_1"
    });
    assert_eq!(
        extract_thought_signature(&item_explicit, true),
        "explicit_sig_1"
    );

    // Snake_case thought_signature is also accepted
    let item_snake = json!({
        "thought_signature": "explicit_sig_2"
    });
    assert_eq!(
        extract_thought_signature(&item_snake, true),
        "explicit_sig_2"
    );

    // Plain call_id without signature falls back to sentinel
    let item_plain = json!({"call_id": "call_plain"});
    assert_eq!(
        extract_thought_signature(&item_plain, true),
        SKIP_THOUGHT_SIGNATURE_VALIDATOR
    );

    // Sentinel roundtrips cleanly
    let encoded_sentinel =
        encode_signature_in_call_id("call_s", SKIP_THOUGHT_SIGNATURE_VALIDATOR, "gemini");
    assert_eq!(
        extract_thought_signature(&json!({"call_id": encoded_sentinel}), true),
        SKIP_THOUGHT_SIGNATURE_VALIDATOR
    );
}

#[test]
fn test_sanitize_thought_signatures_in_gemini_payload() {
    let mut payload = json!({
        "project": "aicode-consumers",
        "request": {
            "contents": [
                {
                    "role": "model",
                    "parts": [
                        {
                            "functionCall": {
                                "id": "call_1",
                                "name": "exec_command",
                                "args": {}
                            },
                            "thoughtSignature": "corrupted_sig_123",
                            "thought_signature": "corrupted_sig_123"
                        },
                        {
                            "text": "thinking...",
                            "thoughtSignature": "corrupted_text_sig"
                        }
                    ]
                }
            ]
        }
    });

    sanitize_thought_signatures_in_gemini_payload(&mut payload);
    let req = &payload["request"]["contents"][0]["parts"];
    assert_eq!(req[0]["thoughtSignature"], SKIP_THOUGHT_SIGNATURE_VALIDATOR);
    assert_eq!(req[0].get("thought_signature"), None);
    assert_eq!(req[1].get("thoughtSignature"), None);
}

#[test]
fn responses_to_gemini_preserves_encoded_thought_signature() {
    let raw_sig = "AQJyZWFsLXNpZ25hdHVyZS1ieXRlcwM=";
    let call_id = encode_signature_in_call_id(
        "call_default_api__exec_command",
        raw_sig,
        "gemini-3.8-flash-high",
    );

    let body = json!({
        "model": "gemini-3.8-flash-high",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": "run pwd"
            },
            {
                "type": "function_call",
                "call_id": call_id,
                "name": "exec_command",
                "namespace": "default_api",
                "arguments": r#"{"cmd":"pwd"}"#
            },
            {
                "type": "function_call_output",
                "call_id": call_id,
                "output": "/home/clf"
            }
        ]
    });
    let converted = responses_to_gemini(&body, None).unwrap();
    let req = &converted.request;
    assert_eq!(req["contents"].as_array().unwrap().len(), 3);

    // Model turn
    let model_part = &req["contents"][1]["parts"][0];
    assert_eq!(
        model_part["functionCall"]["name"],
        "default_api__exec_command"
    );
    assert_eq!(model_part["functionCall"]["args"]["cmd"], "pwd");
    assert_eq!(model_part["thoughtSignature"], raw_sig);

    // User response turn
    let resp_part = &req["contents"][2]["parts"][0];
    assert_eq!(
        resp_part["functionResponse"]["name"],
        "default_api__exec_command"
    );
    assert_eq!(
        resp_part["functionResponse"]["response"]["output"],
        "/home/clf"
    );
}

#[test]
fn responses_to_gemini_injects_sentinel_when_signature_missing() {
    let body = json!({
        "model": "gemini-3.8-flash-high",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": "run pwd"
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "exec_command",
                "namespace": "default_api",
                "arguments": r#"{"cmd":"pwd"}"#
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "/home/clf"
            }
        ]
    });
    let converted = responses_to_gemini(&body, None).unwrap();
    let req = &converted.request;

    let model_part = &req["contents"][1]["parts"][0];
    assert_eq!(
        model_part["functionCall"]["name"],
        "default_api__exec_command"
    );
    assert_eq!(
        model_part["thoughtSignature"],
        SKIP_THOUGHT_SIGNATURE_VALIDATOR
    );

    let resp_part = &req["contents"][2]["parts"][0];
    assert_eq!(
        resp_part["functionResponse"]["name"],
        "default_api__exec_command"
    );
}

#[test]
fn responses_to_gemini_converts_input_image() {
    let body = json!({
        "model": "gemini-2.5-pro",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "text", "text": "look at this"},
                    {"type": "input_image", "image_url": "data:image/png;base64,iVBORw0KGgo="}
                ]
            }
        ]
    });
    let converted = responses_to_gemini(&body, None).unwrap();
    let parts = &converted.request["contents"][0]["parts"];
    assert_eq!(parts.as_array().unwrap().len(), 2);
    assert_eq!(parts[0]["text"], "look at this");
    assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
    assert_eq!(parts[1]["inlineData"]["data"], "iVBORw0KGgo=");
}

#[test]
fn responses_to_gemini_rejects_missing_image_url() {
    let body = json!({
        "model": "gemini-2.5-pro",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_image"}
                ]
            }
        ]
    });
    let err = responses_to_gemini(&body, None).unwrap_err();
    assert!(err.to_string().contains("input_image missing image_url"));
}

#[test]
fn responses_to_gemini_rejects_non_base64_image_url() {
    let body = json!({
        "model": "gemini-2.5-pro",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_image", "image_url": "https://example.com/pic.png"}
                ]
            }
        ]
    });
    let err = responses_to_gemini(&body, None).unwrap_err();
    assert!(
        err.to_string()
            .contains("input_image data URL must be base64")
    );
}

#[test]
fn responses_to_gemini_claude_model_includes_tool_id_without_gemini_sentinel() {
    let body = json!({
        "model": "claude-sonnet-4-6",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": "run ls"
            },
            {
                "type": "function_call",
                "call_id": "toolu_011Cg2Dqo4kro5kCFXWzqMvG",
                "name": "exec_command",
                "namespace": "default_api",
                "arguments": r#"{"cmd":"ls"}"#
            },
            {
                "type": "function_call_output",
                "call_id": "toolu_011Cg2Dqo4kro5kCFXWzqMvG",
                "output": "file.txt"
            }
        ]
    });
    let converted = responses_to_gemini(&body, None).unwrap();
    let req = &converted.request;

    // Model turn must include id for Vertex Anthropic bridge
    let model_part = &req["contents"][1]["parts"][0];
    assert_eq!(
        model_part["functionCall"]["id"],
        "toolu_011Cg2Dqo4kro5kCFXWzqMvG"
    );
    assert_eq!(
        model_part["functionCall"]["name"],
        "default_api__exec_command"
    );
    // Non-Gemini model must NOT have the skip_thought_signature_validator sentinel injected
    assert_eq!(model_part.get("thoughtSignature"), None);

    // User response turn must include matching id
    let resp_part = &req["contents"][2]["parts"][0];
    assert_eq!(
        resp_part["functionResponse"]["id"],
        "toolu_011Cg2Dqo4kro5kCFXWzqMvG"
    );
    assert_eq!(
        resp_part["functionResponse"]["name"],
        "default_api__exec_command"
    );
}

#[test]
fn responses_to_gemini_claude_model_omits_unsupported_web_search_tool() {
    let body = json!({
        "model": "claude-opus-4-6-thinking",
        "input": "hello",
        "tools": [
            {"type": "web_search"},
            {
                "type": "function",
                "name": "lookup",
                "parameters": {"type": "object"}
            }
        ]
    });
    let converted = responses_to_gemini(&body, None).unwrap();
    let req = &converted.request;
    let tools = req["tools"].as_array().expect("tools array expected");
    assert!(!tools.iter().any(|tool| tool.get("googleSearch").is_some()));
    assert!(
        tools
            .iter()
            .any(|tool| tool.get("functionDeclarations").is_some())
    );

    let search_only = json!({
        "model": "claude-opus-4-6-thinking",
        "input": "hello",
        "tools": [{"type": "web_search"}]
    });
    let converted_search_only = responses_to_gemini(&search_only, None).unwrap();
    assert!(converted_search_only.request.get("tools").is_none());
}

#[test]
fn responses_to_gemini_claude_model_thinking_budget_less_than_max_tokens() {
    let body = json!({
        "model": "claude-opus-4-6-thinking",
        "input": "think and answer",
        "max_output_tokens": 8192,
        "reasoning": {
            "effort": "medium"
        }
    });
    let converted = responses_to_gemini(&body, None).unwrap();
    let max_output = converted.request["generationConfig"]["maxOutputTokens"]
        .as_u64()
        .unwrap();
    let budget = converted.request["generationConfig"]["thinkingConfig"]["thinkingBudget"]
        .as_u64()
        .unwrap();
    assert!(
        max_output > budget,
        "maxOutputTokens ({max_output}) must be strictly greater than thinkingBudget ({budget})"
    );
    assert_eq!(budget, 4096);
    assert_eq!(max_output, 8192);

    // High effort for Claude with max_tokens == budget
    let body_high = json!({
        "model": "claude-opus-4-6-thinking",
        "input": "think hard",
        "max_output_tokens": 8192,
        "reasoning": {
            "effort": "high"
        }
    });
    let converted_high = responses_to_gemini(&body_high, None).unwrap();
    let max_output_high = converted_high.request["generationConfig"]["maxOutputTokens"]
        .as_u64()
        .unwrap();
    let budget_high =
        converted_high.request["generationConfig"]["thinkingConfig"]["thinkingBudget"]
            .as_u64()
            .unwrap();
    assert!(
        max_output_high > budget_high,
        "maxOutputTokens ({max_output_high}) must be strictly greater than thinkingBudget ({budget_high})"
    );
    assert_eq!(budget_high, 8192);
    assert_eq!(max_output_high, 12288);
}
