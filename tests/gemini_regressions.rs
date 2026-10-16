use bytes::Bytes;
use codex_mixin::protocol::{
    convert::ToolNameMap, gemini::responses_to_gemini, openai_events::map_gemini_sse,
    sse::SseDecoder,
};
use futures_util::StreamExt;
use serde_json::{Value, json};

fn sse(value: Value) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}

async fn events(chunks: Vec<Bytes>) -> Vec<Value> {
    let upstream = futures_util::stream::iter(chunks.into_iter().map(Ok::<_, reqwest::Error>));
    let mapped = map_gemini_sse(
        upstream,
        json!({"model":"gemini-2.5-pro","input":"test"}),
        ToolNameMap::default(),
    );
    tokio::pin!(mapped);
    let mut decoder = SseDecoder::default();
    let mut output = Vec::new();
    while let Some(chunk) = mapped.next().await {
        for event in decoder.push(&chunk.unwrap()) {
            output.push(serde_json::from_str(&event.data).unwrap());
        }
    }
    output
}

#[test]
fn preserves_system_content() {
    for role in ["developer", "system"] {
        for content in [
            json!("Never reveal passwords"),
            json!([
                {"type":"input_text","text":"Never reveal passwords"},
                {"type":"text","text":"Be brief"}
            ]),
        ] {
            let converted = responses_to_gemini(
                &json!({
                    "model":"gemini-2.5-pro",
                    "instructions":"Session rules",
                    "input":[
                        {"role":role,"content":content},
                        {"role":"user","content":"hello"}
                    ]
                }),
                None,
            )
            .unwrap();
            let mut expected = vec![
                json!({"text":"Session rules"}),
                json!({"text":"Never reveal passwords"}),
            ];
            if content.is_array() {
                expected.push(json!({"text":"Be brief"}));
            }
            assert_eq!(
                converted.request["systemInstruction"]["parts"],
                json!(expected)
            );
            assert_eq!(
                converted.request["contents"][0]["parts"],
                json!([{"text":"hello"}])
            );
        }
    }
}

#[test]
fn rejects_invalid_system_content() {
    for content in [
        Value::Null,
        json!(false),
        json!([{"type":"input_image","image_url":"data:image/png;base64,AAAA"}]),
        json!([{"type":"input_text"}]),
    ] {
        let converted = responses_to_gemini(
            &json!({
                "model":"gemini-2.5-pro",
                "input":[{"role":"system","content":content},{"role":"user","content":"hello"}]
            }),
            None,
        );
        assert!(
            converted.is_err(),
            "silently accepted invalid system content: {content}"
        );
    }
}

#[tokio::test]
async fn rejects_unfinished_streams() {
    for chunks in [
        vec![],
        vec![sse(
            json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}),
        )],
        vec![sse(
            json!({"candidates":[{"finishReason":"FINISH_REASON_UNSPECIFIED"}]}),
        )],
        vec![sse(json!({"usageMetadata":{"promptTokenCount":1}}))],
        vec![Bytes::from_static(b"data: {\"candidates\":")],
    ] {
        let output = events(chunks).await;
        assert_eq!(output.last().unwrap()["type"], "response.failed");
        assert!(!output.iter().any(|event| matches!(
            event["type"].as_str(),
            Some("response.completed" | "response.output_item.done")
        )));
    }
}

#[tokio::test]
async fn accepts_stream_end_markers() {
    for (reason, terminal, detail) in [
        ("STOP", "response.completed", Value::Null),
        (
            "MAX_TOKENS",
            "response.incomplete",
            json!("max_output_tokens"),
        ),
        ("SAFETY", "response.incomplete", json!("content_filter")),
    ] {
        let output = events(vec![sse(json!({"candidates":[{
            "content":{"parts":[{"text":"Hello"}]},"finishReason":reason
        }]}))])
        .await;
        assert_eq!(output.last().unwrap()["type"], terminal);
        assert_eq!(
            output.last().unwrap()["response"]["incomplete_details"]["reason"],
            detail
        );
        assert!(
            !output
                .iter()
                .any(|event| event["type"] == "response.failed")
        );
    }
    let output = events(vec![
        sse(json!({"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]})),
        Bytes::from_static(b"data: [DONE]\n\n"),
    ])
    .await;
    assert_eq!(output.last().unwrap()["type"], "response.completed");
}

#[tokio::test]
async fn preserves_independent_calls() {
    for (first_id, second_id, expected) in [
        (Some("call_a"), Some("call_b"), vec!["call_a", "call_b"]),
        (Some("call_a"), Some("call_a"), vec!["call_a"]),
        (None, None, vec!["call_get_weather", "call_get_weather_1"]),
        (Some("call_a"), None, vec!["call_a", "call_get_weather_1"]),
    ] {
        let mut chunks = Vec::new();
        for id in [first_id, second_id] {
            let mut call = json!({"name":"get_weather","args":{"city":"Tokyo"}});
            if let Some(id) = id {
                call["id"] = json!(id);
            }
            chunks.push(sse(
                json!({"candidates":[{"content":{"parts":[{"functionCall":call}]}}]}),
            ));
        }
        chunks.push(sse(json!({"candidates":[{"finishReason":"STOP"}]})));
        let output = events(chunks).await;
        assert_eq!(output.last().unwrap()["type"], "response.completed");
        let calls: Vec<_> = output
            .iter()
            .filter(|event| {
                event["type"] == "response.output_item.done"
                    && event["item"]["type"] == "function_call"
            })
            .map(|event| event["item"]["call_id"].as_str().unwrap())
            .collect();
        assert_eq!(calls, expected);
    }
}

#[tokio::test]
async fn accepts_fragmented_stream() {
    let text = "\u{4f60}\u{597d}";
    let payload = sse(json!({"candidates":[{
        "content":{"parts":[{"text":text}]},"finishReason":"STOP"
    }]}));
    let mut chunks: Vec<_> = payload
        .iter()
        .map(|byte| Bytes::copy_from_slice(&[*byte]))
        .collect();
    chunks.push(sse(
        json!({"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":5}}),
    ));
    let output = events(chunks).await;
    let completed = output.last().unwrap();
    assert_eq!(completed["type"], "response.completed");
    assert_eq!(completed["response"]["usage"]["input_tokens"], 12);
    assert_eq!(completed["response"]["usage"]["output_tokens"], 5);
    assert!(
        output
            .iter()
            .any(|event| event["type"] == "response.output_text.delta" && event["delta"] == text)
    );
}

#[test]
fn converts_google_search_tools() {
    let converted = responses_to_gemini(&json!({"model":"gemini-future","input":"latest news",
        "tools":[{"type":"web_search"},{"type":"function","name":"lookup","parameters":{"type":"object"}}]
    }), None).unwrap();
    assert!(
        converted.request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool.get("googleSearch").is_some())
    );
    assert!(
        responses_to_gemini(
            &json!({"model":"gemini-future","input":"news",
                "tools":[{"type":"web_search","external_web_access":false}]
            }),
            None
        )
        .is_err()
    );
}

#[test]
fn mixed_tools_config_is_scoped() {
    for search_type in ["web_search", "web_search_preview"] {
        let search = json!({"type":search_type});
        let function = json!({"type":"function", "name":"lookup", "parameters":{"type":"object"}});
        for (tools, choice, enabled) in [
            (json!([search, function]), "auto", true),
            (json!([search]), "auto", false),
            (json!([function]), "auto", false),
            (json!([search, function]), "none", false),
            (json!([]), "auto", false),
        ] {
            let converted = responses_to_gemini(
                &json!({
                    "model":"gemini-3.8-flash-high", "input":"hello",
                    "tools":tools, "tool_choice":choice
                }),
                None,
            )
            .unwrap();
            let config = converted.request.get("toolConfig");
            assert_eq!(config.is_some(), enabled, "tools={tools} choice={choice}");
            if enabled {
                assert_eq!(config.unwrap()["includeServerSideToolInvocations"], true);
                assert_eq!(converted.request["tools"].as_array().unwrap().len(), 2);
            }
            if choice == "none" {
                assert!(converted.request.get("tools").is_none());
            }
        }
    }
}

#[tokio::test]
async fn preserves_search_citations() {
    let output = events(vec![sse(json!({"candidates":[{
        "content":{"parts":[{"text":"\u{4f60}\u{597d}"}]},
        "groundingMetadata":{
            "webSearchQueries":["query"],
            "groundingChunks":[{"web":{"uri":"https://example.com/source","title":"Source"}}],
            "groundingSupports":[{"segment":{"startIndex":0,"endIndex":6},"groundingChunkIndices":[0]}]
        },"finishReason":"STOP"
    }]}))]).await;
    let completed = output.last().unwrap();
    assert_eq!(completed["type"], "response.completed");
    let items = completed["response"]["output"].as_array().unwrap();
    let text = items.iter().find(|item| item["type"] == "message").unwrap();
    assert_eq!(
        text["content"][0]["annotations"][0]["url"],
        "https://example.com/source"
    );
    assert_eq!(text["content"][0]["annotations"][0]["end_index"], 2);
    assert!(items.iter().any(|item| item["type"] == "web_search_call"));
}
