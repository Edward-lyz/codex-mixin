use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::config::{GatewayConfig, valid_cors_origin};
use crate::error::GatewayError;

const ALLOW_METHODS: &str = "GET, POST, OPTIONS";
const ALLOW_HEADERS: &str =
    "authorization, x-api-key, anthropic-version, anthropic-beta, content-type, x-goog-api-key";
const ALLOW_PRIVATE_NETWORK: &str = "access-control-allow-private-network";
const REQUEST_PRIVATE_NETWORK: &str = "access-control-request-private-network";

pub(super) async fn cors(
    State(config): State<Arc<GatewayConfig>>,
    request: Request,
    next: Next,
) -> Response {
    let preflight = request.method() == Method::OPTIONS;
    let origin = request.headers().get(header::ORIGIN).cloned();
    let allowed = origin.as_ref().is_some_and(|origin| {
        origin.to_str().is_ok_and(|origin| {
            config
                .cors_allowed_origins
                .iter()
                .any(|allowed| allowed == "*" || allowed == origin)
        })
    });
    let same_origin = !allowed
        && origin.as_ref().is_some_and(|origin| {
            origin
                .to_str()
                .is_ok_and(|origin| is_local_same_origin(origin, &request, &config))
        });
    let unique_origin = request.headers().get_all(header::ORIGIN).iter().count() <= 1;
    let allowed_method = matches!(
        *request.method(),
        Method::GET | Method::POST | Method::OPTIONS
    ) && request
        .headers()
        .get_all(header::ACCESS_CONTROL_REQUEST_METHOD)
        .iter()
        .all(|method| matches!(method.to_str(), Ok("GET" | "POST" | "OPTIONS")));
    let rejected =
        !unique_origin || (origin.is_some() && !same_origin && (!allowed || !allowed_method));
    let requested_headers = preflight.then(|| {
        request
            .headers()
            .get_all(header::ACCESS_CONTROL_REQUEST_HEADERS)
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    });
    let private_network = !rejected
        && preflight
        && request
            .headers()
            .get(REQUEST_PRIVATE_NETWORK)
            .is_some_and(|value| value == "true");
    // Never read or collect the body: the inner SSE stream keeps its backpressure.
    let mut response = if rejected {
        GatewayError::Forbidden("origin or method is not allowed by CORS policy".to_owned())
            .into_response()
    } else if preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    vary(&mut response, preflight);
    if allowed
        && unique_origin
        && let Some(origin) = origin
    {
        let headers = response.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(ALLOW_METHODS),
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("86400"),
        );
        if let Some(requested_headers) = requested_headers.filter(|headers| !headers.is_empty()) {
            headers.remove(header::ACCESS_CONTROL_ALLOW_HEADERS);
            for value in requested_headers {
                headers.append(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
            }
        } else {
            headers.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static(ALLOW_HEADERS),
            );
        }
        if private_network {
            headers.insert(ALLOW_PRIVATE_NETWORK, HeaderValue::from_static("true"));
        }
    }
    response
}

fn vary(response: &mut Response, preflight: bool) {
    // Append rather than replace the compression layer's Vary: accept-encoding.
    response
        .headers_mut()
        .append(header::VARY, HeaderValue::from_static("Origin"));
    if preflight {
        response.headers_mut().append(header::VARY, HeaderValue::from_static(
            "Access-Control-Request-Method, Access-Control-Request-Headers, Access-Control-Request-Private-Network",
        ));
    }
}

fn is_local_same_origin(origin: &str, request: &Request, config: &GatewayConfig) -> bool {
    if !valid_cors_origin(origin) {
        return false;
    }
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    // Host alone is attacker-controlled under DNS rebinding. Only the actual
    // listener's port and a local IP/localhost can use the same-origin exception.
    let local_host = match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(ip)) => {
            ip.is_loopback()
                || (!config.bind.ip().is_unspecified()
                    && std::net::IpAddr::V4(ip) == config.bind.ip())
        }
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback()
                || (!config.bind.ip().is_unspecified()
                    && std::net::IpAddr::V6(ip) == config.bind.ip())
        }
        _ => false,
    };
    local_host
        && url.port_or_known_default() == Some(config.bind.port())
        && request.headers().get_all(header::HOST).iter().count() == 1
        && request.headers().get(header::HOST).is_some_and(|host| {
            host.to_str()
                .is_ok_and(|host| origin == format!("http://{host}"))
        })
}
