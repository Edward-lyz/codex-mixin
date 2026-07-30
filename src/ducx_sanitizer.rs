use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, OriginalUri, State};
use axum::http::{HeaderMap, Response, StatusCode, header};
use axum::routing::post;
use futures_util::StreamExt;
use futures_util::stream;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use uuid::Uuid;

use crate::upstream::ResponseStream;

const POLICY_TTL: Duration = Duration::from_secs(10 * 60);
const REQUEST_BODY_LIMIT: usize = 16 * 1024 * 1024;
const REQUEST_ID_KEY: &str = "codex_mixin_request_id";

#[derive(Clone)]
struct SanitizerState {
    upstream_base_url: Arc<str>,
    client: reqwest::Client,
    policies: Arc<Mutex<HashMap<String, RequestPolicy>>>,
}

#[derive(Clone)]
struct RequestPolicy {
    created_at: Instant,
    request: Value,
    upstream_model: String,
    downstream: Arc<Mutex<Option<oneshot::Sender<ResponseStream>>>>,
}

pub(crate) struct DucxSanitizer {
    base_url: String,
    state: SanitizerState,
    shutdown: watch::Sender<bool>,
}

impl DucxSanitizer {
    pub(crate) async fn spawn(
        upstream_base_url: String,
        client: reqwest::Client,
    ) -> anyhow::Result<Self> {
        ensure!(
            upstream_base_url.starts_with("http://") || upstream_base_url.starts_with("https://"),
            "DUCX OneAPI base URL must use HTTP or HTTPS"
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind DUCX sanitizer")?;
        let address = listener
            .local_addr()
            .context("read DUCX sanitizer address")?;
        let state = SanitizerState {
            upstream_base_url: Arc::from(upstream_base_url.trim_end_matches('/')),
            client,
            policies: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = Router::new()
            .route(
                "/v1/responses",
                post(forward).layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT)),
            )
            .with_state(state.clone());
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        tokio::spawn(async move {
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.changed().await;
                })
                .await;
            if let Err(error) = result {
                tracing::warn!(%error, "DUCX sanitizer stopped unexpectedly");
            }
        });
        Ok(Self {
            base_url: format!("http://{address}/v1"),
            state,
            shutdown,
        })
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) async fn register(
        &self,
        request: &Value,
        upstream_model: &str,
    ) -> anyhow::Result<(String, oneshot::Receiver<ResponseStream>)> {
        ensure!(request.is_object(), "Responses request must be an object");
        let request_id = Uuid::new_v4().to_string();
        let (downstream_sender, downstream_receiver) = oneshot::channel();
        let now = Instant::now();
        let mut policies = self.state.policies.lock().await;
        policies.retain(|_, policy| now.duration_since(policy.created_at) < POLICY_TTL);
        policies.insert(
            request_id.clone(),
            RequestPolicy {
                created_at: now,
                request: request.clone(),
                upstream_model: upstream_model.to_owned(),
                downstream: Arc::new(Mutex::new(Some(downstream_sender))),
            },
        );
        Ok((request_id, downstream_receiver))
    }
}

impl Drop for DucxSanitizer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn forward(
    State(state): State<SanitizerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    match forward_inner(state, uri, headers, body).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "DUCX sanitizer rejected request");
            Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"error":{"message":"DUCX sanitizer rejected the upstream request"}}"#,
                ))
                .expect("static DUCX sanitizer error response is valid")
        }
    }
}

async fn forward_inner(
    state: SanitizerState,
    uri: axum::http::Uri,
    mut headers: HeaderMap,
    body: Bytes,
) -> anyhow::Result<Response<Body>> {
    let mut payload: Value = serde_json::from_slice(&body).context("decode DUCX OneAPI request")?;
    let request_id = metadata_request_id(&payload).context("missing DUCX turn request id")?;
    let policy = state
        .policies
        .lock()
        .await
        .get(&request_id)
        .cloned()
        .context("unknown or expired DUCX turn request id")?;
    sanitize_payload(&mut payload, &policy)?;
    let encoded = serde_json::to_vec(&payload).context("encode sanitized DUCX request")?;
    let target = upstream_url(&state.upstream_base_url, &uri);
    headers.remove(header::HOST);
    headers.remove(header::CONTENT_LENGTH);
    let upstream = state
        .client
        .post(target)
        .headers(headers)
        .body(encoded)
        .send()
        .await
        .context("forward sanitized DUCX request")?;
    let status = upstream.status();
    let response_headers = upstream.headers().clone();
    let (downstream_sender, downstream_receiver) = mpsc::channel(16);
    let (ducx_sender, ducx_receiver) = mpsc::channel(16);
    let downstream_stream = stream::unfold(downstream_receiver, |mut receiver| async {
        receiver.recv().await.map(|item| (item, receiver))
    })
    .boxed();
    let response_sender = policy
        .downstream
        .lock()
        .await
        .take()
        .context("DUCX turn already opened an upstream response")?;
    response_sender
        .send(downstream_stream)
        .map_err(|_| anyhow::anyhow!("DUCX downstream response receiver closed"))?;
    tokio::spawn(async move {
        let mut bytes = upstream.bytes_stream();
        while let Some(chunk) = bytes.next().await {
            let Ok(chunk) = chunk else {
                break;
            };
            let _ = downstream_sender
                .send(Ok::<_, Infallible>(chunk.clone()))
                .await;
            let _ = ducx_sender.send(Ok::<_, std::io::Error>(chunk)).await;
        }
    });
    let mut builder = Response::builder().status(status);
    if let Some(target_headers) = builder.headers_mut() {
        for (name, value) in response_headers {
            if let Some(name) = name
                && name != header::CONTENT_LENGTH
                && name != header::TRANSFER_ENCODING
            {
                target_headers.append(name, value);
            }
        }
    }
    let ducx_stream = stream::unfold(ducx_receiver, |mut receiver| async {
        receiver.recv().await.map(|item| (item, receiver))
    });
    builder
        .body(Body::from_stream(ducx_stream))
        .context("build DUCX sanitizer response")
}

fn metadata_request_id(payload: &Value) -> Option<String> {
    let metadata = payload
        .pointer("/client_metadata/x-codex-turn-metadata")?
        .as_str()?;
    serde_json::from_str::<Value>(metadata)
        .ok()?
        .get(REQUEST_ID_KEY)?
        .as_str()
        .map(str::to_owned)
}

fn sanitize_payload(payload: &mut Value, policy: &RequestPolicy) -> anyhow::Result<()> {
    let mut sanitized = policy.request.clone();
    let object = sanitized
        .as_object_mut()
        .context("original Responses request must be an object")?;
    object.insert(
        "model".to_owned(),
        Value::String(policy.upstream_model.clone()),
    );
    object.insert("stream".to_owned(), Value::Bool(true));
    *payload = sanitized;
    Ok(())
}

fn upstream_url(base_url: &str, uri: &axum::http::Uri) -> String {
    let path = uri.path().strip_prefix("/v1").unwrap_or(uri.path());
    match uri.query() {
        Some(query) => format!("{base_url}{path}?{query}"),
        None => format!("{base_url}{path}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use futures_util::StreamExt;
    use serde_json::json;
    use tokio::sync::oneshot;

    #[test]
    fn removes_ducx_additions_and_keeps_declared_tools() {
        let policy = RequestPolicy {
            created_at: Instant::now(),
            request: json!({
                "model":"catalog-slug",
                "instructions":"caller instructions",
                "input":[{"type":"message","role":"user","content":"hi"}],
                "tools":[{"type":"function","name":"lookup"}]
            }),
            upstream_model: "upstream-model".to_owned(),
            downstream: Arc::new(Mutex::new(None)),
        };
        let mut payload = json!({
            "instructions": "DUCX platform prompt",
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]},
                {"type":"additional_tools","tools":[
                    {"name":"exec"},
                    {"name":"lookup"},
                    {"name":"wait"}
                ]}
            ],
            "client_metadata": {
                "x-codex-turn-metadata":
                    "{\"codex_mixin_request_id\":\"request-1\",\"preserved\":\"yes\"}"
            }
        });

        sanitize_payload(&mut payload, &policy).unwrap();

        assert_eq!(payload["instructions"], "caller instructions");
        assert_eq!(payload["input"][0]["role"], "user");
        assert_eq!(
            payload["tools"],
            json!([{"type":"function","name":"lookup"}])
        );
        assert_eq!(payload["model"], "upstream-model");
        assert_eq!(payload["stream"], true);
        assert!(payload.get("client_metadata").is_none());
    }

    #[test]
    fn removes_empty_additional_tools_item() {
        let policy = RequestPolicy {
            created_at: Instant::now(),
            request: json!({"input":[{"type":"message","role":"user"}]}),
            upstream_model: "upstream-model".to_owned(),
            downstream: Arc::new(Mutex::new(None)),
        };
        let mut payload = json!({
            "input": [
                {"type":"message","role":"user"},
                {"type":"additional_tools","tools":[{"name":"request_user_input"}]}
            ]
        });

        sanitize_payload(&mut payload, &policy).unwrap();

        assert_eq!(payload["input"], json!([{"type":"message","role":"user"}]));
        assert!(payload.get("instructions").is_none());
        assert!(payload.get("tools").is_none());
    }

    #[test]
    fn maps_proxy_v1_path_to_upstream_base() {
        let uri = "/v1/responses?beta=true".parse().unwrap();
        assert_eq!(
            upstream_url("http://oneapi.example/v1", &uri),
            "http://oneapi.example/v1/responses?beta=true"
        );
    }

    #[tokio::test]
    async fn forwards_header_image_and_sanitized_payload() {
        type Capture = Arc<Mutex<Option<oneshot::Sender<(HeaderMap, Value)>>>>;

        async fn capture(
            State(capture): State<Capture>,
            headers: HeaderMap,
            Json(payload): Json<Value>,
        ) -> &'static str {
            if let Some(sender) = capture.lock().await.take() {
                let _ = sender.send((headers, payload));
            }
            "upstream response"
        }

        let (sender, receiver) = oneshot::channel();
        let capture_state = Arc::new(Mutex::new(Some(sender)));
        let upstream = Router::new()
            .route("/v1/responses", post(capture))
            .with_state(capture_state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let client = reqwest::Client::new();
        let sanitizer = DucxSanitizer::spawn(upstream_base_url, client.clone())
            .await
            .unwrap();
        let image = "data:image/png;base64,AAAA";
        let (request_id, _downstream) = sanitizer
            .register(
                &json!({
                    "model": "catalog-slug",
                    "instructions": "caller instructions",
                    "input": [
                        {"type":"message","content":[{"type":"input_image","image_url":image}]}
                    ],
                    "tools": [{"type":"function","name":"lookup"}]
                }),
                "upstream-model",
            )
            .await
            .unwrap();
        let response = client
            .post(format!("{}/responses", sanitizer.base_url()))
            .header("comate_custom_header", "present-but-not-logged")
            .json(&json!({
                "instructions": "DUCX platform prompt",
                "input": [
                    {"type":"message","content":[{"type":"input_image","image_url":image}]},
                    {"type":"additional_tools","tools":[
                        {"name":"exec"},
                        {"name":"lookup"},
                        {"name":"wait"},
                        {"name":"request_user_input"}
                    ]}
                ],
                "client_metadata": {
                    "x-codex-turn-metadata": serde_json::to_string(&json!({
                        "codex_mixin_request_id": request_id
                    })).unwrap()
                }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "upstream response");

        let (headers, payload) = receiver.await.unwrap();
        assert!(headers.contains_key("comate_custom_header"));
        assert_eq!(payload["instructions"], "caller instructions");
        assert_eq!(payload["input"][0]["content"][0]["image_url"], image);
        assert_eq!(
            payload["tools"],
            json!([{"type":"function","name":"lookup"}])
        );
        assert_eq!(payload["model"], "upstream-model");
        assert!(payload.get("client_metadata").is_none());
    }

    #[tokio::test]
    #[ignore = "requires an installed, logged-in DUCX"]
    async fn real_ducx_round_trip_is_thin() {
        type Capture = Arc<Mutex<Option<oneshot::Sender<(HeaderMap, Value)>>>>;

        async fn capture_ducx(
            State(capture): State<Capture>,
            headers: HeaderMap,
            Json(payload): Json<Value>,
        ) -> Response<Body> {
            if let Some(sender) = capture.lock().await.take() {
                let _ = sender.send((headers, payload.clone()));
            }
            let completed = json!({
                "id": "resp_ducx_sanitizer_test",
                "object": "response",
                "status": "completed",
                "model": payload.get("model").cloned().unwrap_or(Value::Null),
                "output": [{
                    "id": "call_ducx_sanitizer_test",
                    "type": "function_call",
                    "call_id": "call_ducx_sanitizer_test",
                    "name": "lookup",
                    "arguments": "{}",
                    "status": "completed",
                }],
                "usage": {"input_tokens":1,"output_tokens":1,"total_tokens":2}
            });
            let event = json!({"type":"response.completed","response":completed});
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(format!(
                    "event: response.completed\ndata: {event}\n\ndata: [DONE]\n\n"
                )))
                .unwrap()
        }

        let executable = crate::ducx::default_ducx_executable().expect("DUCX is not installed");
        let (sender, receiver) = oneshot::channel();
        let capture_state = Arc::new(Mutex::new(Some(sender)));
        let upstream = Router::new()
            .route("/v1/responses", post(capture_ducx))
            .with_state(capture_state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let client = reqwest::Client::new();
        let sanitizer = DucxSanitizer::spawn(upstream_base_url, client)
            .await
            .unwrap();
        let cwd = std::env::temp_dir();
        let config = crate::ducx::DucxProcessConfig::app_server(executable, &cwd)
            .with_oneapi_base_url(sanitizer.base_url());
        let timeout = Duration::from_secs(30);
        let app_server = crate::ducx::DucxAppServer::spawn_ready(config, timeout)
            .await
            .unwrap();
        let image = concat!(
            "data:image/png;base64,",
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk",
            "+A8AAQUBAScY42YAAAAASUVORK5CYII="
        );
        let request = json!({
            "instructions": "ONLY_CALLER_INSTRUCTIONS",
            "input": [{
                "type":"message",
                "role":"user",
                "content":[
                    {"type":"input_text","text":"Describe this image."},
                    {"type":"input_image","image_url":image,"detail":"high"}
                ]
            }],
            "tools": [{
                "type":"function",
                "name":"lookup",
                "description":"Lookup a value",
                "parameters":{"type":"object","properties":{}}
            }]
        });
        let (request_id, downstream) = sanitizer.register(&request, "gpt-5.6-luna").await.unwrap();
        let trigger = json!({"input":"Open the authenticated upstream Responses transport."});
        let (thread, mut turn) =
            crate::ducx::build_turn_params(&trigger, "gpt-5.6-luna", &cwd).unwrap();
        turn["responsesapiClientMetadata"] = json!({"codex_mixin_request_id":request_id});
        let mut events = app_server
            .start_turn(thread, turn, timeout)
            .await
            .unwrap()
            .into_stream();
        tokio::spawn(async move { while events.next().await.is_some() {} });
        let downstream_bytes = tokio::time::timeout(Duration::from_secs(5), downstream)
            .await
            .unwrap()
            .unwrap()
            .fold(Vec::new(), |mut bytes, chunk| async move {
                bytes.extend_from_slice(&chunk.unwrap());
                bytes
            })
            .await;
        assert!(
            String::from_utf8(downstream_bytes)
                .unwrap()
                .contains(r#""name":"lookup""#)
        );

        let (headers, payload) = receiver.await.unwrap();
        assert!(headers.contains_key("comate_custom_header"));
        assert_eq!(payload["instructions"], "ONLY_CALLER_INSTRUCTIONS");
        assert!(serde_json::to_string(&payload).unwrap().contains(image));
        assert_eq!(payload["tools"], request["tools"]);
        assert!(
            payload["input"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["role"] != "developer")
        );
    }
}
