use std::net::SocketAddr;
use std::time::Duration;

use super::{DoctorCheck, DoctorFix, DoctorStatus};

pub(super) struct LiveGateway {
    pub(super) bind: SocketAddr,
}

pub(super) async fn check_gateway_runtime() -> (DoctorCheck, Option<LiveGateway>) {
    let runtime = match super::super::runtime::load_runtime_metadata() {
        Ok(Some(runtime)) => runtime,
        Ok(None) => {
            return (
                DoctorCheck::new(
                    "gateway",
                    "Local gateway",
                    DoctorStatus::Warning,
                    "gateway is not running",
                )
                .hint("codex-mixin service start")
                .fix(DoctorFix::StartGateway),
                None,
            );
        }
        Err(error) => {
            return (
                DoctorCheck::new(
                    "gateway",
                    "Local gateway",
                    DoctorStatus::Error,
                    "runtime metadata could not be read",
                )
                .detail(format!("{error:#}")),
                None,
            );
        }
    };
    match super::super::runtime::pid_is_running(runtime.pid) {
        Ok(false) => (
            DoctorCheck::new(
                "gateway",
                "Local gateway",
                DoctorStatus::Warning,
                format!(
                    "stale runtime metadata found: pid {} is not running",
                    runtime.pid
                ),
            )
            .detail(format!("bind {}", runtime.bind))
            .fix(DoctorFix::CleanStaleGatewayMetadata)
            .fix(DoctorFix::StartGateway),
            None,
        ),
        Err(error) => (
            DoctorCheck::new(
                "gateway",
                "Local gateway",
                DoctorStatus::Error,
                format!("could not inspect gateway pid {}", runtime.pid),
            )
            .detail(format!("{error:#}")),
            None,
        ),
        Ok(true) => {
            let url = format!("http://{}/healthz", runtime.bind);
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build();
            let client = match client {
                Ok(client) => client,
                Err(error) => {
                    return (
                        DoctorCheck::new(
                            "gateway",
                            "Local gateway",
                            DoctorStatus::Error,
                            "could not build the local gateway health request",
                        )
                        .detail(format!("{error:#}")),
                        None,
                    );
                }
            };
            match client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    let current_version = env!("CARGO_PKG_VERSION");
                    let live = Some(LiveGateway { bind: runtime.bind });
                    match runtime.version.as_deref() {
                        Some(version) if version != current_version => (
                            DoctorCheck::new(
                                "gateway",
                                "Local gateway",
                                DoctorStatus::Warning,
                                format!(
                                    "running as pid {} on {}, but gateway version {} does not match CLI version {}",
                                    runtime.pid, runtime.bind, version, current_version
                                ),
                            )
                            .hint("codex-mixin restart")
                            .fix(DoctorFix::StartGateway),
                            live,
                        ),
                        version => {
                            let mut check = DoctorCheck::new(
                                "gateway",
                                "Local gateway",
                                DoctorStatus::Ok,
                                format!("running as pid {} on {}", runtime.pid, runtime.bind),
                            );
                            if let Some(version) = version {
                                check = check.detail(format!("version {version}"));
                            }
                            (check, live)
                        }
                    }
                }
                Ok(response) => (
                    DoctorCheck::new(
                        "gateway",
                        "Local gateway",
                        DoctorStatus::Error,
                        format!("healthz returned {}", response.status()),
                    )
                    .detail(url)
                    .hint("codex-mixin restart"),
                    None,
                ),
                Err(error) => (
                    DoctorCheck::new(
                        "gateway",
                        "Local gateway",
                        DoctorStatus::Error,
                        "process is alive but healthz is unreachable",
                    )
                    .detail(format!("{error:#}"))
                    .hint("codex-mixin restart"),
                    None,
                ),
            }
        }
    }
}

pub(super) async fn check_gateway_models(
    bind: SocketAddr,
    gateway_api_key: Option<&str>,
) -> (DoctorCheck, Option<Vec<String>>) {
    let url = format!("http://{bind}/v1/models");
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return (
                DoctorCheck::new(
                    "gateway_models",
                    "Gateway models",
                    DoctorStatus::Error,
                    "could not build the models list request",
                )
                .detail(format!("{error:#}")),
                None,
            );
        }
    };
    let mut request = client.get(&url);
    if let Some(key) = gateway_api_key {
        request = request.bearer_auth(key);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => {
            let body = match response.json::<serde_json::Value>().await {
                Ok(body) => body,
                Err(error) => {
                    return (
                        DoctorCheck::new(
                            "gateway_models",
                            "Gateway models",
                            DoctorStatus::Error,
                            "models list response is not valid JSON",
                        )
                        .detail(format!("{error:#}")),
                        None,
                    );
                }
            };
            let ids: Vec<String> = body
                .get("data")
                .and_then(serde_json::Value::as_array)
                .map(|models| {
                    models
                        .iter()
                        .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if ids.is_empty() {
                (
                    DoctorCheck::new(
                        "gateway_models",
                        "Gateway models",
                        DoctorStatus::Error,
                        "gateway /v1/models returned an empty list; Codex will have no models",
                    )
                    .detail(url),
                    Some(ids),
                )
            } else {
                (
                    DoctorCheck::new(
                        "gateway_models",
                        "Gateway models",
                        DoctorStatus::Ok,
                        format!("gateway /v1/models returned {} model(s)", ids.len()),
                    )
                    .detail(url),
                    Some(ids),
                )
            }
        }
        Ok(response)
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                || response.status() == reqwest::StatusCode::FORBIDDEN =>
        {
            (
                DoctorCheck::new(
                    "gateway_models",
                    "Gateway models",
                    DoctorStatus::Error,
                    format!(
                        "gateway rejected the models list request ({}); API key validation failed",
                        response.status()
                    ),
                )
                .detail(url),
                None,
            )
        }
        Ok(response) => (
            DoctorCheck::new(
                "gateway_models",
                "Gateway models",
                DoctorStatus::Error,
                format!("gateway /v1/models returned {}", response.status()),
            )
            .detail(url),
            None,
        ),
        Err(error) => (
            DoctorCheck::new(
                "gateway_models",
                "Gateway models",
                DoctorStatus::Error,
                "could not request the gateway models list",
            )
            .detail(format!("{url}: {error:#}")),
            None,
        ),
    }
}
