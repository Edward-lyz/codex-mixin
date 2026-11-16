use super::*;

fn tools() -> Value {
    json!([{
        "type":"namespace", "name":"collaboration", "tools":[
            {"type":"function", "name":"send_message", "parameters":{
                "type":"object", "properties":{
                    "message":{"type":"string", "encrypted":true},
                    "target":{"type":"string"}
                }, "required":["message","target"]
            }},
            {"type":"function", "name":"wait_agent", "parameters":{
                "properties":{"message":{"encrypted":true}}
            }}
        ]
    }])
}

#[test]
fn collaboration_requests_preserve_history_and_restore_native_schemas() {
    let mut body = json!({
        "tools":tools(),
        "input":[
            {"type":"additional_tools", "tools":tools()},
            {"type":"tool_search_output", "execution":"client", "tools":tools()},
            {"type":"agent_message", "content":[
                {"type":"encrypted_content", "encrypted_content":"opaque-native-message"}
            ]},
            {"type":"reasoning", "encrypted_content":"opaque-native-reasoning"},
            {"type":"function_call_output", "output":[
                {"type":"encrypted_content", "encrypted_content":"opaque-tool-result"}
            ]}
        ]
    });
    let original = body.clone();
    let saved = prepare_request(&mut body).unwrap();
    for tools in [
        &body["tools"],
        &body["input"][0]["tools"],
        &body["input"][1]["tools"],
    ] {
        assert_eq!(tools[0]["name"], PLAINTEXT_NAMESPACE);
        assert!(
            tools[0]["tools"][0]["parameters"]["properties"]["message"]
                .get("encrypted")
                .is_none()
        );
        assert_eq!(
            tools[0]["tools"][0]["parameters"]["required"],
            json!(["message", "target"])
        );
        assert_eq!(
            tools[0]["tools"][1]["parameters"]["properties"]["message"]["encrypted"],
            true
        );
    }
    assert_eq!(
        body["input"].as_array().unwrap()[2..],
        original["input"].as_array().unwrap()[2..]
    );
    restore_request(&mut body, saved);
    assert_eq!(body, original);
}

#[test]
fn collaboration_schema_changes_only_known_message_parameters() {
    let schema = json!({"properties":{
        "message":{"type":"string", "encrypted":true},
        "other":{"type":"string", "encrypted":true}
    }});
    for name in ["spawn_agent", "send_message", "followup_task"] {
        let mut actual = schema.clone();
        prepare_schema(&mut actual, Some("collaboration"), name);
        assert_eq!(actual["properties"]["message"], json!({"type":"string"}));
        assert_eq!(actual["properties"]["other"], schema["properties"]["other"]);
    }
    for (namespace, name) in [
        (None, "send_message"),
        (Some("mail"), "send_message"),
        (Some("collaboration"), "wait_agent"),
    ] {
        let mut actual = schema.clone();
        prepare_schema(&mut actual, namespace, name);
        assert_eq!(actual, schema);
    }
}

#[test]
fn collaboration_plaintext_calls_have_an_explicit_transport_marker() {
    for name in ["spawn_agent", "send_message", "followup_task"] {
        for annotation in [None, Some(Value::Null)] {
            let mut call = json!({"type":"function_call", "namespace":"collaboration", "name":name,
                "arguments":"{\"message\":\"inspect the build\"}", "call_id":"call_1"});
            if let Some(annotation) = annotation {
                call["encrypted_function_args"] = annotation;
            }
            let original_arguments = call["arguments"].clone();
            mark_plaintext_call(&mut call);
            assert_eq!(call["encrypted_function_args"], json!([]));
            assert_eq!(call["arguments"], original_arguments);
            let once = call.clone();
            mark_plaintext_call(&mut call);
            assert_eq!(call, once);
        }
    }
}

#[test]
fn collaboration_native_ciphertext_and_other_tools_are_not_reinterpreted() {
    for mut call in [
        json!({"type":"function_call", "namespace":"collaboration", "name":"send_message",
            "arguments":"{\"message\":\"opaque\"}", "encrypted_function_args":["message"]}),
        json!({"type":"function_call", "namespace":"mail", "name":"send_message"}),
        json!({"type":"function_call", "name":"send_message"}),
        json!({"type":"custom_tool_call", "namespace":"collaboration", "name":"send_message"}),
        json!({"type":"function_call", "namespace":"collaboration", "name":"wait_agent"}),
    ] {
        let original = call.clone();
        mark_plaintext_call(&mut call);
        assert_eq!(call, original);
    }
}

#[test]
fn collaboration_converted_schemas_and_plaintext_history_stay_readable() {
    use crate::config::ThinkingMode;
    use crate::protocol::convert::responses_to_anthropic;
    use crate::protocol::openai_chat::responses_to_openai_chat;

    let body = json!({"model":"dsf41", "stream":true, "tools":tools(), "input":[
        {"type":"agent_message", "author":"/root", "recipient":"/root/worker",
            "content":[{"type":"input_text", "text":"Inspect the build"}]}
    ]});
    let secret = || panic!("plaintext collaboration must not access compaction storage");
    let anthropic = responses_to_anthropic(
        &body,
        None,
        None,
        ThinkingMode::Off,
        1024,
        None,
        false,
        None,
        &secret,
    )
    .unwrap();
    let anthropic = serde_json::to_value(anthropic.request).unwrap();
    assert!(
        anthropic["tools"][0]["input_schema"]["properties"]["message"]
            .get("encrypted")
            .is_none()
    );
    assert_eq!(anthropic["messages"][0]["role"], "user");
    assert!(
        anthropic["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Inspect the build")
    );
    let chat = responses_to_openai_chat(&body, None, &secret)
        .unwrap()
        .request;
    assert!(
        chat["tools"][0]["function"]["parameters"]["properties"]["message"]
            .get("encrypted")
            .is_none()
    );
    assert!(
        chat["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("Inspect the build")
    );
    assert_eq!(
        body["tools"][0]["tools"][0]["parameters"]["properties"]["message"]["encrypted"],
        true
    );
}

#[tokio::test]
async fn collaboration_chat_calls_use_plaintext_in_added_done_and_completed_items() {
    use crate::protocol::convert::ToolNameMap;
    use crate::protocol::openai_events::map_openai_chat_sse;
    use crate::protocol::sse::SseDecoder;
    use bytes::Bytes;
    use futures_util::{StreamExt, stream};

    let mut names = ToolNameMap::default();
    names
        .insert_namespaced(
            "collaboration__send_message".to_owned(),
            "collaboration".to_owned(),
            "send_message".to_owned(),
        )
        .unwrap();
    let delta = json!({"choices":[{"index":0,"delta":{"tool_calls":[{
        "index":0,"id":"call_1","type":"function","function":{
            "name":"collaboration__send_message","arguments":"{\"message\":\"done\"}"
        }
    }]},"finish_reason":null}]});
    let fixture = format!("data: {delta}\n\ndata: [DONE]\n\n");
    let upstream = stream::iter([Ok::<_, &str>(Bytes::from(fixture))]);
    let chunks = map_openai_chat_sse(upstream, json!({"model":"dsf41"}), names, None)
        .collect::<Vec<_>>()
        .await;
    let mut decoder = SseDecoder::default();
    let events = chunks
        .into_iter()
        .flat_map(|chunk| decoder.push(&chunk.unwrap()))
        .map(|event| serde_json::from_str::<Value>(&event.data).unwrap())
        .collect::<Vec<_>>();
    for kind in ["response.output_item.added", "response.output_item.done"] {
        let event = events.iter().find(|event| event["type"] == kind).unwrap();
        assert_eq!(event["item"]["encrypted_function_args"], json!([]));
    }
    let completed = events
        .iter()
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    assert_eq!(
        completed["response"]["output"][0]["encrypted_function_args"],
        json!([])
    );
}

#[tokio::test]
async fn collaboration_anthropic_messages_are_marked_plaintext_without_a_gateway() {
    use crate::protocol::convert::ToolNameMap;
    use crate::protocol::openai_events::map_anthropic_sse;
    use crate::protocol::sse::SseDecoder;
    use bytes::Bytes;
    use futures_util::{StreamExt, stream};

    let mut names = ToolNameMap::default();
    names
        .insert_namespaced(
            "collaboration__send_message".to_owned(),
            "collaboration".to_owned(),
            "send_message".to_owned(),
        )
        .unwrap();
    let fixture = concat!(
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":1}}}\n\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"collaboration__send_message\",\"input\":{\"target\":\"/root\",\"message\":\"R1 done\"}}}\n\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );
    let upstream = stream::iter([Ok::<_, &str>(Bytes::from_static(fixture.as_bytes()))]);
    let chunks = map_anthropic_sse(upstream, json!({"model":"dsf41"}), names)
        .collect::<Vec<_>>()
        .await;
    let mut decoder = SseDecoder::default();
    let events = chunks
        .into_iter()
        .flat_map(|chunk| decoder.push(&chunk.unwrap()))
        .map(|event| serde_json::from_str::<Value>(&event.data).unwrap())
        .collect::<Vec<_>>();
    let done = events
        .iter()
        .find(|event| event["type"] == "response.output_item.done")
        .unwrap();
    assert_eq!(done["item"]["encrypted_function_args"], json!([]));
    assert_eq!(done["item"]["namespace"], "collaboration");
    let arguments: Value =
        serde_json::from_str(done["item"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments["message"], "R1 done");
    let completed = events
        .iter()
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    assert_eq!(completed["response"]["output"][0], done["item"]);
}

#[test]
fn native_ciphertext_without_metadata_is_never_reclassified() {
    for metadata in [
        None,
        Some(Value::Null),
        Some(json!(["message"])),
        Some(json!([])),
    ] {
        for name in ["spawn_agent", "send_message", "followup_task"] {
            let mut call = json!({"type":"function_call", "namespace":"collaboration",
                "name":name, "arguments":"{\"message\":\"opaque-native-payload\"}"});
            if let Some(metadata) = &metadata {
                call["encrypted_function_args"] = metadata.clone();
            }
            for mut event in [
                json!({"type":"response.output_item.added", "item":call}),
                json!({"type":"response.output_item.done", "item":call}),
                json!({"type":"response.completed", "response":{"output":[call]}}),
            ] {
                let original = event.clone();
                restore_event(&mut event).unwrap();
                assert_eq!(event, original);
            }
        }
    }
}

#[test]
fn ordinary_namespace_selects_plaintext_for_all_message_tools() {
    for name in ["spawn_agent", "send_message", "followup_task", "wait_agent"] {
        let call = json!({"type":"function_call", "namespace":PLAINTEXT_NAMESPACE,
            "name":name, "call_id":"call_kept", "id":"fc_kept",
            "arguments":"{\"message\":\"Keep codex_mixin_collaboration literal\"}"});
        for mut event in [json!({"item":call}), json!({"response":{"output":[call]}})] {
            restore_event(&mut event).unwrap();
            let restored = event.get("item").unwrap_or(&event["response"]["output"][0]);
            assert_eq!(restored["namespace"], "collaboration");
            assert_eq!(restored["arguments"], call["arguments"]);
            assert_eq!(restored["call_id"], call["call_id"]);
            assert_eq!(restored["id"], call["id"]);
            if name == "wait_agent" {
                assert!(restored.get("encrypted_function_args").is_none());
            } else {
                assert_eq!(restored["encrypted_function_args"], json!([]));
            }
        }
    }
}

#[test]
fn encrypted_alias_calls_fail_before_they_can_be_delivered() {
    for metadata in [json!(["message"]), json!({"invalid":"metadata"})] {
        let mut event = json!({"item":{"type":"function_call", "namespace":PLAINTEXT_NAMESPACE,
            "name":"send_message", "encrypted_function_args":metadata, "arguments":"{}"}});
        let original = event.clone();
        let error = restore_event(&mut event).unwrap_err();
        assert!(matches!(error, GatewayError::Upstream(_)));
        assert_eq!(event, original);
    }
}

#[test]
fn request_replay_only_renames_calls_with_plaintext_provenance() {
    let opaque = json!({"type":"function_call", "namespace":"collaboration", "name":"send_message",
        "arguments":"{\"message\":\"opaque\"}"});
    let mut plain = opaque.clone();
    plain["encrypted_function_args"] = json!([]);
    let mut body = json!({"tools":tools(), "input":[opaque, plain,
        {"type":"function_call", "namespace":"collaboration", "name":"wait_agent", "arguments":"{}"},
        {"type":"agent_message", "author":"/root", "recipient":"/root/worker", "content":[
            {"type":"input_text", "text":"header"},
            {"type":"encrypted_content", "encrypted_content":"opaque-native"}]}]});
    let original = body.clone();
    let saved = prepare_request(&mut body).unwrap();
    assert!(saved.active());
    assert_eq!(body["input"][0], original["input"][0]);
    assert_eq!(body["input"][1]["namespace"], PLAINTEXT_NAMESPACE);
    assert_eq!(
        body["input"][1]["arguments"],
        original["input"][1]["arguments"]
    );
    assert_eq!(body["input"][2]["namespace"], PLAINTEXT_NAMESPACE);
    assert_eq!(body["input"][3], original["input"][3]);
    restore_request(&mut body, saved);
    assert_eq!(body, original);
}

#[test]
fn alias_collisions_fail_before_any_request_mutation() {
    for collision in [
        json!({"type":"namespace", "name":PLAINTEXT_NAMESPACE, "tools":[]}),
        json!({"type":"function", "name":PLAINTEXT_NAMESPACE}),
    ] {
        let mut body = json!({"tools":tools()});
        body["tools"].as_array_mut().unwrap().push(collision);
        let original = body.clone();
        assert!(matches!(
            prepare_request(&mut body),
            Err(GatewayError::BadRequest(_))
        ));
        assert_eq!(body, original);
    }
    let mut body = json!({"tools":tools(), "input":[
        {"type":"tool_search_output", "tools":[{"type":"namespace", "name":PLAINTEXT_NAMESPACE}]}
    ]});
    let original = body.clone();
    assert!(prepare_request(&mut body).is_err());
    assert_eq!(body, original);
}

#[test]
fn plaintext_agent_messages_use_portable_inputs_and_restore_exactly() {
    let mut body = json!({"input":[{"type":"agent_message", "id":"am_1",
        "author":"/root/worker", "recipient":"/root", "content":[
            {"type":"input_text", "text":"Task completed"},
            {"type":"input_text", "text":"Keep this second part"}]}]});
    let original = body.clone();
    let saved = prepare_request(&mut body).unwrap();
    assert!(!saved.active());
    assert_eq!(body["input"][0]["type"], "message");
    assert_eq!(body["input"][0]["role"], "user");
    let text = body["input"][0]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("/root/worker"));
    assert!(text.contains("to /root"));
    assert!(text.ends_with("Task completed\nKeep this second part"));
    assert!(body["input"][0].get("author").is_none());
    restore_request(&mut body, saved);
    assert_eq!(body, original);
}

#[test]
fn opaque_agent_messages_are_rejected_by_plaintext_protocols() {
    use crate::config::ThinkingMode;
    use crate::protocol::convert::responses_to_anthropic;
    use crate::protocol::openai_chat::responses_to_openai_chat;
    let body = json!({"model":"dsf41", "input":[{"type":"agent_message",
        "author":"/root", "recipient":"/root/worker", "content":[
            {"type":"input_text", "text":"header"},
            {"type":"encrypted_content", "encrypted_content":"opaque-native"}]}]});
    let secret = || panic!("agent messages must not access compaction storage");
    let chat = responses_to_openai_chat(&body, None, &secret)
        .err()
        .unwrap();
    assert!(matches!(chat, GatewayError::BadRequest(_)));
    let anthropic = responses_to_anthropic(
        &body,
        None,
        None,
        ThinkingMode::Off,
        1024,
        None,
        false,
        None,
        &secret,
    )
    .err()
    .unwrap();
    assert!(matches!(anthropic, GatewayError::BadRequest(_)));
}

#[test]
fn malformed_plaintext_messages_fail_before_tools_or_history_are_changed() {
    for item in [
        json!({"type":"agent_message", "author":"/root", "recipient":"/root/worker"}),
        json!({"type":"agent_message", "author":"/root", "recipient":"/root/worker", "content":[]}),
        json!({"type":"agent_message", "recipient":"/root/worker", "content":[{"type":"input_text", "text":"task"}]}),
        json!({"type":"agent_message", "author":"/root", "content":[{"type":"input_text", "text":"task"}]}),
        json!({"type":"agent_message", "author":"/root", "recipient":"/root/worker", "content":[{"type":"input_text"}]}),
    ] {
        let mut body = json!({"tools":tools(), "input":[
            {"type":"agent_message", "author":"/root", "recipient":"/root/worker", "content":[{"type":"input_text", "text":"valid"}]}, item
        ]});
        let original = body.clone();
        assert!(matches!(
            prepare_request(&mut body),
            Err(GatewayError::BadRequest(_))
        ));
        assert_eq!(body, original);
    }
}
