//! HTTP surface of the web control panel.
//!
//! Contract (all `/api/*` routes require `Authorization: Bearer <token>`):
//! - `GET /api/snapshot?usage=1d|7d|30d|all` -> `{status, providers,
//!   codex_install_mode, fusion_profile, gateway_bind, gateway_auth_configured,
//!   benchmark, usage, models, panel, refreshed_at_ms}`; mirrors `info`,
//!   `providers list`, `benchmark status`,
//!   `usage` and `fusion models` JSON.
//! - `GET /api/quota` -> `quota --json`; `GET /api/doctor` ->
//!   `doctor --quick --json`; `GET /api/logs?lines=N` -> `{text}`.
//! - `POST /api/actions` with `{action, params?}` (see `actions.rs`) ->
//!   `{ok: true, stdout, json}`. One action runs at a time; a concurrent
//!   request gets HTTP 409 with code `busy`.
//! - Errors: `{error: {code, message, stage, committed}}` copied from the
//!   CLI `--json-errors` envelope; `committed: true` means the change was
//!   saved but a later step failed.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use super::actions::PanelAction;
use super::cli::{CliFailure, run_cli, run_json};

const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_LOG_LINES: u32 = 2_000;
const DEFAULT_LOG_LINES: u32 = 200;
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
     img-src 'self' data:; connect-src 'self'; font-src 'self'; object-src 'none'; \
     base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_CSS: &str = include_str!("assets/app.css");
const APP_JS: &str = include_str!("assets/app.js");

pub(super) struct PanelState {
    pub(super) token: String,
    pub(super) start_page: &'static str,
    pub(super) installed_cli_path: Option<String>,
    /// Serializes CLI mutations; config commits must not interleave.
    pub(super) action_lock: tokio::sync::Mutex<()>,
}

type Shared = Arc<PanelState>;

struct ApiError {
    status: StatusCode,
    failure: CliFailure,
}

impl ApiError {
    fn invalid(message: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            failure: CliFailure::panel("invalid_arguments", message, Value::Bool(false)),
        }
    }
}

impl From<CliFailure> for ApiError {
    fn from(failure: CliFailure) -> Self {
        let status = match failure.code.as_str() {
            "invalid_arguments" => StatusCode::BAD_REQUEST,
            "timeout" => StatusCode::GATEWAY_TIMEOUT,
            _ => StatusCode::BAD_GATEWAY,
        };
        Self { status, failure }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.failure.to_json())).into_response()
    }
}

pub(super) fn router(state: Shared) -> Router {
    let api = Router::new()
        .route("/api/snapshot", get(snapshot))
        .route("/api/quota", get(quota))
        .route("/api/doctor", get(doctor))
        .route("/api/logs", get(logs))
        .route("/api/actions", post(action))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token));
    Router::new()
        .route(
            "/",
            get(|| async { asset("text/html; charset=utf-8", INDEX_HTML) }),
        )
        .route(
            "/app.css",
            get(|| async { asset("text/css; charset=utf-8", APP_CSS) }),
        )
        .route(
            "/app.js",
            get(|| async { asset("text/javascript; charset=utf-8", APP_JS) }),
        )
        .merge(api)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (name, value) in [
        (header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::CACHE_CONTROL, "no-store"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response
}

pub(super) fn token_matches(expected: &str, authorization: Option<&str>) -> bool {
    authorization
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|presented| bool::from(presented.as_bytes().ct_eq(expected.as_bytes())))
}

async fn require_token(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if !token_matches(&state.token, authorization) {
        return ApiError {
            status: StatusCode::UNAUTHORIZED,
            failure: CliFailure::panel(
                "unauthorized",
                "missing or invalid panel access token".to_owned(),
                Value::Bool(false),
            ),
        }
        .into_response();
    }
    next.run(request).await
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
enum UsageRange {
    #[serde(rename = "1d")]
    Day,
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
    #[default]
    #[serde(rename = "all")]
    All,
}

impl UsageRange {
    fn args(self) -> &'static [&'static str] {
        match self {
            Self::Day => &["usage", "--json", "--days", "1"],
            Self::Week => &["usage", "--json", "--days", "7"],
            Self::Month => &["usage", "--json", "--days", "30"],
            Self::All => &["usage", "--json"],
        }
    }
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    #[serde(default)]
    usage: UsageRange,
}

async fn snapshot(
    State(state): State<Shared>,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<Value>, ApiError> {
    let status = run_json(&["info", "--json"]).await?;
    let configured = status
        .get("configured")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let running = status.get("gateway").and_then(Value::as_str) == Some("running");
    let providers = async {
        if configured {
            run_json(&["providers", "list", "--json"]).await
        } else {
            Ok(Value::Null)
        }
    };
    let benchmark = async {
        if running {
            run_json(&["benchmark", "status"])
                .await
                .map(|document| document.get("snapshot").cloned().unwrap_or(Value::Null))
        } else {
            Ok(Value::Null)
        }
    };
    let usage = async {
        if running {
            run_json(query.usage.args()).await
        } else {
            Ok(json!([]))
        }
    };
    let models = async {
        if configured {
            run_json(&["fusion", "models", "--json"])
                .await
                .map(|document| document.get("models").cloned().unwrap_or(json!([])))
        } else {
            Ok(json!([]))
        }
    };
    let (providers, benchmark, usage, models) =
        tokio::try_join!(providers, benchmark, usage, models)?;
    let refreshed_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    Ok(Json(json!({
        "status": status,
        "providers": providers.get("providers").cloned().unwrap_or(json!([])),
        "codex_install_mode": providers.get("codex_install_mode").cloned().unwrap_or(Value::Null),
        "fusion_profile": providers.get("fusion_profile").cloned().unwrap_or(Value::Null),
        "gateway_bind": providers.get("gateway_bind").cloned().unwrap_or(Value::Null),
        "gateway_auth_configured": providers
            .get("gateway_auth_configured")
            .cloned()
            .unwrap_or(Value::Null),
        "benchmark": benchmark,
        "usage": usage,
        "models": models,
        "panel": {
            "version": env!("CARGO_PKG_VERSION"),
            // Without this build feature the gateway accepts every client.
            "client_auth_build": cfg!(feature = "client-auth"),
            "start_page": state.start_page,
            "installed_cli_path": state.installed_cli_path,
        },
        "refreshed_at_ms": refreshed_at_ms,
    })))
}

async fn quota() -> Result<Json<Value>, ApiError> {
    Ok(Json(run_json(&["quota", "--json"]).await?))
}

async fn doctor() -> Result<Json<Value>, ApiError> {
    Ok(Json(run_json(&["doctor", "--quick", "--json"]).await?))
}

#[derive(Debug, Deserialize)]
struct LogsQuery {
    lines: Option<u32>,
}

async fn logs(Query(query): Query<LogsQuery>) -> Result<Json<Value>, ApiError> {
    let lines = query
        .lines
        .unwrap_or(DEFAULT_LOG_LINES)
        .clamp(1, MAX_LOG_LINES);
    let args = vec![
        "service".to_owned(),
        "logs".to_owned(),
        "-n".to_owned(),
        lines.to_string(),
    ];
    let text = run_cli(&args, &[]).await?;
    Ok(Json(json!({ "text": text })))
}

async fn action(
    State(state): State<Shared>,
    Json(action): Json<PanelAction>,
) -> Result<Json<Value>, ApiError> {
    let invocation = action.invocation().map_err(ApiError::invalid)?;
    let Ok(_guard) = state.action_lock.try_lock() else {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            failure: CliFailure::panel(
                "busy",
                "another panel action is still running".to_owned(),
                Value::Bool(false),
            ),
        });
    };
    let stdout = run_cli(&invocation.args, &invocation.secrets).await?;
    let parsed = if action.prints_json() {
        serde_json::from_str(&stdout).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    Ok(Json(
        json!({ "ok": true, "stdout": stdout, "json": parsed }),
    ))
}
