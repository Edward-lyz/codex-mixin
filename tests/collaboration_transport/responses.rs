use super::*;
use futures_util::stream;
use serde_json::json;

#[tokio::test]
async fn collaboration_responses_keep_plaintext_markers_consistent_across_chunks() {
    let call = json!({"type":"function_call", "id":"fc_1", "call_id":"call_1",
        "namespace":"codex_mixin_collaboration", "name":"send_message", "arguments":"{\"message\":\"done\"}"});
    let payloads = [
        json!({"type":"response.output_item.added", "output_index":0, "item":call}),
        json!({"type":"response.function_call_arguments.delta", "delta":"unchanged"}),
        json!({"type":"response.output_item.done", "output_index":0, "item":call}),
        json!({"type":"response.completed", "response":{"model":"gpt-upstream", "output":[call]}}),
    ];
    let wire = payloads
        .iter()
        .flat_map(|payload| encode_event(payload["type"].as_str().unwrap(), payload).unwrap())
        .collect::<Vec<_>>();
    // Split inside every JSON token and SSE delimiter, as a transport can.
    let chunks = wire
        .into_iter()
        .map(|byte| Ok::<_, reqwest::Error>(Bytes::from(vec![byte])))
        .collect::<Vec<_>>();
    let mapped = map_openai_responses_sse(
        stream::iter(chunks),
        "gpt-upstream".to_owned(),
        "gpt-custom".to_owned(),
        true,
    )
    .collect::<Vec<_>>()
    .await;
    let mut decoder = SseDecoder::default();
    let events = mapped
        .into_iter()
        .flat_map(|chunk| decoder.push(&chunk.unwrap()))
        .map(|event| serde_json::from_str::<Value>(&event.data).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), payloads.len());
    for index in [0, 2] {
        assert_eq!(events[index]["item"]["encrypted_function_args"], json!([]));
        assert_eq!(events[index]["item"]["arguments"], call["arguments"]);
        assert_eq!(events[index]["item"]["call_id"], call["call_id"]);
    }
    assert_eq!(events[1], payloads[1]);
    assert_eq!(events[3]["response"]["output"][0], events[2]["item"]);
    assert_eq!(events[3]["response"]["model"], "gpt-custom");
}

#[tokio::test]
async fn collaboration_responses_handle_data_only_events_and_escaped_namespaces() {
    let fixture = concat!(
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"namespace\":\"\\u0063odex_mixin_collaboration\",\"name\":\"followup_task\",\"arguments\":\"{}\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"namespace\":\"collaboration\",\"name\":\"send_message\",\"arguments\":\"{}\",\"encrypted_function_args\":[\"message\"]}}\n\n"
    );
    let upstream = stream::iter([Ok::<_, reqwest::Error>(Bytes::from_static(
        fixture.as_bytes(),
    ))]);
    let chunks =
        map_openai_responses_sse(upstream, "gpt".to_owned(), "gpt-custom".to_owned(), true)
            .collect::<Vec<_>>()
            .await;
    let mut decoder = SseDecoder::default();
    let events = chunks
        .into_iter()
        .flat_map(|chunk| decoder.push(&chunk.unwrap()))
        .map(|event| serde_json::from_str::<Value>(&event.data).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["item"]["encrypted_function_args"], json!([]));
    assert_eq!(
        events[1]["item"]["encrypted_function_args"],
        json!(["message"])
    );
}

#[tokio::test]
async fn native_responses_stay_opaque_even_while_other_calls_are_adapted() {
    let calls = [
        json!({"type":"function_call", "namespace":"collaboration", "name":"spawn_agent",
            "arguments":"{\"message\":\"opaque-without-metadata\"}"}),
        json!({"type":"function_call", "namespace":"collaboration", "name":"send_message",
            "arguments":"{\"message\":\"opaque-null-metadata\"}", "encrypted_function_args":null}),
    ];
    for active in [true, false] {
        let payload =
            json!({"type":"response.completed", "response":{"model":"native", "output":calls}});
        let wire = encode_event("response.completed", &payload).unwrap();
        let mapped = map_openai_responses_sse(
            stream::iter([Ok::<_, reqwest::Error>(wire)]),
            "native".to_owned(),
            "client".to_owned(),
            active,
        );
        let collected = collect_response_stream(mapped).await.unwrap();
        assert_eq!(collected.response["output"], json!(calls));
        assert_eq!(collected.response["model"], "client");
    }
}

#[tokio::test]
async fn encrypted_alias_response_terminates_with_a_failure_event() {
    let item = json!({"type":"function_call", "namespace":"codex_mixin_collaboration",
        "name":"spawn_agent", "arguments":"{}", "encrypted_function_args":["message"]});
    for payload in [
        json!({"type":"response.output_item.added", "item":item}),
        json!({"type":"response.output_item.done", "item":item}),
        json!({"type":"response.completed", "response":{"output":[item]}}),
    ] {
        let kind = payload["type"].as_str().unwrap();
        let wire = encode_event(kind, &payload).unwrap();
        let chunks = map_openai_responses_sse(
            stream::iter([Ok::<_, reqwest::Error>(wire)]),
            "native".to_owned(),
            "client".to_owned(),
            true,
        )
        .collect::<Vec<_>>()
        .await;
        let mut decoder = SseDecoder::default();
        let events = chunks
            .into_iter()
            .flat_map(|c| decoder.push(&c.unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("response.failed"));
        assert!(
            events[0]
                .data
                .contains("encrypted arguments returned for a plaintext")
        );
    }
}
