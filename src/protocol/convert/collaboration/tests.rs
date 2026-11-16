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
    let saved = prepare_request(&mut body);
    for tools in [
        &body["tools"],
        &body["input"][0]["tools"],
        &body["input"][1]["tools"],
    ] {
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
