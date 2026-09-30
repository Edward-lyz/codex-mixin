//! Request-local transport evidence. Extensions never become upstream headers.
use std::time::Instant;

use reqwest::{Client, Request, RequestBuilder, Response};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OfficialTransport {
    Ech(Uuid),
    DirectFallback,
    Default,
    Untracked,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TransportTrace {
    request_id: Uuid,
    transport: OfficialTransport,
    attempt: u8,
    request_kind: &'static str,
}

impl TransportTrace {
    pub(crate) fn new(transport: OfficialTransport) -> Self {
        Self {
            request_id: Uuid::new_v4(),
            transport,
            attempt: 1,
            request_kind: "gateway",
        }
    }

    pub(crate) fn diagnostic(transport: OfficialTransport) -> Self {
        Self {
            request_kind: "diagnostic",
            ..Self::new(transport)
        }
    }

    pub(crate) fn uses_ech(self) -> bool {
        matches!(self.transport, OfficialTransport::Ech(_))
    }

    pub(crate) fn direct_retry(self) -> Self {
        Self {
            transport: OfficialTransport::DirectFallback,
            attempt: 2,
            ..self
        }
    }

    fn name(self) -> &'static str {
        match self.transport {
            OfficialTransport::Ech(_) => "ech",
            OfficialTransport::DirectFallback => "direct_fallback",
            OfficialTransport::Default => "default",
            OfficialTransport::Untracked => "untracked",
        }
    }

    fn transport_id(self) -> Uuid {
        match self.transport {
            OfficialTransport::Ech(id) => id,
            _ => Uuid::nil(),
        }
    }

    pub(crate) async fn execute(
        self,
        client: &Client,
        request: Request,
    ) -> reqwest::Result<Response> {
        let host = request.url().host_str().unwrap_or("unknown").to_owned();
        let method = request.method().clone();
        let started = Instant::now();
        tracing::info!(event = "official_http_request", request_id = %self.request_id, request_kind = self.request_kind,
            transport = self.name(), transport_id = %self.transport_id(), attempt = self.attempt,
            host, method = %method, "official HTTP request started");
        let response = client.execute(request).await;
        match &response {
            Ok(response) => {
                // rustls with_ech rejects the handshake before HTTP when ECH
                // is not accepted. A response from that client proves ECH.
                tracing::info!(event = "official_http_response", request_id = %self.request_id, request_kind = self.request_kind,
                    transport = self.name(), transport_id = %self.transport_id(), attempt = self.attempt,
                    host, method = %method, status = response.status().as_u16(),
                    peer_ip = ?response.remote_addr().map(|peer| peer.ip()),
                    peer_port = ?response.remote_addr().map(|peer| peer.port()),
                    ech_accepted = self.uses_ech(), elapsed_ms = started.elapsed().as_millis(),
                    phase = "response_headers", "official HTTP response received");
            }
            Err(error) => {
                // Error URLs can contain query secrets. Log categories only.
                tracing::warn!(event = "official_http_error", request_id = %self.request_id, request_kind = self.request_kind,
                    transport = self.name(), transport_id = %self.transport_id(), attempt = self.attempt,
                    host, method = %method, connect_error = error.is_connect(), timeout = error.is_timeout(),
                    elapsed_ms = started.elapsed().as_millis(), "official HTTP request failed");
            }
        }
        response
    }
}

pub(crate) fn attach_trace(
    builder: RequestBuilder,
    transport: OfficialTransport,
) -> reqwest::Result<RequestBuilder> {
    attach_trace_for(builder, TransportTrace::new(transport))
}

pub(crate) fn attach_trace_for(
    builder: RequestBuilder,
    trace: TransportTrace,
) -> reqwest::Result<RequestBuilder> {
    let (client, request) = builder.build_split();
    let request = request?;
    let no_body = request.body().is_none();
    let mut message: axum::http::Request<reqwest::Body> = request.try_into()?;
    message.extensions_mut().insert(trace);
    let mut request = Request::try_from(message)?;
    if no_body {
        *request.body_mut() = None;
    }
    Ok(RequestBuilder::from_parts(client, request))
}

pub(crate) fn take_trace(request: Request) -> reqwest::Result<(Request, Option<TransportTrace>)> {
    let no_body = request.body().is_none();
    let mut message: axum::http::Request<reqwest::Body> = request.try_into()?;
    let trace = message.extensions_mut().remove::<TransportTrace>();
    let mut request = Request::try_from(message)?;
    if no_body {
        *request.body_mut() = None;
    }
    Ok((request, trace))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing::instrument::WithSubscriber;

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    impl Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn request_trace_survives_changes_and_never_becomes_a_header() {
        let id = Uuid::new_v4();
        let builder = attach_trace(
            Client::new().post("https://chatgpt.com/backend-api/codex/responses"),
            OfficialTransport::Ech(id),
        )
        .unwrap()
        .bearer_auth("test-token")
        .header("x-request-id", "upstream-id")
        .json(&serde_json::json!({"input": "private prompt"}));
        let (_, request) = builder.build_split();
        let request = request.unwrap();
        let cloned = request.try_clone().unwrap();
        let (request, trace) = take_trace(request).unwrap();
        let (cloned, clone_trace) = take_trace(cloned).unwrap();
        let trace = trace.unwrap();
        assert_eq!(trace.request_id, clone_trace.unwrap().request_id);
        assert_eq!(trace.transport, OfficialTransport::Ech(id));
        assert_eq!(request.headers(), cloned.headers());
        assert_eq!(request.headers().len(), 3);
        assert_eq!(
            request.body().unwrap().as_bytes(),
            cloned.body().unwrap().as_bytes()
        );
        let retry = trace.direct_retry();
        assert_eq!(retry.request_id, trace.request_id);
        assert_eq!(retry.transport, OfficialTransport::DirectFallback);
        assert_eq!(retry.attempt, 2);
    }

    #[test]
    fn attaching_trace_preserves_absent_body_and_timeout() {
        let builder = Client::new()
            .get("https://api.openai.com/v1/models")
            .timeout(std::time::Duration::from_secs(3));
        let builder = attach_trace(builder, OfficialTransport::Default).unwrap();
        let (_, request) = builder.build_split();
        let (request, trace) = take_trace(request.unwrap()).unwrap();
        assert!(request.body().is_none());
        assert_eq!(
            request.timeout().copied(),
            Some(std::time::Duration::from_secs(3))
        );
        assert_eq!(trace.unwrap().transport, OfficialTransport::Default);
    }

    #[tokio::test]
    async fn actual_response_logs_peer_without_request_secrets() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(axum::routing::post(
            |headers: axum::http::HeaderMap, body: axum::body::Bytes| async move {
                assert_eq!(headers["authorization"], "Bearer private-token");
                assert!(
                    headers
                        .keys()
                        .all(|name| !name.as_str().contains("transport")
                            && !name.as_str().contains("trace"))
                );
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["input"],
                    "private-prompt"
                );
                (axum::http::StatusCode::ACCEPTED, "ok")
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = Client::builder().no_proxy().build().unwrap();
        let builder = attach_trace(
            client.post(format!(
                "http://{address}/private-path?api_key=private-query"
            )),
            OfficialTransport::DirectFallback,
        )
        .unwrap()
        .bearer_auth("private-token")
        .header("cookie", "private-cookie")
        .json(&serde_json::json!({"input": "private-prompt"}));
        let (_, request) = builder.build_split();
        let (request, trace) = take_trace(request.unwrap()).unwrap();
        let trace = trace.unwrap();
        let buffer = LogBuffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(move || writer.clone())
            .finish();
        let response = trace
            .execute(&client, request)
            .with_subscriber(subscriber)
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        let log = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        assert!(log.contains("official_http_request"));
        assert!(log.contains("official_http_response"));
        assert!(log.contains("direct_fallback"));
        assert!(log.contains("ech_accepted=false"));
        assert!(log.contains("status=202"));
        assert!(log.contains("peer_ip=Some(127.0.0.1)"));
        assert_eq!(log.matches(&trace.request_id.to_string()).count(), 2);
        for secret in [
            "private-token",
            "private-cookie",
            "private-query",
            "private-prompt",
            "private-path",
        ] {
            assert!(!log.contains(secret), "logged {secret}");
        }
        server.abort();
    }
}
