use super::state::MapperState;
use super::*;
use crate::sse::drain_events;

async fn collect_events<S>(events: S) -> String
where
    S: Stream<Item = Result<Bytes, Infallible>>,
{
    tokio::pin!(events);
    let mut output = Vec::new();
    while let Some(chunk) = events.next().await {
        output.extend_from_slice(&chunk.unwrap());
    }
    String::from_utf8(output).unwrap()
}

async fn map_openai_tool_call(tool_call: Value, tool_names: ToolNameMap) -> String {
    let chunk = json!({
        "choices": [{
            "delta": {"tool_calls": [tool_call]},
            "finish_reason": "tool_calls"
        }]
    });
    let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(format!(
        "data: {chunk}\n\ndata: [DONE]\n\n"
    )))]);
    collect_events(map_openai_chat_sse(upstream, json!({}), tool_names)).await
}

fn tool_call(id: Option<&str>, name: Option<&str>, arguments: &str) -> Value {
    let mut tool_call = json!({"index":0,"function":{"arguments":arguments}});
    if let Some(id) = id {
        tool_call["id"] = json!(id);
    }
    if let Some(name) = name {
        tool_call["function"]["name"] = json!(name);
    }
    tool_call
}

#[tokio::test]
async fn maps_valid_special_and_namespaced_tool_calls() {
    let mut custom_names = ToolNameMap::default();
    custom_names
        .insert_custom("apply_patch".to_owned(), "apply_patch".to_owned())
        .unwrap();
    let custom_body = map_openai_tool_call(
        tool_call(
            Some("call_custom"),
            Some("apply_patch"),
            r#"{"input":"*** Begin Patch"}"#,
        ),
        custom_names,
    )
    .await;
    assert!(custom_body.contains("\"type\":\"custom_tool_call\""));
    assert!(
        custom_body.contains("\"input\":\"*** Begin Patch\""),
        "{custom_body}"
    );

    let mut search_names = ToolNameMap::default();
    search_names
        .insert_tool_search(
            "tool_search".to_owned(),
            "tool_search".to_owned(),
            "client".to_owned(),
        )
        .unwrap();
    let search_body = map_openai_tool_call(
        tool_call(
            Some("call_search"),
            Some("tool_search"),
            r#"{"query":"calendar"}"#,
        ),
        search_names,
    )
    .await;
    assert!(search_body.contains("\"type\":\"tool_search_call\""));
    assert!(
        search_body.contains("\"arguments\":{\"query\":\"calendar\"}"),
        "{search_body}"
    );

    let mut namespaced_names = ToolNameMap::default();
    namespaced_names
        .insert_namespaced(
            "collaboration__spawn_agent".to_owned(),
            "collaboration".to_owned(),
            "spawn_agent".to_owned(),
        )
        .unwrap();
    let namespaced_body = map_openai_tool_call(
        tool_call(Some("call_spawn"), Some("collaboration__spawn_agent"), "{}"),
        namespaced_names,
    )
    .await;
    assert!(namespaced_body.contains("\"name\":\"spawn_agent\""));
    assert!(namespaced_body.contains("\"namespace\":\"collaboration\""));
    assert!(!namespaced_body.contains("\"name\":\"collaboration__spawn_agent\""));
}

#[tokio::test]
async fn rejects_malformed_openai_tool_calls() {
    let mut custom_names = ToolNameMap::default();
    custom_names
        .insert_custom("apply_patch".to_owned(), "apply_patch".to_owned())
        .unwrap();
    let mut search_names = ToolNameMap::default();
    search_names
        .insert_tool_search(
            "tool_search".to_owned(),
            "tool_search".to_owned(),
            "client".to_owned(),
        )
        .unwrap();
    let cases = [
        (
            "custom JSON",
            tool_call(Some("call_custom"), Some("apply_patch"), "{"),
            custom_names.clone(),
        ),
        (
            "custom input type",
            tool_call(Some("call_custom"), Some("apply_patch"), r#"{"input":7}"#),
            custom_names.clone(),
        ),
        (
            "custom input field",
            tool_call(
                Some("call_custom"),
                Some("apply_patch"),
                r#"{"other":"value"}"#,
            ),
            custom_names,
        ),
        (
            "tool_search JSON",
            tool_call(Some("call_search"), Some("tool_search"), "{"),
            search_names,
        ),
        (
            "id",
            tool_call(None, Some("get_weather"), "{}"),
            ToolNameMap::default(),
        ),
        (
            "name",
            tool_call(Some("call_weather"), None, "{}"),
            ToolNameMap::default(),
        ),
    ];

    for (malformed, tool_call, tool_names) in cases {
        let body = map_openai_tool_call(tool_call, tool_names).await;
        assert!(
            body.contains("event: response.failed"),
            "{malformed}: {body}"
        );
        assert!(
            !body.contains("event: response.completed"),
            "{malformed}: {body}"
        );
        assert!(!body.contains("event: response.output_item.done"));
        assert!(!body.contains("call_0"));
        assert!(!body.contains("\"name\":\"\""));
        let mut encoded = body.as_bytes().to_vec();
        let failed = drain_events(&mut encoded)
            .into_iter()
            .find(|event| event.event.as_deref() == Some("response.failed"))
            .unwrap();
        let failed: Value = serde_json::from_str(&failed.data).unwrap();
        assert!(
            failed
                .pointer("/response/error/message")
                .and_then(Value::as_str)
                .is_some(),
            "{malformed}: {body}"
        );
    }
}

#[tokio::test]
async fn rejects_anthropic_tool_calls_missing_id_or_name() {
    let cases = [
        (
            "id",
            json!({"type":"tool_use","name":"get_weather","input":{}}),
        ),
        (
            "name",
            json!({"type":"tool_use","id":"call_weather","input":{}}),
        ),
    ];

    for (missing, content_block) in cases {
        let start = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": content_block
        });
        let stop = json!({"type":"content_block_stop","index":0});
        let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(format!(
            "data: {start}\n\ndata: {stop}\n\ndata: {{\"type\":\"message_stop\"}}\n\n"
        )))]);
        let body = collect_events(map_anthropic_sse(
            upstream,
            json!({}),
            ToolNameMap::default(),
        ))
        .await;

        assert!(
            body.contains("event: response.failed"),
            "missing {missing}: {body}"
        );
        assert!(
            !body.contains("event: response.completed"),
            "missing {missing}: {body}"
        );
        assert!(
            !body.contains("\"call_id\":\"\""),
            "missing {missing}: {body}"
        );
        assert!(!body.contains("\"name\":\"\""), "missing {missing}: {body}");
    }
}

#[tokio::test]
async fn preserves_usage_from_anthropic_message_start() {
    let events = [
        json!({"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":3}}}),
        json!({"type":"message_stop"}),
    ];
    let stream = events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(stream))]);
    let body = collect_events(map_anthropic_sse(
        upstream,
        json!({}),
        ToolNameMap::default(),
    ))
    .await;
    let mut encoded = body.into_bytes();
    let completed = drain_events(&mut encoded)
        .into_iter()
        .find(|event| event.event.as_deref() == Some("response.completed"))
        .unwrap();
    let completed: Value = serde_json::from_str(&completed.data).unwrap();

    assert_eq!(completed["response"]["usage"]["input_tokens"], 7);
    assert_eq!(completed["response"]["usage"]["output_tokens"], 3);
    assert_eq!(completed["response"]["usage"]["total_tokens"], 10);
}

#[tokio::test]
async fn maps_anthropic_server_web_search_lifecycle() {
    let events = [
        json!({"type":"message_start","message":{"usage":{"input_tokens":10}}}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srvtoolu_123","name":"web_search","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"weather seattle\"}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_123","content":[{"type":"web_search_result","title":"Seattle Weather","url":"https://example.com"}]}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"message_stop"}),
    ];
    let stream = events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(stream))]);
    let body = collect_events(map_anthropic_sse(
        upstream,
        json!({"model":"Claude Sonnet 5"}),
        ToolNameMap::default(),
    ))
    .await;
    let mut encoded = body.as_bytes().to_vec();
    let events = drain_events(&mut encoded);
    let added: Value = serde_json::from_str(
        &events
            .iter()
            .find(|event| event.event.as_deref() == Some("response.output_item.added"))
            .unwrap()
            .data,
    )
    .unwrap();
    assert_eq!(added["item"]["type"], "web_search_call");
    assert_eq!(added["item"]["status"], "in_progress");
    let done: Value = serde_json::from_str(
        &events
            .iter()
            .find(|event| event.event.as_deref() == Some("response.output_item.done"))
            .unwrap()
            .data,
    )
    .unwrap();
    assert_eq!(done["item"]["id"], "srvtoolu_123");
    assert_eq!(done["item"]["status"], "completed");
    assert_eq!(done["item"]["action"]["type"], "search");
    assert_eq!(done["item"]["action"]["query"], "weather seattle");
    let completed: Value = serde_json::from_str(
        &events
            .iter()
            .find(|event| event.event.as_deref() == Some("response.completed"))
            .unwrap()
            .data,
    )
    .unwrap();
    assert_eq!(completed["response"]["output"][0], done["item"]);
}

#[tokio::test]
async fn infers_omitted_web_search_tool_use_id_when_unambiguous() {
    let events = [
        json!({"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srvtoolu_123","name":"web_search","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"Codex release\"}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"web_search_tool_result","content":[{"type":"web_search_result","title":"Codex","url":"https://example.com"}]}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"message_stop"}),
    ];
    let stream = events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(stream))]);
    let body = collect_events(map_anthropic_sse(
        upstream,
        json!({"model":"Claude Haiku 4.5"}),
        ToolNameMap::default(),
    ))
    .await;

    assert!(body.contains("event: response.completed"), "{body}");
    assert!(body.contains("\"id\":\"srvtoolu_123\""), "{body}");
}

#[test]
fn rejects_omitted_web_search_tool_use_id_when_ambiguous() {
    let mut state = MapperState::new(json!({}), ToolNameMap::default());
    state
        .start_web_search(
            1,
            Some("srvtoolu_1".to_owned()),
            Some("web_search".to_owned()),
            "{\"query\":\"one\"}".to_owned(),
        )
        .unwrap();
    state.finish_tool(1, None).unwrap();
    state
        .start_web_search(
            2,
            Some("srvtoolu_2".to_owned()),
            Some("web_search".to_owned()),
            "{\"query\":\"two\"}".to_owned(),
        )
        .unwrap();
    state.finish_tool(2, None).unwrap();

    let error = state
        .finish_web_search_result(3, &json!({"type":"web_search_tool_result","content":[]}))
        .unwrap_err();
    assert!(error.contains("2 pending searches"));
}

#[tokio::test]
async fn rejects_web_search_without_result_block() {
    let events = [
        json!({"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srvtoolu_123","name":"web_search","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"weather seattle\"}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_stop"}),
    ];
    let stream = events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    let upstream = futures_util::stream::iter([Ok::<_, reqwest::Error>(Bytes::from(stream))]);
    let body = collect_events(map_anthropic_sse(
        upstream,
        json!({}),
        ToolNameMap::default(),
    ))
    .await;
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(!body.contains("event: response.completed"), "{body}");
}
