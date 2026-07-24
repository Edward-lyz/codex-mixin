use codex_mixin::config::GatewayConfig;
use serde_json::json;

use super::runtime::{load_runtime_metadata, pid_is_running};

pub(super) async fn benchmark_status() -> anyhow::Result<()> {
    proxy_benchmark_request(reqwest::Method::GET, None).await
}

pub(super) async fn benchmark_start(
    timeout_seconds: u64,
    providers: Vec<String>,
    models: Vec<String>,
) -> anyhow::Result<()> {
    proxy_benchmark_request(
        reqwest::Method::POST,
        Some(json!({
            "timeout_seconds": timeout_seconds,
            "providers": providers,
            "models": models,
        })),
    )
    .await
}

async fn proxy_benchmark_request(
    method: reqwest::Method,
    body: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    let runtime = load_runtime_metadata()?
        .filter(|runtime| pid_is_running(runtime.pid).unwrap_or(false))
        .ok_or_else(|| anyhow::anyhow!("gateway is not running"))?;
    let config = GatewayConfig::from_stored_config()?;
    let url = format!("http://{}/v1/model-benchmarks", runtime.bind);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut request = client.request(method, url);
    if let Some(key) = config.gateway_api_key {
        request = request.bearer_auth(key);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let response_body = response.text().await?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&response_body)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/error/message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or(response_body);
        anyhow::bail!("benchmark gateway request failed ({status}): {message}");
    }
    println!("{response_body}");
    Ok(())
}
