//! Record/replay corpus for protocol conversion.
//!
//! Each directory under `tests/fixtures/protocol_corpus/` holds one case:
//! `case.json` (client endpoint, request and upstream protocol), `upstream.sse`
//! (the raw stream the provider sends back) and `expected.json` (the request
//! the provider must receive and the events the client must see). Run with
//! `CODEX_MIXIN_RECORD=1` to rewrite `expected.json` after an intended change,
//! then review the diff.

use super::*;
use std::path::{Path, PathBuf};

/// Values the gateway generates per request; replaced before comparing.
const VOLATILE_KEYS: [&str; 5] = ["id", "item_id", "response_id", "created_at", "created"];

#[derive(serde::Deserialize)]
struct Case {
    endpoint: String,
    protocol: ProviderProtocol,
    api_path: String,
    request: Value,
}

#[derive(Clone)]
struct Replay {
    stream: String,
    requests: Arc<Mutex<Vec<Value>>>,
}

async fn replay_upstream(
    State(replay): State<Replay>,
    OriginalUri(uri): OriginalUri,
    Json(body): Json<Value>,
) -> Response {
    replay
        .requests
        .lock()
        .unwrap()
        .push(json!({"path": uri.path(), "body": body}));
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(replay.stream))
        .unwrap()
}

fn scrub(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if VOLATILE_KEYS.contains(&key.as_str()) && !item.is_null() {
                    *item = json!("<volatile>");
                } else {
                    scrub(item);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(scrub),
        _ => {}
    }
}

fn client_events(body: &str) -> Vec<Value> {
    let mut buffer = body.as_bytes().to_vec();
    let mut events: Vec<Value> = drain_events(&mut buffer)
        .into_iter()
        .filter(|event| event.data != "[DONE]")
        .map(|event| serde_json::from_str(&event.data).unwrap())
        .collect();
    events.iter_mut().for_each(scrub);
    events
}

async fn run_case(dir: &Path) -> Value {
    let case: Case =
        serde_json::from_str(&std::fs::read_to_string(dir.join("case.json")).unwrap()).unwrap();
    let replay = Replay {
        stream: std::fs::read_to_string(dir.join("upstream.sse")).unwrap(),
        requests: Arc::default(),
    };
    let upstream = spawn_router(
        Router::new()
            .fallback(post(replay_upstream))
            .with_state(replay.clone()),
    )
    .await;
    let mut config = test_config(upstream);
    config.providers[0].protocol = case.protocol;
    config.providers[0].api_path = case.api_path;
    let gateway = spawn_gateway_with_config(config).await;
    let response = reqwest::Client::new()
        .post(format!("{gateway}{}", case.endpoint))
        .bearer_auth("gateway-key")
        .json(&case.request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{}", dir.display());
    let events = client_events(&response.text().await.unwrap());
    let mut upstream_requests = Value::Array(replay.requests.lock().unwrap().clone());
    scrub(&mut upstream_requests);
    json!({"upstream_requests": upstream_requests, "client_events": events})
}

#[tokio::test]
async fn protocol_corpus_replays_unchanged() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/protocol_corpus");
    let record = std::env::var_os("CODEX_MIXIN_RECORD").is_some();
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "no corpus cases in {}", root.display());
    for dir in cases {
        let actual = run_case(&dir).await;
        let expected_path = dir.join("expected.json");
        if record {
            let text = serde_json::to_string_pretty(&actual).unwrap() + "\n";
            std::fs::write(&expected_path, text).unwrap();
            continue;
        }
        let expected: Value = serde_json::from_str(
            &std::fs::read_to_string(&expected_path)
                .unwrap_or_else(|_| panic!("missing {}", expected_path.display())),
        )
        .unwrap();
        assert_eq!(actual, expected, "corpus case {}", dir.display());
    }
}
