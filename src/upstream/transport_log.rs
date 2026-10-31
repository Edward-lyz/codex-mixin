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
